#!/usr/bin/env python3
"""Collect community reports about ChatGPT export structure; never certify drift."""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import html
import json
import os
import re
import sys
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass
from typing import Any


API = "https://api.github.com"
OWNER = "wenlan-app"
TRACKER_TITLE = "Wenlan ChatGPT export format watch checkpoint"
STATE_MARKER = "<!-- wenlan-chatgpt-format-watch-state-v1"
COMMENT_MARKER = "<!-- wenlan-chatgpt-format-watch-batch:"
EVENT_MARKER = "<!-- wenlan-chatgpt-format-watch-event:"
OVERLAP = dt.timedelta(hours=24)
BASELINE_WINDOW = dt.timedelta(days=7)
MAX_ISSUE_PAGES = 5
MAX_COMMIT_PAGES = 2
MAX_EVENTS = 400
MAX_SEEN = 1000
MAX_GITHUB_BODY_BYTES = 60_000
USER_AGENT = "wenlan-chatgpt-format-watch/1.0"

REPOSITORIES = {
    "ryayoung/chatgpt-schema": {
        "kind": "community",
        "paths": ["model/", "README.md"],
    },
    "difegam/chatgpt-to-markdown": {
        "kind": "community",
        "paths": ["src/chatgpt_to_markdown/models/", "docs/explanation/export-format.md"],
    },
    "Gusarovv/chatgpt-export-viewer": {
        "kind": "community",
        "paths": ["build_site.py", "tests/make_demo_export.py", ".github/workflows/demo.yml"],
    },
    "wenlan-app/wenlan": {
        "kind": "wenlan",
        "paths": [
            "crates/wenlan-core/src/chat_import/chatgpt.rs",
            "crates/wenlan-core/tests/chat_import_e2e.rs",
        ],
    },
}

CHATGPT_RE = re.compile(r"\bchat\s*gpt\b|\bchatgpt\b|\bopenai\b", re.I)
FORMAT_RE = re.compile(
    r"export[_ -]?manifest|conversation(?:s)?(?:[-_ ]\d+)?\.json|"
    r"(?:conversation|message|export)[ -]?(?:format|schema)|"
    r"shard(?:ed|ing|s)?|json schema|data export|export file",
    re.I,
)
CHANGE_RE = re.compile(
    r"chang(?:e|ed|es|ing)|new|support|break|broken|fail(?:s|ed|ure)?|"
    r"miss(?:es|ing)|drop(?:ped|s)?|fix(?:es|ed)?|issue|bug|incompatib|"
    r"unexpected|regression|different|not work|corrupt|partial|import",
    re.I,
)


class MonitorError(RuntimeError):
    pass


def validate_issue_body(body: str, purpose: str) -> None:
    if len(body.encode("utf-8")) > MAX_GITHUB_BODY_BYTES:
        raise MonitorError(f"{purpose} exceeds the safe GitHub issue-body size limit; refusing to write")


def utc_now() -> dt.datetime:
    return dt.datetime.now(dt.timezone.utc).replace(microsecond=0)


def iso(value: dt.datetime) -> str:
    return value.astimezone(dt.timezone.utc).isoformat().replace("+00:00", "Z")


def parse_time(value: str) -> dt.datetime:
    try:
        result = dt.datetime.fromisoformat(value.replace("Z", "+00:00"))
    except (ValueError, TypeError) as exc:
        raise MonitorError("GitHub returned an invalid timestamp") from exc
    if result.tzinfo is None:
        raise MonitorError("GitHub returned a timestamp without a timezone")
    return result.astimezone(dt.timezone.utc)


def relevance(item: dict[str, Any], require_chatgpt: bool) -> bool:
    text = f"{item.get('title') or ''}\n{item.get('body') or ''}"
    return bool(FORMAT_RE.search(text) and CHANGE_RE.search(text) and
                (not require_chatgpt or CHATGPT_RE.search(text)))


@dataclass
class State:
    last_successful_scan: str
    seen: set[str]


def state_json(state: State) -> dict[str, Any]:
    return {"version": 1, "last_successful_scan": state.last_successful_scan,
            "seen": sorted(state.seen)}


def replace_state(body: str, state: State) -> str:
    payload = json.dumps(state_json(state), sort_keys=True, separators=(",", ":"))
    block = f"{STATE_MARKER}\n{payload}\n-->"
    pattern = re.compile(re.escape(STATE_MARKER) + r".*?-->", re.S)
    if pattern.search(body):
        return pattern.sub(lambda _: block, body, count=1)
    return body.rstrip() + "\n\n" + block + "\n"


