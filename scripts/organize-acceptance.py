#!/usr/bin/env python3
"""Seed/read back a PUBLIC synthetic organization fixture through stdio MCP.

This is a test driver, never a substitute for the cold agent. Start an isolated
daemon first; the driver refuses production ports and checks its live pages root.
The agent receives only the generated request and the candidate skills/tools.
"""
import argparse
import json
import os
from pathlib import Path
import queue
import re
import subprocess
import threading
import urllib.request


class Mcp:
    def __init__(self, binary, origin, root):
        env = dict(os.environ, WENLAN_NO_AUTOSTART="1", WENLAN_MCP_CACHE_DIR=str(root / "mcp-cache"))
        for name in ("WENLAN_SPACE", "WENLAN_DEFAULT_SPACE"):
            env.pop(name, None)
        self.log = (root / "driver-mcp.stderr").open("ab")
        self.proc = subprocess.Popen([binary, "--origin-url", origin, "--agent-name", "organize-fixture"],
                                     stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=self.log, env=env)
        self.lines = queue.Queue()
        self.serial = 0
        threading.Thread(target=self.pump, daemon=True).start()
        self.rpc("initialize", {"protocolVersion": "2024-11-05", "capabilities": {},
                                "clientInfo": {"name": "organize-fixture", "version": "1"}})
        self.send({"jsonrpc": "2.0", "method": "notifications/initialized"})

    def pump(self):
        for line in self.proc.stdout:
            self.lines.put(line)
        self.lines.put(None)

    def send(self, value):
        self.proc.stdin.write((json.dumps(value) + "\n").encode())
        self.proc.stdin.flush()

    def rpc(self, method, params):
        self.serial += 1
        self.send({"jsonrpc": "2.0", "id": self.serial, "method": method, "params": params})
        while True:
            line = self.lines.get(timeout=90)
            if line is None:
                raise RuntimeError("MCP closed stdout")
            value = json.loads(line)
            if value.get("id") == self.serial:
                if "error" in value:
                    raise RuntimeError(value["error"])
                return value["result"]

    def call(self, tool_name, **arguments):
        result = self.rpc("tools/call", {"name": tool_name, "arguments": arguments})
        if result.get("isError"):
            raise RuntimeError((tool_name, result))
        if "structuredContent" in result:
            return result["structuredContent"]
        return "\n".join(c.get("text", "") for c in result.get("content", []))

    def close(self):
        self.proc.stdin.close()
        try:
            self.proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.proc.terminate()
            self.proc.wait(timeout=10)
        self.log.close()


PAGES = [
    ("personal", "林以安", "我是林以安。這是我的個人首頁。", "home"),
    ("writing", "個人品牌", "林以安的品牌定位：分享通用 AI 工具清單。這個定位已過期，待本人更新。", "brand"),
    ("writing", "可重用的表達準則", "跨平台共用：先展示做出的產品與實際工作流程，再談心得。保留失敗與限制，不虛構成效。", "shared"),
    ("writing", "每週短文試寫", "Threads 草稿，尚未發布。主題是把真實產品的製作過程寫成短文。", "threads"),
    ("writing", "照片發布工作表", "Instagram / Facebook 由另一個 Meta 流程負責維護。本頁已發布紀錄：2025-01-02 分享產品製作照片，成效未知。請保留原頁及內容。", "meta"),
    ("writing", "產品更新短訊", "X 由另一個流程負責。本頁是產品更新短訊草稿，尚未發布。請保留原頁及內容。", "x"),
    ("writing", "圖文步驟紀錄", "小紅書的產品教學貼文曾發布，後來已刪除。沒有保留成效資料，不代表渠道失敗。", "xiaohongshu"),
    ("site", "青禾網站", "林以安的個人網站專案，展示真實產品與工作流程。沿用這個專案頁，別複製到品牌資料中。", "site"),
    ("writing", "春季發布回顧", "一次跨平台活動：Threads 與個人網站同時介紹新作品。已執行，未量測漲粉。共用規則見 [[可重用的表達準則]]。", "campaign"),
    ("writing", "創業資金筆記", "與個人品牌無關的公司現金流閱讀筆記，保留原處。", "unrelated"),
    ("other", "個人品牌", "同名但屬於另一位創作者的讀書筆記，與林以安無關。", "same_name"),
]

REQUEST = """請在 Wenlan 幫我整理個人品牌。我叫林以安，定位已從分享 AI 工具清單改成「用 AI 做出真實產品與工作流程的 builder」，請記住這個已確認的定位並更新既有品牌介紹。建立或沿用我的個人主頁，下面連到個人品牌，再整理全渠道共用內容、各平台資料與跨平台活動。既有資料有 Threads、Instagram／Facebook、X、小紅書和個人網站；補上 Reddit、Show HN、Product Hunt 的可執行規劃。Meta／X 已有別的流程負責，沿用原頁與原內容。我不記得頁名或連結，請自己找，不要叫我選 Space 或內部分類。不要把不相關資料搬進來，也不要把未知成效當成果。只使用 Wenlan 正式工具，完成後讀回核對，告訴我入口與尚未生效或受阻的項目。"""


def inventory(mcp):
    result = []
    for status in ("active", "draft", "archived"):
        offset = 0
        while True:
            pages = mcp.call("list_pages", status=status, limit=200, offset=offset)["pages"]
            result.extend(pages)
            if len(pages) < 200:
                break
            offset += len(pages)
    return result


