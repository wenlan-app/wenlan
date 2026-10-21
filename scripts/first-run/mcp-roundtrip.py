#!/usr/bin/env python3
"""First-run gauntlet: drive `wenlan-mcp` over stdio JSON-RPC.

initialize -> tools/list -> capture -> recall (polled up to 60s) [-> brief].
Lifted from scripts/smoke-mcp.sh. A failed step never raises: every step
appends a row to $GAUNTLET_OUT/findings.tsv (same TSV shape as lib.sh:
channel, name, status, rc, detail) and writes its raw response to
$GAUNTLET_OUT/checks/<name>.log. Always exits 0 — the channel script's
`evaluate` turns FAIL rows into a nonzero exit.

Env:
  MCP_BIN               path to wenlan-mcp (required)
  MCP_ARGS              JSON list of extra args (default [])
  EXPECT_TOOL_COUNT     when set, tools/list must return exactly this many tools
  EXPECT_ENRICHMENT     when set (e.g. "off"), the capture receipt must report
                        that enrichment state
  MCP_TOOLS             comma list of tools that must be advertised (default
                        "capture,recall"); when it names "brief", brief is also
                        called with {} and must not error
  GAUNTLET_OUT          default ./gauntlet-out
  GAUNTLET_CHANNEL      default "mcp"
  WENLAN_MCP_CACHE_DIR  default $GAUNTLET_OUT/mcp-cache (keeps the self-update
                        probe out of the user cache)
"""

import json
import re
import os
import queue
import subprocess
import sys
import threading
import time
from pathlib import Path

SENTINEL = "walrus-observatory-5507"
OUT = Path(os.environ.get("GAUNTLET_OUT") or Path.cwd() / "gauntlet-out")
CHANNEL = os.environ.get("GAUNTLET_CHANNEL") or "mcp"
CHECKS = OUT / "checks"
TSV = OUT / "findings.tsv"
CHECKS.mkdir(parents=True, exist_ok=True)
os.environ.setdefault("WENLAN_MCP_CACHE_DIR", str(OUT / "mcp-cache"))
Path(os.environ["WENLAN_MCP_CACHE_DIR"]).mkdir(parents=True, exist_ok=True)


def _escape(detail):
    # One line, no tabs, capped — mirrors _gauntlet_escape in lib.sh.
    return detail.replace("\t", " ").replace("\r", " ").replace("\n", "|")[:2000]


def record(status, name, rc, detail=""):
    with TSV.open("a", encoding="utf-8") as fh:
        fh.write(f"{CHANNEL}\t{name}\t{status}\t{rc}\t{_escape(detail)}\n")
    short = _escape(detail)[:200]
    print(f"[{status}] {name} (rc={rc})" + (f" — {short}" if short else ""), flush=True)


def expect(names):
    """Declare the PASS/FAIL rows this run owes. Derived from MCP_TOOLS, never
    pasted by the calling channel: every tool beyond capture/recall is only
    stepped when it is required."""
    with (OUT / "expected.tsv").open("a", encoding="utf-8") as fh:
        for name in names:
            fh.write(f"{CHANNEL}\t{name}\n")


def log(name, text):
    (CHECKS / f"{name}.log").write_text(text, encoding="utf-8")


class StepFailed(Exception):
    pass


class Transport(StepFailed):
    """The server died or stopped answering: later steps cannot run."""


# ---------------------------------------------------------------------------

required_tools = [t.strip() for t in (os.environ.get("MCP_TOOLS") or "capture,recall").split(",") if t.strip()]
# Declared before the first guard below, so a run that dies at MCP_BIN still owes
# the rows it never recorded rather than quietly owing nothing.
OPTIONAL_STEPS = ("update_brief", "brief", "write_page", "list_pages", "read_page")
expect(["mcp-initialize", "mcp-tools-list", "mcp-capture", "mcp-recall"]
       + [f"mcp-{t}" for t in OPTIONAL_STEPS if t in required_tools])

if not os.environ.get("MCP_BIN"):
    record("FAIL", "mcp-initialize", 2, "MCP_BIN (path to wenlan-mcp) is required")
    sys.exit(0)

try:
    extra_args = json.loads(os.environ.get("MCP_ARGS") or "[]")
    if not isinstance(extra_args, list):
        raise ValueError("MCP_ARGS must be a JSON list")