def parse_state(body: str) -> State:
    match = re.search(re.escape(STATE_MARKER) + r"\s*(\{.*?\})\s*-->", body, re.S)
    if not match:
        raise MonitorError("tracking issue has no valid state marker; refusing to advance")
    try:
        obj = json.loads(match.group(1))
    except json.JSONDecodeError as exc:
        raise MonitorError("tracking issue state is malformed; refusing to advance") from exc
    if obj.get("version") != 1 or not isinstance(obj.get("seen"), list):
        raise MonitorError("tracking issue state version is unsupported; refusing to advance")
    if len(obj["seen"]) > MAX_SEEN:
        raise MonitorError("tracking issue seen-key list exceeds its safety limit; refusing to advance")
    if not isinstance(obj.get("last_successful_scan"), str):
        raise MonitorError("tracking issue checkpoint is missing; refusing to advance")
    parse_time(obj["last_successful_scan"])
    if any(not isinstance(item, str) or len(item) > 200 for item in obj["seen"]):
        raise MonitorError("tracking issue seen-key list is invalid; refusing to advance")
    return State(obj["last_successful_scan"], set(obj["seen"]))


def api_url(url_or_path: str, params: dict[str, str] | None = None) -> str:
    url = urllib.parse.urljoin(API + "/", url_or_path.lstrip("/"))
    parts = urllib.parse.urlsplit(url)
    if parts.scheme != "https" or parts.netloc != "api.github.com":
        raise MonitorError("refusing a GitHub API URL outside api.github.com")
    query = dict(urllib.parse.parse_qsl(parts.query, keep_blank_values=True))
    if params:
        query.update(params)
    return urllib.parse.urlunsplit(("https", "api.github.com", parts.path,
                                    urllib.parse.urlencode(query), ""))


def validate_response_url(requested: str, returned: str) -> None:
    a, b = urllib.parse.urlsplit(requested), urllib.parse.urlsplit(returned)
    if b.scheme != "https" or b.netloc != "api.github.com" or a.netloc != b.netloc:
        raise MonitorError("GitHub API redirected outside api.github.com; refusing to continue")


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        raise MonitorError("GitHub API redirect refused")


class GitHub:
    def __init__(self, token: str | None, opener=None):
        self.token = token
        self.opener = opener or urllib.request.build_opener(NoRedirect)
        self.requests = 0

    def request(self, url_or_path: str, params: dict[str, str] | None = None,
                method: str = "GET", data: dict[str, Any] | None = None) -> Any:
        url = api_url(url_or_path, params)
        headers = {"Accept": "application/vnd.github+json", "User-Agent": USER_AGENT,
                   "X-GitHub-Api-Version": "2022-11-28"}
        if self.token:
            headers["Authorization"] = f"Bearer {self.token}"
        body = None if data is None else json.dumps(data).encode("utf-8")
        request = urllib.request.Request(url, data=body, headers=headers, method=method)
        self.requests += 1
        try:
            with self.opener.open(request, timeout=20) as response:
                validate_response_url(url, response.geturl())
                raw = response.read(2_000_001)
                if len(raw) > 2_000_000:
                    raise MonitorError("GitHub API response exceeded the 2 MB safety limit")
                if response.status == 204:
                    return None
                return json.loads(raw)
        except urllib.error.HTTPError as exc:
            if exc.code in (301, 302, 303, 307, 308):
                raise MonitorError("GitHub API redirect refused") from exc
            raise MonitorError(f"GitHub API returned HTTP {exc.code} for {urllib.parse.urlsplit(url).path}") from exc
        except urllib.error.URLError as exc:
            raise MonitorError("GitHub API request failed: " + type(exc.reason).__name__) from exc
        except json.JSONDecodeError as exc:
            raise MonitorError("GitHub API returned invalid JSON") from exc

    def pages(self, path: str, params: dict[str, str], max_pages: int = MAX_ISSUE_PAGES):
        all_items = []
        for page in range(1, max_pages + 1):
            result = self.request(path, {**params, "per_page": "100", "page": str(page)})
            if not isinstance(result, list):
                raise MonitorError("GitHub API list response had an unexpected shape")
            all_items.extend(result)
            if len(result) < 100:
                return all_items, page
        raise MonitorError(f"GitHub API pagination exceeded {max_pages * 100} items; scan is incomplete")