def snapshot(mcp):
    return {p["page_id"]: mcp.call("read_page", page_id=p["page_id"]) for p in inventory(mcp)
            if p.get("creation_kind") != "entity"}


def seed(mcp, root):
    if inventory(mcp):
        raise RuntimeError("seed requires an empty scratch library")
    for space in ("personal", "writing", "site", "other"):
        mcp.call("update_brief", space=space, operation_id=f"fixture-space-{space}",
                 mutations=[{"kind": "add", "text": "Public synthetic organization fixture.", "state": "backlog"}])
    source = mcp.call("capture", content="跨平台寫作先展示真實產品與工作流程，保留限制，不虛構成效。", space="writing", memory_type="preference")
    source_id = re.search(r"source_memory_id: (\S+)", source).group(1)
    ids = {}
    for space, title, content, role in PAGES:
        sources = [source_id] if role == "shared" else []
        reply = mcp.call("write_page", title=title, content=content, space=space, source_memory_ids=sources)
        match = re.search(r"(?:Created page|Attached to existing page) (\S+)", reply)
        if not match:
            raise RuntimeError(reply)
        ids[role] = match.group(1)
    mcp.call("create_entity", name="林以安", entity_type="person", space="personal")
    mcp.call("create_entity", name="青禾網站", entity_type="project", space="site")
    (root / "fixture-ids.json").write_text(json.dumps(ids, ensure_ascii=False, indent=2))
    (root / "before.json").write_text(json.dumps(snapshot(mcp), ensure_ascii=False, indent=2))
    (root / "request.txt").write_text(REQUEST)
    print(json.dumps({"seeded_pages": len(ids), "request": str(root / "request.txt")}, ensure_ascii=False))


def verify(mcp, root, rerun):
    before = json.loads((root / "before.json").read_text())
    ids = json.loads((root / "fixture-ids.json").read_text())
    after = snapshot(mcp)
    for role, page_id in ids.items():
        old, new = before[page_id]["page"], after[page_id]["page"]
        for field in ("id", "created_at", "space"):
            assert old[field] == new[field], (role, field)
        assert set(old["source_memory_ids"]) <= set(new["source_memory_ids"]), (role, "lost sources")
        if role not in ("home", "brand"):
            assert old["source_memory_ids"] == new["source_memory_ids"], (role, "sources changed")
            for field in ("content", "version"):
                assert old[field] == new[field], (role, field)
            assert before[page_id]["sources"] == after[page_id]["sources"], (role, "sources")
        if role in ("unrelated", "same_name", "site"):
            assert old.get("storage_path") == new.get("storage_path"), (role, "path")
    assert "builder" in after[ids["brand"]]["page"]["content"], "brand positioning not updated"
    combined = "\n".join(p["page"]["content"] for p in after.values())
    for platform in ("Reddit", "Show HN", "Product Hunt"):
        assert platform in combined, f"missing plan: {platform}"
    home_links = mcp.call("get_page_links", page_id=ids["home"])
    assert any(link.get("target_page_id") == ids["brand"] for link in home_links["outbound"]), "home does not reach brand"
    brand_links = mcp.call("get_page_links", page_id=ids["brand"])
    assert brand_links["outbound"], "brand has no navigation"
    inspected = set(after) - set(before) | {ids["home"], ids["brand"]}
    for page_id in inspected:
        links = mcp.call("get_page_links", page_id=page_id)
        assert all(link.get("target_page_id") for link in links["outbound"]), (page_id, "unresolved links", links)
    moved = [role for role, pid in ids.items() if before[pid]["page"].get("storage_path") != after[pid]["page"].get("storage_path")]
    assert moved, "no existing page organized into folders"
    if rerun:
        first = json.loads((root / "after-first.json").read_text())
        assert set(first) == set(after), "rerun added or removed pages"
        for pid, previous in first.items():
            assert previous["page"]["content"] == after[pid]["page"]["content"], (pid, "rerun rewrote content")
            assert previous["page"]["version"] == after[pid]["page"]["version"], (pid, "rerun bumped version")
    else:
        (root / "after-first.json").write_text(json.dumps(after, ensure_ascii=False, indent=2))
    print(json.dumps({"verified": True, "pages": len(after), "moved_roles": moved, "rerun": rerun}, ensure_ascii=False))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("seed", "verify", "verify-rerun", "snapshot"))
    parser.add_argument("--binary", required=True)
    parser.add_argument("--origin", required=True)
    parser.add_argument("--root", required=True, type=Path)
    args = parser.parse_args()
    root = args.root.resolve(strict=True)
    from urllib.parse import urlparse
    origin = urlparse(args.origin)
    assert origin.hostname in ("127.0.0.1", "localhost") and origin.port not in (None, 7878), "scratch origin required"
    assert (root / "ORGANIZE_SYNTHETIC_FIXTURE").is_file(), "explicit fixture marker required"
    with urllib.request.urlopen(args.origin + "/api/knowledge/path", timeout=5) as response:
        live = json.load(response)
    assert Path(live["path"]).resolve() == root / "pages", "live page root is not scratch"
    mcp = Mcp(args.binary, args.origin, root)
    try:
        if args.action == "seed":
            seed(mcp, root)
        elif args.action == "snapshot":
            print(json.dumps(snapshot(mcp), ensure_ascii=False, indent=2))
        else:
            verify(mcp, root, args.action == "verify-rerun")
    finally:
        mcp.close()


if __name__ == "__main__":
    main()