except ValueError as exc:
    record("FAIL", "mcp-initialize", 2, f"bad MCP_ARGS: {exc}")
    sys.exit(0)

expect_count = os.environ.get("EXPECT_TOOL_COUNT")

stderr_log = (CHECKS / "mcp-server-stderr.log").open("wb")
try:
    proc = subprocess.Popen(
        [os.environ["MCP_BIN"], *extra_args],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=stderr_log,
    )
except OSError as exc:
    record("FAIL", "mcp-initialize", 127, f"cannot start {os.environ['MCP_BIN']}: {exc}")
    sys.exit(0)

buf = b""
next_id = 0
dead = None  # reason string once the transport is gone

# A pump thread hands stdout chunks over a queue: select() only accepts
# sockets on Windows (WSAENOTSOCK/WSANOTINITIALISED on a pipe), and a queue
# read with a timeout is the same wait on every platform. An empty chunk
# means the server closed stdout.
_chunks = queue.Queue()


def _pump():
    fd = proc.stdout.fileno()
    while True:
        try:
            chunk = os.read(fd, 65536)
        except OSError:
            chunk = b""
        _chunks.put(chunk)
        if not chunk:
            return


threading.Thread(target=_pump, name="stdout-pump", daemon=True).start()


def send(obj):
    try:
        proc.stdin.write((json.dumps(obj) + "\n").encode())
        proc.stdin.flush()
    except (BrokenPipeError, OSError) as exc:
        raise Transport(f"wenlan-mcp closed stdin: {exc}")


def recv(want_id, timeout=60):
    # Frame by hand from raw chunks: a buffered readline() can hang on a
    # partial frame or strand a second frame already sitting in Python's
    # buffer while the caller waits for more bytes.
    global buf
    deadline = time.monotonic() + timeout
    while True:
        while b"\n" in buf:
            line, buf = buf.split(b"\n", 1)
            try:
                msg = json.loads(line)
            except json.JSONDecodeError:
                continue
            if msg.get("id") == want_id:
                return msg
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise Transport(f"timeout waiting for response id={want_id}")
        try:
            chunk = _chunks.get(timeout=min(remaining, 1))
        except queue.Empty:
            continue
        if not chunk:
            raise Transport("wenlan-mcp closed stdout")
        buf += chunk


def call(method, params=None, timeout=60):
    global next_id
    next_id += 1
    req = {"jsonrpc": "2.0", "id": next_id, "method": method}
    if params is not None:
        req["params"] = params
    send(req)
    return recv(next_id, timeout=timeout)


def expect_ok(msg, what):
    if "error" in msg:
        raise StepFailed(f"{what} returned JSON-RPC error: {json.dumps(msg['error'])}")
    if msg.get("result", {}).get("isError"):
        raise StepFailed(f"{what} returned tool error: {json.dumps(msg['result'])}")
    return msg["result"]


def step(name, fn):
    """Run fn(); record PASS/FAIL; return its value or None. Never raises."""
    global dead
    if dead:
        record("FAIL", name, 1, f"not run: {dead}")
        return None
    try:
        value, detail = fn()
    except Transport as exc:
        dead = f"transport failed at {name}: {exc}"
        log(name, str(exc))
        record("FAIL", name, 1, str(exc))
        return None
    except StepFailed as exc:
        log(name, str(exc))
        record("FAIL", name, 1, str(exc))
        return None
    except Exception as exc:  # noqa: BLE001 — a driver bug must still be a row, not a crash
        log(name, repr(exc))
        record("FAIL", name, 1, f"driver error: {exc!r}")
        return None
    record("PASS", name, 0, detail)
    return value


# ---------------------------------------------------------------------------

def do_initialize():
    msg = call("initialize", {
        "protocolVersion": "2024-11-05", "capabilities": {},
        "clientInfo": {"name": "first-run-gauntlet", "version": "0"}})
    log("mcp-initialize", json.dumps(msg, indent=2))
    init = expect_ok(msg, "initialize")
    send({"jsonrpc": "2.0", "method": "notifications/initialized"})
    server = init.get("serverInfo", {})
    return init, f"server {server.get('name', '?')} {server.get('version', '')}".strip()