def event_key(repo: str, kind: str, identity: str, fingerprint: str) -> str:
    payload = f"{repo}\0{kind}\0{identity}\0{fingerprint}"
    digest = hashlib.sha256(payload.encode("utf-8", "replace")).hexdigest()[:32]
    return f"event:{digest}"


def pending_events(events: list[dict[str, Any]], state: State) -> list[dict[str, Any]]:
    last = parse_time(state.last_successful_scan)
    return [row for row in events
            if parse_time(row["updated_at"]) >= last - OVERLAP and row["key"] not in state.seen]


def issue_events(gh: GitHub, repo: str, kind: str, since: dt.datetime,
                 require_chatgpt: bool) -> tuple[list[dict[str, Any]], int]:
    items, pages = gh.pages(f"/repos/{repo}/issues", {
        "state": "all", "sort": "updated", "direction": "asc", "since": iso(since),
    }, max_pages=MAX_ISSUE_PAGES)
    found = []
    for item in items:
        updated = item.get("updated_at")
        if not isinstance(updated, str):
            raise MonitorError(f"{repo} issue response omitted updated_at")
        parse_time(updated)
        title, body = item.get("title") or "", item.get("body") or ""
        if repo == f"{OWNER}/wenlan" and title == TRACKER_TITLE and "pull_request" not in item:
            continue
        if not relevance({"title": title, "body": body}, require_chatgpt):
            continue
        number = item.get("number")
        if not isinstance(number, int):
            raise MonitorError(f"{repo} issue response omitted its number")
        kind_name = "pull request" if isinstance(item.get("pull_request"), dict) else "issue"
        kind_key = "pr" if kind_name == "pull request" else "issue"
        source = f"https://github.com/{repo}/{'pull' if kind_name == 'pull request' else 'issues'}/{number}"
        found.append({
            "repo": repo, "kind": kind_name, "number": number,
            "url": source, "updated_at": updated,
            # Identity is content-based. A comment or label update changes
            # updated_at but not the issue signal and must not re-notify.
            "key": event_key(repo, kind_key, str(number), title + "\n" + body),
            "reasons": ["ChatGPT context", "export-format structure term", "change or failure term"],
        })
    return found, pages


def commit_events(gh: GitHub, repo: str, paths: list[str], since: dt.datetime) -> tuple[list[dict[str, Any]], int]:
    grouped: dict[str, dict[str, Any]] = {}
    pages = 0
    for path in paths:
        items, count = gh.pages(f"/repos/{repo}/commits", {"path": path, "since": iso(since)}, max_pages=MAX_COMMIT_PAGES)
        pages += count
        for item in items:
            sha = item.get("sha")
            if not isinstance(sha, str) or not re.fullmatch(r"[0-9a-fA-F]{40}", sha):
                raise MonitorError(f"{repo} commit response had an invalid SHA")
            commit = item.get("commit")
            # GitHub's `since` filter uses commit/committer time. Use the same
            # timestamp for overlap/dedup so backdated author dates don't hide
            # a newly pushed parser change.
            committer = commit.get("committer") if isinstance(commit, dict) else None
            when = committer.get("date") if isinstance(committer, dict) else None
            if not isinstance(when, str):
                raise MonitorError(f"{repo} commit response omitted committer date")
            parse_time(when)
            html_url = f"https://github.com/{repo}/commit/{sha}"
            row = grouped.setdefault(sha, {
                "repo": repo, "kind": "parser or fixture commit", "sha": sha,
                "url": html_url, "updated_at": when, "paths": set(),
            })
            row["paths"].add(path)
            if when < row["updated_at"]:
                row["updated_at"] = when
    found = []
    for row in grouped.values():
        paths_in_change = sorted(row.pop("paths"))
        row["reasons"] = ["commit changed a watched ChatGPT parser, schema, or fixture path"]
        row["paths"] = paths_in_change
        row["key"] = event_key(repo, "commit", row["sha"], "\n".join(paths_in_change))
        found.append(row)
    return found, pages


