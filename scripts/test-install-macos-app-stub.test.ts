// @vitest-environment node
// SPDX-License-Identifier: AGPL-3.0-only
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, resolve } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { resolveTestBash } from "./lib/test-bash";

const root = resolve(import.meta.dirname, "..");
const harness = readFileSync(resolve(root, "scripts/test-install-macos-app.sh"), "utf8");
const main = harness.indexOf("\ntest_installs_verified_app_without_quarantine\n");
if (main < 0) throw new Error("cannot separate installer test functions from their main calls");
const definitions = harness.slice(0, main);
const bash = resolveTestBash();
const temporaryRoots: string[] = [];
const quote = (value: string) => `'${value.replaceAll("'", "'\\''")}'`;

afterEach(() => {
  for (const folder of temporaryRoots.splice(0)) rmSync(folder, { recursive: true, force: true });
});

function runFixture(injection: string, mutateDns = false) {
  const directory = mkdtempSync(resolve(tmpdir(), "wenlan-installer-stub-"));
  temporaryRoots.push(directory);
  const bin = resolve(directory, "bin");
  mkdirSync(bin);
  const python = spawnSync(process.env.WENLAN_TEST_PYTHON ?? "python3", ["-c", "import sys; print(sys.executable)"], { encoding: "utf8" });
  if (python.status !== 0 || !python.stdout.trim()) throw new Error(`Python unavailable: ${python.stderr}`);
  writeFileSync(resolve(bin, "python3"), `#!/usr/bin/env bash\nexec ${quote(python.stdout.trim().replaceAll("\\", "/"))} "$@"\n`, { mode: 0o755 });
  writeFileSync(resolve(directory, "sitecustomize.py"), injection);
  let source = definitions;
  if (mutateDns) {
    const guarded = 'server = LoopbackHTTPServer(("127.0.0.1", 0), Handler)';
    if (!source.includes(guarded)) throw new Error("loopback server mutation anchor is missing");
    source = source.replace(guarded, 'server = http.server.HTTPServer(("127.0.0.1", 0), Handler)');
  }
  const driver = resolve(directory, "fixture.sh");
  writeFileSync(driver, `${source}\nINSTALLER=${quote(resolve(root, "scripts/install-macos-app.sh").replaceAll("\\", "/"))}\ntest_names_rate_limiting_without_sending_the_token_elsewhere\n`);
  const env: NodeJS.ProcessEnv = {
    ...process.env,
    PATH: `${bin}${delimiter}${process.env.PATH ?? ""}`,
    PYTHONPATH: directory,
    TMPDIR: directory.replaceAll("\\", "/"),
    WENLAN_NO_AUTOSTART: "1",
    NO_PROXY: "127.0.0.1,localhost",
    no_proxy: "127.0.0.1,localhost",
  };
  delete env.GH_TOKEN;
  delete env.GITHUB_TOKEN;
  return spawnSync(bash, [driver], { cwd: root, env, encoding: "utf8", timeout: 40_000 });
}

const rejectDns = `import socket
def reject_lookup(*args):
    raise RuntimeError("unexpected reverse DNS in loopback fixture")
socket.getfqdn = reject_lookup
`;

describe("macOS installer loopback fixture startup", () => {
  it("serves the real rate-limit/token case without reverse DNS and rejects the old server", () => {
    const working = runFixture(rejectDns);
    expect(working.error).toBeUndefined();
    expect(working.status, working.stderr).toBe(0);
    const old = runFixture(rejectDns, true);
    expect(old.status).not.toBe(0);
    expect(old.stderr).toContain("unexpected reverse DNS in loopback fixture");
    expect(old.stderr).toContain("stub exited before publishing its port");
  }, 40_000);

  it("waits for readiness when startup exceeds the old five-second window", () => {
    const result = runFixture(`import socketserver, time
original = socketserver.TCPServer.server_bind
def delayed_bind(self):
    time.sleep(6)
    return original(self)
socketserver.TCPServer.server_bind = delayed_bind
`);
    expect(result.error).toBeUndefined();
    expect(result.status, result.stderr).toBe(0);
  }, 40_000);

  it("reports an actual child startup failure with its diagnostic", () => {
    const result = runFixture(`import socketserver
def failed_bind(self):
    raise RuntimeError("injected bind failure")
socketserver.TCPServer.server_bind = failed_bind
`);
    expect(result.error).toBeUndefined();
    expect(result.status).not.toBe(0);
    expect(result.stderr).toContain("stub exited before publishing its port (status 1)");
    expect(result.stderr).toContain("injected bind failure");
    expect(result.stderr).not.toContain("timed out waiting");
  }, 40_000);
});