def do_tools_list():
    msg = call("tools/list")
    log("mcp-tools-list", json.dumps(msg, indent=2))
    tools = sorted(t["name"] for t in expect_ok(msg, "tools/list")["tools"])
    record("INFO", "mcp-tool-count", 0, f"{len(tools)}: {', '.join(tools)}")
    problems = []
    missing = [t for t in required_tools if t not in tools]
    if missing:
        problems.append(f"missing required tool(s) {missing}")
    if expect_count is not None and len(tools) != int(expect_count):
        problems.append(f"expected {expect_count} tools, got {len(tools)}")
    if problems:
        raise StepFailed("; ".join(problems) + f" (advertised: {tools})")
    return tools, f"{len(tools)} tools"


def do_capture():
    # The caller sets type and entity itself, as the capture description asks:
    # a new entity name that comes with its type is created and linked even
    # when nothing runs in the background.
    msg = call("tools/call", {
        "name": "capture",
        "arguments": {
            "content": f"The {SENTINEL} sentinel lives in the MCP smoke.",
            "memory_type": "fact",
            "entity": "Walrus Observatory",
            "entity_type": "project"}})
    log("mcp-capture", json.dumps(msg, indent=2))
    text = _text_of(expect_ok(msg, "capture"))
    if not re.search(r"^entity_id: \S+$", text, re.M):
        raise StepFailed(f"capture did not link the named entity: {text!r}")
    want = os.environ.get("EXPECT_ENRICHMENT")
    if want and f"enrichment: {want}." not in text:
        raise StepFailed(f"capture receipt does not say enrichment: {want}: {text!r}")
    return True, "captured sentinel, entity linked"


def do_recall():
    # One monotonic window for the whole poll — per-retry timeouts would let
    # the advertised 60s stretch to 30 minutes against a hung daemon.
    poll_deadline = time.monotonic() + 60
    polls = 0
    last = None
    while time.monotonic() < poll_deadline:
        polls += 1
        last = call("tools/call", {
            "name": "recall",
            "arguments": {"query": "walrus observatory sentinel"}},
            timeout=max(1, poll_deadline - time.monotonic()))
        log("mcp-recall", json.dumps(last, indent=2))
        result = expect_ok(last, "recall")
        if SENTINEL in json.dumps(result):
            return True, f"sentinel round-tripped after {polls} poll(s)"
        time.sleep(2)
    raise StepFailed(f"captured sentinel not present in recall output within 60s ({polls} polls); last: {json.dumps(last)}")


BRIEF_ITEM = f"Follow up on the {SENTINEL} sentinel from the MCP smoke"
PAGE_TITLE = f"{SENTINEL} sentinel notes"
# Three distinct facts: the daemon rejects near-duplicate captures, and a page
# needs at least three different cited memories whose text its body restates.
PAGE_FACTS = [
    f"The {SENTINEL} observatory logs one tide reading every six hours.",
    f"Tide readings at the {SENTINEL} observatory are kept for ninety days before archiving.",
    f"The {SENTINEL} observatory archive is exported as CSV on the first day of each month.",
]


def _text_of(result):
    return "\n".join(
        block.get("text", "") for block in result.get("content", []) if block.get("type") == "text"
    )


# Explicit Space: the smoke daemon has no default Space configured and
# update_brief refuses to guess one. update_brief also registers the Space, so
# the page steps only scope to it when that step runs first.
SMOKE_SPACE = {"space": "smoke"} if "update_brief" in required_tools else {}


def do_update_brief():
    msg = call("tools/call", {"name": "update_brief", "arguments": {
        "space": "smoke",
        "operation_id": f"first-run-{SENTINEL}",
        "mutations": [{"kind": "add", "text": BRIEF_ITEM, "state": "backlog"}]}})
    log("mcp-update_brief", json.dumps(msg, indent=2))
    result = expect_ok(msg, "update_brief")
    receipt = result.get("structuredContent") or {}
    if receipt.get("conflicts"):
        raise StepFailed(f"update_brief reported conflicts: {receipt['conflicts']}")
    if not receipt.get("applied"):
        raise StepFailed(f"update_brief applied nothing: {json.dumps(receipt)}")
    return True, f"brief version {receipt.get('brief_version')} ({len(receipt['applied'])} applied)"