def collect(gh: GitHub, start: dt.datetime, since: dt.datetime) -> dict[str, Any]:
    events, coverage = [], []
    for repo, settings in REPOSITORIES.items():
        try:
            issue_rows, issue_pages = issue_events(gh, repo, settings["kind"], since,
                                                    require_chatgpt=settings["kind"] == "wenlan")
            commit_rows, commit_pages = commit_events(gh, repo, settings["paths"], since)
            rows = issue_rows + commit_rows
            if len(events) + len(rows) > MAX_EVENTS:
                raise MonitorError("scan candidate count exceeded the safety limit")
            events.extend(rows)
            coverage.append({"repo": repo, "status": "complete", "issue_pages": issue_pages,
                             "watched_paths": len(settings["paths"]), "commit_pages": commit_pages,
                             "candidates": len(rows)})
        except MonitorError as exc:
            coverage.append({"repo": repo, "status": "error", "error": str(exc)})
    complete = all(row["status"] == "complete" for row in coverage)
    return {"version": 1, "started_at": iso(start), "window_start": iso(since),
            "complete": complete, "coverage": coverage,
            "candidates": sorted(events, key=lambda row: (row["updated_at"], row["key"])),
            "github_api_requests": gh.requests}


def find_tracker(gh: GitHub) -> dict[str, Any] | None:
    q = f'repo:{OWNER}/wenlan is:issue in:title "{TRACKER_TITLE}"'
    result = gh.request("/search/issues", {"q": q, "per_page": "10", "page": "1"})
    if not isinstance(result, dict) or result.get("incomplete_results"):
        raise MonitorError("GitHub tracker search was incomplete")
    items = result.get("items")
    if not isinstance(items, list) or len(items) > 1:
        raise MonitorError("expected at most one open checkpoint issue")
    if not items:
        return None
    item = items[0]
    if item.get("title") != TRACKER_TITLE or "pull_request" in item:
        raise MonitorError("checkpoint search returned an unexpected item")
    if item.get("state") != "open":
        raise MonitorError("checkpoint issue is closed; refusing to create a duplicate")
    return item


def render_candidate(row: dict[str, Any]) -> str:
    repo = row["repo"]
    number = row.get("number")
    identity = f"#{number}" if isinstance(number, int) else row["sha"][:12]
    reasons = "; ".join(row["reasons"])
    paths = ""
    if row.get("paths"):
        paths = " Changed watched paths: " + ", ".join(f"`{p}`" for p in row["paths"][:8]) + "."
    # Never echo issue titles or bodies: report only URLs and fixed reason labels.
    return f"- **Candidate** — [{repo} {identity}]({row['url']}) — {reasons}.{paths} Updated `{row['updated_at']}`. Review the source; this is not confirmation of an upstream format change."


def render_report(report: dict[str, Any]) -> str:
    lines = ["# ChatGPT export format community signals", "",
             f"Scan: `{report['started_at']}` · window starts `{report['window_start']}`", "",
             "Coverage is limited to the curated public GitHub repositories listed below; it is not an exhaustive scan of community discussions.", "",
             "These are community and repository signals. They do not establish that OpenAI changed the official export or that Wenlan is incompatible.", "",
             "## Coverage", ""]
    for row in report["coverage"]:
        if row["status"] == "complete":
            lines.append(f"- `{row['repo']}`: complete; {row['issue_pages']} issue/PR pages, {row['watched_paths']} watched paths, {row['commit_pages']} commit pages, {row['candidates']} candidates")
        else:
            lines.append(f"- `{row['repo']}`: **ERROR** — {row['error']}")
    lines += ["", "## Candidate signals", ""]
    if report["candidates"]:
        lines.extend(render_candidate(row) for row in report["candidates"])
    elif report["complete"]:
        lines.append("No new matching signals were found in this scan window. This is not evidence that the format has not changed.")
    else:
        lines.append("Scan incomplete; no all-clear conclusion.")
    lines += ["", f"GitHub API requests: {report['github_api_requests']}", ""]
    return "\n".join(lines)


def publish_batch(gh: GitHub, tracker: dict[str, Any], pending: list[dict[str, Any]]) -> bool:
    if not pending:
        return False
    comments = gh.request(f"/repos/{OWNER}/wenlan/issues/{tracker['number']}/comments",
                          {"per_page": "100", "page": "1"})
    if not isinstance(comments, list):
        raise MonitorError("checkpoint comments response was malformed")
    if len(comments) == 100:
        raise MonitorError("checkpoint has 100 or more comments; refusing unsafe dedupe")
    already_reported = set()
    for comment in comments:
        if not isinstance(comment, dict) or not isinstance(comment.get("body"), str):
            continue
        already_reported.update(re.findall(
            re.escape(EVENT_MARKER) + r"([A-Za-z0-9:/._-]{1,200})\s*-->", comment["body"]))
    pending = [row for row in pending if row["key"] not in already_reported]
    if not pending:
        return False
    keys = sorted(row["key"] for row in pending)
    batch = hashlib.sha256("\n".join(keys).encode()).hexdigest()[:24]
    marker = f"{COMMENT_MARKER}{batch} -->"
    event_markers = "\n".join(f"{EVENT_MARKER}{key} -->" for key in keys)
    body = "## New candidate signals\n\n" + "\n".join(render_candidate(row) for row in pending) + f"\n\n{event_markers}\n{marker}"
    validate_issue_body(body, "candidate comment")
    gh.request(f"/repos/{OWNER}/wenlan/issues/{tracker['number']}/comments", method="POST", data={"body": body})
    return True


