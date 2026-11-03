#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Exercise the MCP caller embedded in the Grok Bot setup guide."""

import json
import os
import re
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
GUIDE = ROOT / "docs/setup-with-ai.md"


def mcp_function():
    text = GUIDE.read_text(encoding="utf-8")
    section = text.split("### MCP over a local shell\n", 1)[1]
    section = section.split("\n### Verify\n", 1)[0]
    match = re.search(r"```bash\n(.*?)\n```", section, re.DOTALL)
    if not match:
        raise AssertionError("Grok Bot guide has no MCP shell example")
    block = match.group(1)
    function = re.search(r"(?ms)^wenlan_mcp_call\(\) \{.*?^\}", block)
    if not function:
        raise AssertionError("MCP example must define wenlan_mcp_call")
    return block[: function.end()]


MOCK_MCP = r'''#!/usr/bin/env python3
import json, sys, time
initialized = False
for line in sys.stdin:
    msg = json.loads(line)
    if msg.get("method") == "initialize":
        print(json.dumps({"jsonrpc":"2.0", "id":msg["id"], "result":{"serverInfo":{"name":"mock"}}}), flush=True)
    elif msg.get("method") == "notifications/initialized":
        initialized = True
    elif msg.get("method") == "tools/call":
        assert initialized, "tools/call arrived before notifications/initialized"
        tool = msg["params"]["name"]
        args = msg["params"]["arguments"]
        assert tool in ("recall", "forget"), f"unexpected tool: {tool}"
        if tool == "forget":
            assert args == {"memory_id":"canary-memory-17"}, f"forget arguments changed: {args}"
        if args.get("mode") == "delay":
            time.sleep(6)
        if args.get("mode") == "eof":
            break
        if args.get("mode") == "json-rpc-error":
            print(json.dumps({"jsonrpc":"2.0", "id":msg["id"], "error":{"code":-32602, "message":"invalid params"}}), flush=True)
            continue
        result = {"content":[{"type":"text", "text":tool + ":" + json.dumps(args)}]}
        if args.get("mode") == "tool-error":
            result["isError"] = True
        print(json.dumps({"jsonrpc":"2.0", "id":msg["id"], "result":result}), flush=True)
'''


class GrokMcpExampleTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="wenlan-grok-doc-")
        self.addCleanup(self.temp.cleanup)
        self.home = Path(self.temp.name)
        binary = self.home / ".wenlan/bin/wenlan-mcp"
        binary.parent.mkdir(parents=True)
        binary.write_text(MOCK_MCP, encoding="utf-8")
        binary.chmod(0o755)
        self.env = os.environ.copy()
        self.env["HOME"] = str(self.home)

    def call(self, arguments, *, tool="recall", timeout=None):
        script = mcp_function()
        if timeout is not None:
            script = script.replace("RESPONSE_TIMEOUT = 60", f"RESPONSE_TIMEOUT = {timeout}")
        return subprocess.run(
            ["bash", "-c", script + "\nwenlan_mcp_call " + tool + " '" + arguments + "'"],
            cwd=ROOT,
            env=self.env,
            text=True,
            capture_output=True,
            timeout=15,
        )

    def test_waits_for_delayed_tool_reply_after_initialized(self):
        result = self.call('{"mode":"delay"}')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            json.loads(result.stdout)["content"][0]["text"],
            'recall:{"mode": "delay"}',
        )

    def test_forget_name_and_memory_id_are_sent_unchanged(self):
        result = self.call('{"memory_id":"canary-memory-17"}', tool="forget")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            json.loads(result.stdout)["content"][0]["text"],
            'forget:{"memory_id": "canary-memory-17"}',
        )

    def test_rejects_tool_errors(self):
        result = self.call('{"mode":"tool-error"}')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("tool call failed", result.stderr)

    def test_rejects_json_rpc_errors(self):
        result = self.call('{"mode":"json-rpc-error"}')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("JSON-RPC error", result.stderr)

    def test_rejects_early_server_exit(self):
        result = self.call('{"mode":"eof"}')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("closed", result.stderr)

    def test_response_timeout_cleans_up_child(self):
        result = self.call('{"mode":"delay"}', timeout="0.1")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("timed out", result.stderr)


if __name__ == "__main__":
    unittest.main()