def do_brief():
    msg = call("tools/call", {"name": "brief", "arguments": dict(SMOKE_SPACE)})
    log("mcp-brief", json.dumps(msg, indent=2))
    result = expect_ok(msg, "brief")
    if "update_brief" in required_tools and BRIEF_ITEM not in json.dumps(result):
        raise StepFailed("the item added by update_brief is missing from the brief read")
    return True, f"brief ok ({len(json.dumps(result))} bytes)"


def do_write_page():
    # A page needs three distinct cited memories whose text its body matches.
    ids = []
    for fact in PAGE_FACTS:
        msg = call("tools/call", {"name": "capture", "arguments": {"content": fact, **SMOKE_SPACE}})
        log("mcp-write_page", json.dumps(msg, indent=2))
        text = _text_of(expect_ok(msg, "capture"))
        m = re.search(r"^source_memory_id: (\S+)$", text, re.M)
        if not m:
            raise StepFailed(f"capture returned no source_memory_id: {text!r}")
        ids.append(m.group(1))
    msg = call("tools/call", {"name": "write_page", "arguments": {
        "title": PAGE_TITLE, **SMOKE_SPACE,
        "content": "\n\n".join(PAGE_FACTS),
        "summary": f"Notes on the {SENTINEL} sentinel.",
        "source_memory_ids": ids}})
    log("mcp-write_page", json.dumps(msg, indent=2))
    text = _text_of(expect_ok(msg, "write_page"))
    m = re.search(r"^(?:Created page|Attached to existing page) (\S+)", text, re.M)
    if not m:
        raise StepFailed(f"write_page returned no page id: {text!r}")
    return m.group(1), f"page {m.group(1)}"


def do_list_pages():
    msg = call("tools/call", {"name": "list_pages", "arguments": {"limit": 50, **SMOKE_SPACE}})
    log("mcp-list_pages", json.dumps(msg, indent=2))
    result = expect_ok(msg, "list_pages")
    pages = (result.get("structuredContent") or {}).get("pages")
    if not isinstance(pages, list):
        raise StepFailed(f"list_pages returned no pages array: {json.dumps(result)[:300]}")
    if page_id is not None and not any(p.get("page_id") == page_id for p in pages):
        raise StepFailed(f"page {page_id} written this run is missing from list_pages")
    return pages, f"{len(pages)} page(s)"


def do_read_page():
    target = page_id
    if target is None:
        listed = step_results.get("mcp-list_pages") or []
        target = listed[0]["page_id"] if listed else None
    if target is None:
        raise StepFailed("no page to read: write_page did not run and list_pages returned nothing")
    msg = call("tools/call", {"name": "read_page", "arguments": {"page_id": target}})
    log("mcp-read_page", json.dumps(msg, indent=2))
    result = expect_ok(msg, "read_page")
    body = result.get("structuredContent") or {}
    # The page object is the daemon's inventory entry: the id key is `id`.
    if (body.get("page") or {}).get("id") != target:
        raise StepFailed(f"read_page returned a different page: {json.dumps(body)[:300]}")
    sources = body.get("sources")
    if page_id is not None and (not isinstance(sources, list) or len(sources) < 3):
        raise StepFailed(f"read_page returned {sources!r} sources for a page written with three")
    return True, f"page {target} with {len(sources) if isinstance(sources, list) else '?'} source(s)"


step_results = {}
page_id = None


def optional(name, fn):
    """Run one MCP_TOOLS-gated step and keep its value for later steps."""
    if tools is not None and name not in tools:
        record("FAIL", f"mcp-{name}", 1, f"not run: {name} is not advertised by tools/list")
        return None
    value = step(f"mcp-{name}", fn)
    step_results[f"mcp-{name}"] = value
    return value


step("mcp-initialize", do_initialize)
tools = step("mcp-tools-list", do_tools_list)
step("mcp-capture", do_capture)
step("mcp-recall", do_recall)
if "update_brief" in required_tools:
    optional("update_brief", do_update_brief)
if "brief" in required_tools:
    optional("brief", do_brief)
if "write_page" in required_tools:
    page_id = optional("write_page", do_write_page)
if "list_pages" in required_tools:
    optional("list_pages", do_list_pages)
if "read_page" in required_tools:
    optional("read_page", do_read_page)

try:
    proc.stdin.close()
except OSError:
    pass
try:
    proc.wait(timeout=10)
except subprocess.TimeoutExpired:
    proc.kill()
    proc.wait()
stderr_log.close()
sys.exit(0)