def persist(gh: GitHub, tracker: dict[str, Any] | None, report: dict[str, Any],
            prior: State | None, publish: bool) -> None:
    if not report["complete"]:
        raise MonitorError("scan is incomplete; checkpoint will not advance")
    if prior is None:
        # Establish a no-flood baseline: everything found in this bounded scan
        # is already examined and won't be announced during the overlap.
        baseline_seen = trim_seen({row["key"] for row in report["candidates"]})
        state = State(report["started_at"], baseline_seen)
        body = replace_state("This issue stores the weekly ChatGPT export-format signal scan checkpoint. Candidate reports are not format confirmation.", state)
        validate_issue_body(body, "initial checkpoint")
        gh.request(f"/repos/{OWNER}/wenlan/issues", method="POST",
                   data={"title": TRACKER_TITLE, "body": body})
        return
    # The overlap catches late indexing. Stable event keys prevent duplicates.
    pending = pending_events(report["candidates"], prior)
    new_seen = set(prior.seen)
    new_seen.update(row["key"] for row in report["candidates"])
    if publish:
        if tracker is None:
            raise MonitorError("checkpoint disappeared during publish")
        next_state = State(report["started_at"], trim_seen(new_seen))
        body = replace_state(tracker.get("body") or "", next_state)
        validate_issue_body(body, "updated checkpoint")
        publish_batch(gh, tracker, pending)
        gh.request(f"/repos/{OWNER}/wenlan/issues/{tracker['number']}", method="PATCH", data={"body": body})


def trim_seen(keys: set[str]) -> set[str]:
    return set(sorted(keys)[-MAX_SEEN:])


def write_outputs(report: dict[str, Any], markdown: str, json_path: str,
                  markdown_path: str | None, summary_path: str | None) -> None:
    if json_path:
        with open(json_path, "w", encoding="utf-8") as handle:
            json.dump(report, handle, ensure_ascii=False, indent=2)
    if markdown_path:
        with open(markdown_path, "w", encoding="utf-8") as handle:
            handle.write(markdown + "\n")
    if summary_path:
        with open(summary_path, "a", encoding="utf-8") as handle:
            handle.write(markdown + "\n")


def run(args: argparse.Namespace) -> int:
    token = os.environ.get("GITHUB_TOKEN") or os.environ.get("GH_TOKEN")
    if not token:
        raise MonitorError("GITHUB_TOKEN is required for GitHub API rate limits and checkpoint access")
    gh = GitHub(token)
    start = utc_now()
    tracker = find_tracker(gh) if args.publish else None
    prior = parse_state(tracker.get("body") or "") if tracker else None
    if prior:
        since = parse_time(prior.last_successful_scan) - OVERLAP
    elif args.since:
        since = parse_time(args.since)
    else:
        window = BASELINE_WINDOW if args.publish else dt.timedelta(days=14)
        since = start - window
    report = collect(gh, start, since)
    markdown = render_report(report)
    write_outputs(report, markdown, args.output, args.markdown_output,
                  os.environ.get("GITHUB_STEP_SUMMARY"))
    print(markdown)
    if not report["complete"]:
        raise MonitorError("one or more source checks failed; see report for coverage")
    if args.publish:
        if prior is None:
            persist(gh, tracker, report, prior, publish=True)
        else:
            persist(gh, tracker, report, prior, publish=True)
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--publish", action="store_true", help="write a shared checkpoint/notification; dry-run is the default")
    parser.add_argument("--since", help="UTC timestamp for a read-only preview, e.g. 2026-10-20T00:00:00Z")
    parser.add_argument("--output", default="chatgpt-format-watch.json")
    parser.add_argument("--markdown-output", default="chatgpt-format-watch.md")
    args = parser.parse_args()
    try:
        return run(args)
    except (MonitorError, OSError) as exc:
        print(f"ChatGPT format watch failed: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
