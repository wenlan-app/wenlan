// SPDX-License-Identifier: Apache-2.0
// Run with: node --test crates/wenlan-cli/npm/run.test.js
//
// Not shipped: package.json's `files` allowlist is run.js, README.md, LICENSE.
// Every effect of installBinaries (network, tar, home directory) is injected,
// so these tests use a temp directory and never touch ~/.wenlan.

const assert = require("node:assert/strict");
const childProcess = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { afterEach, beforeEach, describe, test } = require("node:test");

const {
  ASSET,
  BINARIES,
  STAGING_PREFIX,
  cacheProblem,
  extractBinaries,
  hasUserServiceManager,
  installBinaries,
  pathHint,
  startForThisSession,
  place,
  readStamp,
  tarCommand,
  targetFor,
  verifyChecksum,
} = require("./run.js");

let root;
let dir;
let archiveDir;

beforeEach(() => {
  root = fs.mkdtempSync(path.join(os.tmpdir(), "wenlan-run-test-"));
  dir = path.join(root, "bin");
  archiveDir = path.join(root, "tmp");
  fs.mkdirSync(archiveDir);
});

afterEach(() => {
  fs.rmSync(root, { recursive: true, force: true });
});

// Records every effect so a test can assert what did and did not happen.
function harness({ latest = "v1.0.0", requestedTag = "", extractBody } = {}) {
  const calls = { latest: 0, download: [], verify: [], extract: 0, log: [] };
  return {
    calls,
    opts: {
      requestedTag,
      dir,
      archiveDir,
      platform: "darwin",
      arch: "arm64",
      log: (message) => calls.log.push(message),
      fetchLatestTag: async () => {
        calls.latest += 1;
        if (latest instanceof Error) throw latest;
        return latest;
      },
      downloadFile: async (url, dest) => {
        calls.download.push(url);
        fs.writeFileSync(dest, "archive");
      },
      verify: async (_archive, tag, asset) => {
        calls.verify.push(`${tag}/${asset}`);
      },
      extract: (_archive, into, target) => {
        calls.extract += 1;
        if (extractBody) return extractBody(into, target);
        for (const name of target.binaries) {
          fs.writeFileSync(path.join(into, name), `#!/bin/sh\n# ${name}\n`);
        }
      },
    },
  };
}

// Anything in the install dir or the archive dir that is not an installed
// binary or the stamp: a staging directory or a downloaded archive.
function leftovers() {
  const installed = new Set([...BINARIES, ".wenlan-install.json"]);
  return [
    ...fs.readdirSync(dir).filter((name) => !installed.has(name)),
    ...fs.readdirSync(archiveDir),
  ];
}

function deferred() {
  let resolve;
  const promise = new Promise((r) => {
    resolve = r;
  });
  return { promise, resolve };
}

describe("installBinaries version cache", () => {
  test("fresh install downloads once, announces it, and stamps the tag", async () => {
    const h = harness();
    const result = await installBinaries(h.opts);

    assert.equal(result, dir);
    assert.deepEqual(h.calls.download, [
      `https://github.com/wenlan-app/wenlan/releases/download/v1.0.0/${ASSET}`,
    ]);
    assert.equal(h.calls.extract, 1);
    assert.deepEqual(h.calls.log, ["Downloading Wenlan v1.0.0...\n"]);
    const stamp = readStamp(dir);
    assert.equal(stamp.tag, "v1.0.0");
    assert.equal(stamp.asset, ASSET);
    assert.deepEqual(Object.keys(stamp.sizes).sort(), [...BINARIES].sort());
    assert.equal(cacheProblem(dir, "v1.0.0"), null);
    for (const name of BINARIES) {
      assert.ok(fs.statSync(path.join(dir, name)).mode & 0o111, `${name} executable`);
    }
  });

  test("second run of the same tag reuses the binaries with no download", async () => {
    await installBinaries(harness().opts);

    const again = harness();
    const result = await installBinaries(again.opts);

    assert.equal(result, dir);
    assert.deepEqual(again.calls.download, []);
    assert.equal(again.calls.extract, 0);
    assert.deepEqual(again.calls.log, [], "a cache hit is silent");
  });

  test("a newer release re-downloads over the stamped older one", async () => {
    await installBinaries(harness({ latest: "v1.0.0" }).opts);

    const upgrade = harness({ latest: "v1.1.0" });
    await installBinaries(upgrade.opts);

    assert.equal(upgrade.calls.download.length, 1);
    assert.match(upgrade.calls.download[0], /\/v1\.1\.0\//);
    assert.equal(readStamp(dir).tag, "v1.1.0");
  });

  test("a truncated binary is not trusted and is re-downloaded", async () => {
    await installBinaries(harness().opts);
    fs.writeFileSync(path.join(dir, "wenlan-server"), "");

    assert.match(cacheProblem(dir, "v1.0.0"), /wenlan-server/);
    const repair = harness();
    await installBinaries(repair.opts);
    assert.equal(repair.calls.download.length, 1);
    assert.equal(cacheProblem(dir, "v1.0.0"), null);
  });

  test("a missing binary is re-downloaded", async () => {
    await installBinaries(harness().opts);
    fs.unlinkSync(path.join(dir, "wenlan-mcp"));

    const repair = harness();
    await installBinaries(repair.opts);
    assert.equal(repair.calls.download.length, 1);
  });

  test("a non-executable binary is re-downloaded", async () => {
    await installBinaries(harness().opts);
    fs.chmodSync(path.join(dir, "wenlan"), 0o644);

    assert.match(cacheProblem(dir, "v1.0.0"), /not executable/);
  });

  test("binaries with no stamp (older runner, other installer) are re-downloaded once", async () => {
    fs.mkdirSync(dir, { recursive: true });
    for (const name of BINARIES) fs.writeFileSync(path.join(dir, name), "old");

    const h = harness();
    await installBinaries(h.opts);
    assert.equal(h.calls.download.length, 1);
    assert.equal(readStamp(dir).tag, "v1.0.0");
  });

  test("an interrupted extraction leaves the previous install untouched and nothing behind", async () => {
    await installBinaries(harness().opts);
    const before = fs.readFileSync(path.join(dir, "wenlan"), "utf8");

    const broken = harness({
      latest: "v1.1.0",
      extractBody: (into) => {
        fs.writeFileSync(path.join(into, "wenlan"), "partial");
        throw new Error("tar exited with status 1");
      },
    });
    await assert.rejects(installBinaries(broken.opts), /tar exited/);

    // Extraction happens in a staging directory, so the v1.0.0 files are not
    // half-overwritten and the v1.0.0 stamp still describes them truthfully.
    assert.equal(readStamp(dir).tag, "v1.0.0");
    assert.equal(cacheProblem(dir, "v1.0.0"), null);
    assert.equal(fs.readFileSync(path.join(dir, "wenlan"), "utf8"), before);
    assert.deepEqual(leftovers(), [], "no staging directory or archive survives");

    const next = harness({ latest: "v1.1.0" });
    await installBinaries(next.opts);
    assert.equal(next.calls.download.length, 1);
    assert.equal(readStamp(dir).tag, "v1.1.0");
  });

  test("a placement that dies halfway leaves no stamp, so the next run starts over", async () => {
    await installBinaries(harness().opts);

    const broken = harness({ latest: "v1.1.0" });
    let placed = 0;
    broken.opts.rename = (from, to) => {
      placed += 1;
      if (placed === 2) throw new Error("EIO: simulated crash between renames");
      fs.renameSync(from, to);
    };
    await assert.rejects(installBinaries(broken.opts), /simulated crash/);

    assert.equal(placed, 2);
    assert.equal(readStamp(dir), null, "a stale v1.0.0 stamp must not vouch for a mix of v1.0.0 and v1.1.0 binaries");
    assert.deepEqual(leftovers(), []);

    const next = harness({ latest: "v1.1.0" });
    await installBinaries(next.opts);
    assert.equal(next.calls.download.length, 1);
    assert.equal(cacheProblem(dir, "v1.1.0"), null);
  });

  test("a pinned tag never asks GitHub for the latest release", async () => {
    const pinned = harness({ requestedTag: "v0.9.0", latest: new Error("must not be called") });
    await installBinaries(pinned.opts);
    assert.equal(pinned.calls.latest, 0);
    assert.match(pinned.calls.download[0], /\/v0\.9\.0\//);

    const again = harness({ requestedTag: "v0.9.0", latest: new Error("must not be called") });
    await installBinaries(again.opts);
    assert.equal(again.calls.latest, 0);
    assert.deepEqual(again.calls.download, []);
  });

  test("offline with a complete install runs the installed version and says so", async () => {
    await installBinaries(harness({ latest: "v1.0.0" }).opts);

    const offline = harness({ latest: new Error("getaddrinfo ENOTFOUND github.com") });
    const result = await installBinaries(offline.opts);

    assert.equal(result, dir);
    assert.deepEqual(offline.calls.download, []);
    assert.equal(offline.calls.log.length, 1);
    assert.match(offline.calls.log[0], /ENOTFOUND/);
    assert.match(offline.calls.log[0], /v1\.0\.0/);
  });

  test("offline with nothing installed still fails with the network error", async () => {
    const offline = harness({ latest: new Error("getaddrinfo ENOTFOUND github.com") });
    await assert.rejects(installBinaries(offline.opts), /ENOTFOUND/);
    assert.deepEqual(offline.calls.download, []);
  });

  test("offline with a damaged install does not pretend it is usable", async () => {
    await installBinaries(harness().opts);
    fs.writeFileSync(path.join(dir, "wenlan"), "");

    const offline = harness({ latest: new Error("getaddrinfo ENOTFOUND github.com") });
    await assert.rejects(installBinaries(offline.opts), /ENOTFOUND/);
  });

  test("a failed download keeps its explanation and leaves no stamp", async () => {
    const h = harness();
    h.opts.downloadFile = async () => {
      throw new Error("HTTP 404 for https://example.invalid");
    };
    await assert.rejects(installBinaries(h.opts), /HTTP 404.*Check that the release exists/);
    assert.equal(readStamp(dir), null);
  });

  test("a computer with no build is refused before touching disk, and told what works", async () => {
    const h = harness();
    h.opts.platform = "darwin";
    h.opts.arch = "x64";
    await assert.rejects(
      installBinaries(h.opts),
      /no build for this computer \(darwin x64\).*Apple Silicon.*Linux.*Windows/
    );
    assert.equal(fs.existsSync(dir), false);
    assert.equal(h.calls.latest, 0);
  });

  test("a download that fails its checksum is never unpacked or stamped", async () => {
    const h = harness();
    h.opts.verify = async () => {
      throw new Error("does not match its checksum");
    };
    await assert.rejects(installBinaries(h.opts), /does not match its checksum/);
    assert.equal(h.calls.extract, 0);
    assert.equal(readStamp(dir), null);
    assert.deepEqual(leftovers(), []);
  });
});

describe("installBinaries on each supported computer", () => {
  for (const [platform, arch, asset] of [
    ["darwin", "arm64", "wenlan-darwin-arm64.tar.gz"],
    ["linux", "x64", "wenlan-linux-x64.tar.gz"],
    ["linux", "arm64", "wenlan-linux-arm64.tar.gz"],
    ["win32", "x64", "wenlan-windows-x64.zip"],
  ]) {
    test(`${platform} ${arch} downloads and verifies ${asset}`, async () => {
      const h = harness();
      h.opts.platform = platform;
      h.opts.arch = arch;
      await installBinaries(h.opts);

      assert.deepEqual(h.calls.download, [
        `https://github.com/wenlan-app/wenlan/releases/download/v1.0.0/${asset}`,
      ]);
      assert.deepEqual(h.calls.verify, [`v1.0.0/${asset}`]);
      assert.equal(readStamp(dir).asset, asset);
      const target = targetFor(platform, arch);
      assert.equal(cacheProblem(dir, "v1.0.0", target), null);
      for (const name of target.binaries) assert.ok(fs.existsSync(path.join(dir, name)), name);
    });
  }

  test("Windows installs the .exe programs and every DLL shipped beside them", async () => {
    const target = targetFor("win32", "x64");
    assert.deepEqual(target.binaries, ["wenlan.exe", "wenlan-server.exe", "wenlan-mcp.exe"]);
    const h = harness({
      extractBody: (into) => {
        for (const name of [...target.binaries, "onnxruntime.dll", "vcruntime140.dll"]) {
          fs.writeFileSync(path.join(into, name), `file ${name}`);
        }
      },
    });
    h.opts.platform = "win32";
    h.opts.arch = "x64";
    await installBinaries(h.opts);

    assert.deepEqual(
      fs.readdirSync(dir).sort(),
      [".wenlan-install.json", "onnxruntime.dll", "vcruntime140.dll", ...target.binaries].sort()
    );
    assert.deepEqual(Object.keys(readStamp(dir).sizes).sort(), [...target.binaries].sort());
  });

  test("an install for another computer in the same folder is not reused", async () => {
    await installBinaries(harness().opts);
    assert.match(
      cacheProblem(dir, "v1.0.0", targetFor("linux", "x64")),
      /installed asset wenlan-darwin-arm64/
    );
  });
});

describe("place", () => {
  test("a file the system will not replace is moved aside, and the new one takes its name", () => {
    fs.mkdirSync(dir);
    const from = path.join(root, "new.exe");
    const to = path.join(dir, "wenlan-server.exe");
    fs.writeFileSync(from, "new");
    fs.writeFileSync(to, "running");
    let refused = 0;
    const rename = (a, b) => {
      if (fs.existsSync(b)) {
        refused += 1;
        throw Object.assign(new Error("in use"), { code: "EPERM" });
      }
      fs.renameSync(a, b);
    };

    place(from, to, rename);

    assert.equal(refused, 1);
    assert.equal(fs.readFileSync(to, "utf8"), "new");
    const aside = fs.readdirSync(dir).filter((name) => name.startsWith(".old-"));
    assert.equal(aside.length, 1);
    assert.equal(fs.readFileSync(path.join(dir, aside[0]), "utf8"), "running");
  });

  test("the next install removes what was moved aside", async () => {
    fs.mkdirSync(dir);
    fs.writeFileSync(path.join(dir, ".old-1-abcd-wenlan-server.exe"), "old");
    await installBinaries(harness().opts);
    assert.deepEqual(leftovers(), []);
  });

  test("any other failure is reported as it is", () => {
    const rename = () => {
      throw Object.assign(new Error("disk full"), { code: "ENOSPC" });
    };
    assert.throws(() => place(path.join(root, "a"), path.join(root, "b"), rename), /disk full/);
  });
});

describe("verifyChecksum", () => {
  const crypto = require("node:crypto");
  const sumsFor = (name, body) =>
    `${"0".repeat(64)}  other.tar.gz\n${crypto.createHash("sha256").update(body).digest("hex")}  ${name}\n`;
  const quiet = { skip: false, log: () => {} };

  test("passes when the archive matches its line in SHA256SUMS", async () => {
    const archive = path.join(root, "a.tar.gz");
    fs.writeFileSync(archive, "real archive");
    let asked;
    await verifyChecksum(archive, "v1.0.0", "wenlan-linux-x64.tar.gz", {
      ...quiet,
      fetchSums: async (url) => {
        asked = url;
        return sumsFor("wenlan-linux-x64.tar.gz", "real archive");
      },
    });
    assert.equal(asked, "https://github.com/wenlan-app/wenlan/releases/download/v1.0.0/SHA256SUMS");
  });

  test("refuses an archive that does not match", async () => {
    const archive = path.join(root, "a.tar.gz");
    fs.writeFileSync(archive, "tampered");
    await assert.rejects(
      verifyChecksum(archive, "v1.0.0", "wenlan-linux-x64.tar.gz", {
        ...quiet,
        fetchSums: async () => sumsFor("wenlan-linux-x64.tar.gz", "real archive"),
      }),
      /does not match its checksum/
    );
  });

  test("refuses when SHA256SUMS does not name the archive", async () => {
    const archive = path.join(root, "a.zip");
    fs.writeFileSync(archive, "x");
    await assert.rejects(
      verifyChecksum(archive, "v1.0.0", "wenlan-windows-x64.zip", {
        ...quiet,
        fetchSums: async () => sumsFor("wenlan-linux-x64.tar.gz", "x"),
      }),
      /no entry for wenlan-windows-x64\.zip/
    );
  });

  test("a missing SHA256SUMS stops the install unless the person opted out", async () => {
    const archive = path.join(root, "a.tar.gz");
    fs.writeFileSync(archive, "x");
    const fetchSums = async () => {
      throw new Error("HTTP 404");
    };
    await assert.rejects(
      verifyChecksum(archive, "v1.0.0", "wenlan-linux-x64.tar.gz", { ...quiet, fetchSums }),
      /cannot be verified/
    );
    const said = [];
    await verifyChecksum(archive, "v1.0.0", "wenlan-linux-x64.tar.gz", {
      fetchSums,
      skip: true,
      log: (message) => said.push(message),
    });
    assert.match(said.join(""), /installing unverified/);
  });
});

describe("a Linux system with no systemd user session", () => {
  test("only Linux is probed, and a failing or missing systemctl means no session", () => {
    const never = () => {
      throw new Error("not probed");
    };
    assert.equal(hasUserServiceManager("darwin", never), true);
    assert.equal(hasUserServiceManager("win32", never), true);
    assert.equal(hasUserServiceManager("linux", () => ({ status: 0 })), true);
    assert.equal(hasUserServiceManager("linux", () => ({ status: 1 })), false);
    assert.equal(hasUserServiceManager("linux", () => ({ error: new Error("ENOENT") })), false);
  });

  test("the daemon is started once and the person is told it will not come back by itself", async () => {
    fs.mkdirSync(dir);
    const spawned = [];
    const said = [];
    let checks = 0;
    await startForThisSession(dir, {
      spawn: (program, args, options) => {
        spawned.push({ program, detached: options.detached });
        return { unref() {} };
      },
      healthy: () => (checks += 1) > 1,
      waitMs: 5000,
      log: (message) => said.push(message),
    });
    assert.deepEqual(spawned, [{ program: path.join(dir, "wenlan-server"), detached: true }]);
    assert.match(said.join(""), /After this computer restarts, start it again/);
    assert.match(said.join(""), /wenlan-server &/);
  });

  test("a daemon that is already running is not started twice", async () => {
    fs.mkdirSync(dir);
    let spawned = 0;
    await startForThisSession(dir, {
      spawn: () => {
        spawned += 1;
        return { unref() {} };
      },
      healthy: () => true,
      log: () => {},
    });
    assert.equal(spawned, 0);
  });

  test("a daemon that never answers is reported with where its output went", async () => {
    fs.mkdirSync(dir);
    await assert.rejects(
      startForThisSession(dir, {
        spawn: () => ({ unref() {} }),
        healthy: () => false,
        waitMs: 600,
        log: () => {},
      }),
      /did not start\. Its output is in .*wenlan-server\.log/
    );
  });
});

describe("what the person is told", () => {
  test("no hint when the folder is already on PATH", () => {
    assert.equal(pathHint("/home/a/.wenlan/bin", "linux", { PATH: "/usr/bin:/home/a/.wenlan/bin" }), "");
    assert.equal(
      pathHint("C:\\Users\\a\\.wenlan\\bin", "win32", { Path: "C:\\Windows;c:\\users\\A\\.wenlan\\bin\\" }),
      ""
    );
  });

  test("each system gets a line its own shell understands", () => {
    assert.match(pathHint("/home/a/.wenlan/bin", "linux", { PATH: "/usr/bin" }), /export PATH="\/home\/a\/\.wenlan\/bin:\$PATH"/);
    const windows = pathHint("C:\\Users\\a\\.wenlan\\bin", "win32", { Path: "C:\\Windows" });
    assert.match(windows, /PowerShell/);
    assert.match(windows, /SetEnvironmentVariable/);
    assert.doesNotMatch(windows, /export PATH/);
  });

  test("Windows unpacks with its own tar, not one found on PATH", () => {
    assert.equal(tarCommand("win32", { SystemRoot: "D:\\Win" }), "D:\\Win\\System32\\tar.exe");
    assert.equal(tarCommand("linux", {}), "tar");
  });
});

describe("installBinaries with another install running", () => {
  // Distinct payload per process, and an extract that fails the way the real
  // one does when its archive has been deleted or overwritten underneath it.
  function process_(id, { parked, release } = {}) {
    const h = harness();
    h.archives = [];
    h.opts.downloadFile = async (_url, dest) => {
      h.archives.push(dest);
      fs.writeFileSync(dest, `archive-${id}`);
      if (parked) {
        parked.resolve();
        await release.promise;
      }
    };
    h.opts.extract = (archive, into) => {
      h.calls.extract += 1;
      assert.equal(fs.readFileSync(archive, "utf8"), `archive-${id}`, `${id}'s archive was clobbered`);
      for (const name of BINARIES) {
        fs.writeFileSync(path.join(into, name), `#!/bin/sh\n# ${name} from ${id}\n`);
      }
    };
    return h;
  }

  test("a second install that finishes first does not break the one still in flight", async () => {
    const parked = deferred();
    const release = deferred();
    const slow = process_("slow", { parked, release });
    const fast = process_("fast");

    const slowRun = installBinaries(slow.opts);
    await parked.promise; // `slow` has written its archive and is mid-download
    await installBinaries(fast.opts); // `fast` runs start to finish and cleans up
    release.resolve();
    await slowRun; // must not fail on a deleted or overwritten archive

    assert.notEqual(slow.archives[0], fast.archives[0], "each process downloads to its own archive");
    assert.notEqual(slow.archives[0], path.join(archiveDir, ASSET));
    assert.equal(slow.calls.extract, 1);
    assert.equal(cacheProblem(dir, "v1.0.0"), null);
    // The install `fast` completed is what is on disk: `slow` found it
    // complete and reused it instead of replacing binaries under a caller.
    assert.match(fs.readFileSync(path.join(dir, "wenlan"), "utf8"), /from fast/);
    assert.deepEqual(leftovers(), []);
  });

  // What a second process leaves behind when its install completes: every
  // binary in place, executable, and a stamp that describes them.
  function completeInstallByAnotherProcess(tag, body) {
    fs.mkdirSync(dir, { recursive: true });
    const sizes = {};
    for (const name of BINARIES) {
      const content = body(name);
      fs.writeFileSync(path.join(dir, name), content);
      fs.chmodSync(path.join(dir, name), 0o755);
      sizes[name] = content.length;
    }
    fs.writeFileSync(path.join(dir, ".wenlan-install.json"), JSON.stringify({ tag, asset: ASSET, sizes }));
  }

  test("a stamp never vouches for binaries another install interleaved into the directory", async () => {
    const ours = harness({ latest: "v1.0.0" });
    let renames = 0;
    ours.opts.rename = (from, to) => {
      renames += 1;
      // Between our first and second rename, another process places a
      // different release (longer binaries) over the directory.
      if (renames === 2) completeInstallByAnotherProcess("v1.1.0", (name) => `v1.1.0 ${name} ${"x".repeat(64)}`);
      fs.renameSync(from, to);
    };

    await installBinaries(ours.opts);

    // The directory is now a mix: our wenlan-server and wenlan-mcp, but the
    // other release's wenlan was placed after ours. Our stamp describes OUR
    // sizes, so the mix is detected and the next run repairs it.
    assert.match(cacheProblem(dir, "v1.0.0"), /wenlan is not the size it was installed with/);
    const repair = harness({ latest: "v1.0.0" });
    await installBinaries(repair.opts);
    assert.equal(repair.calls.download.length, 1);
    assert.equal(cacheProblem(dir, "v1.0.0"), null);
  });

  test("losing the race to place files is not a failure when the winner's install is complete", async () => {
    const loser = harness();
    loser.opts.rename = () => {
      completeInstallByAnotherProcess("v1.0.0", (name) => `winner ${name}`);
      throw new Error("EBUSY: simulated");
    };

    const result = await installBinaries(loser.opts);

    assert.equal(result, dir);
    assert.equal(cacheProblem(dir, "v1.0.0"), null);
    assert.equal(fs.readFileSync(path.join(dir, "wenlan"), "utf8"), "winner wenlan");
    assert.deepEqual(leftovers(), []);
  });

  test("a failure with no complete install behind it is still reported", async () => {
    const h = harness();
    h.opts.rename = () => {
      throw new Error("EBUSY: simulated");
    };
    await assert.rejects(installBinaries(h.opts), /EBUSY/);
    assert.equal(readStamp(dir), null);
  });

  test("staging directories left by a killed install are swept once they are old", async () => {
    fs.mkdirSync(dir, { recursive: true });
    const stale = path.join(dir, `${STAGING_PREFIX}999-stale`);
    const live = path.join(dir, `${STAGING_PREFIX}998-live`);
    for (const d of [stale, live]) {
      fs.mkdirSync(d);
      fs.writeFileSync(path.join(d, "wenlan"), "partial");
    }
    const twoDaysAgo = new Date(Date.now() - 48 * 60 * 60 * 1000);
    fs.utimesSync(stale, twoDaysAgo, twoDaysAgo);

    await installBinaries(harness().opts);

    assert.equal(fs.existsSync(stale), false, "an old staging directory is removed");
    assert.equal(fs.existsSync(live), true, "a recent one may belong to a running install");
  });
});

describe("extractBinaries", () => {
  const tar = childProcess.spawnSync("tar", ["--version"]);

  test(
    "extracts exactly the three binaries from a release-shaped archive",
    { skip: tar.error ? "tar is not installed" : false },
    () => {
      const staging = path.join(root, "staging");
      fs.mkdirSync(staging);
      for (const name of BINARIES) fs.writeFileSync(path.join(staging, name), `bin ${name}`);
      fs.writeFileSync(path.join(staging, "README.txt"), "not extracted");
      const archive = path.join(root, ASSET);
      const made = childProcess.spawnSync("tar", ["-czf", archive, "-C", staging, ...BINARIES, "README.txt"]);
      assert.equal(made.status, 0);

      fs.mkdirSync(dir);
      extractBinaries(archive, dir);

      assert.deepEqual(fs.readdirSync(dir).sort(), [...BINARIES].sort());
      assert.equal(fs.readFileSync(path.join(dir, "wenlan"), "utf8"), "bin wenlan");
    }
  );
});

// Real loopback sockets exercise timeout and stream-error propagation without
// reaching GitHub or touching installed binaries. HTTPS policy remains in the
// production request helper; only its transport is injected for this fixture.
const http = require('node:http');
const { Writable } = require('node:stream');
const { pipeline } = require('node:stream/promises');
const { request, latestTag } = require('./run.js');
async function withDownloadServer(handler, verify) {
  const server = http.createServer(handler);
  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolve);
  });
  const address = server.address();
  const get = (url, options, callback) => http.get(
    `http://127.0.0.1:${address.port}${new URL(url).pathname}`,
    options, callback,
  );
  try {
    await verify({ get, idleMs: 75, totalMs: 1000 });
  } finally {
    server.closeAllConnections();
    await new Promise(resolve => server.close(resolve));
  }
}

test('latest lookup and archive request reject a server that never sends headers', async () => {
  await withDownloadServer(() => {}, async options => {
    await assert.rejects(latestTag(options), /timed out/);
    await assert.rejects(request('https://fixture/archive', {}, options), /timed out/);
  });
});

test('stalled archive body rejects the download pipeline', async () => {
  await withDownloadServer((_req, res) => {
    res.writeHead(200);
    res.write('partial archive');
  }, async options => {
    const response = await request('https://fixture/archive', {}, options);
    await assert.rejects(pipeline(response, new Writable({
      write(_chunk, _encoding, done) { done(); },
    })), /timed out/);
  });
});

test('total deadline also bounds a continuously progressing response', async () => {
  await withDownloadServer((_req, res) => {
    res.writeHead(200);
    const timer = setInterval(() => res.write('x'), 10);
    res.once('close', () => clearInterval(timer));
  }, async options => {
    const response = await request('https://fixture/archive', {}, { ...options, idleMs: 200, totalMs: 80 });
    await assert.rejects(pipeline(response, new Writable({
      write(_chunk, _encoding, done) { done(); },
    })), /timed out/);
  });
});

test('relative redirects succeed, loops and insecure redirects fail', async () => {
  await withDownloadServer((req, res) => {
    if (req.url === '/ok') { res.end('archive'); return; }
    res.writeHead(302, { location: req.url === '/start' ? '/ok' : req.url === '/insecure' ? 'http://fixture/ok' : '/loop' });
    res.end();
  }, async options => {
    const response = await request('https://fixture/start', {}, options);
    let body = '';
    for await (const chunk of response) body += chunk;
    assert.equal(body, 'archive');
    await assert.rejects(request('https://fixture/loop', {}, { ...options, maxRedirects: 2 }), /Too many/);
    await assert.rejects(request('https://fixture/insecure', {}, options), /HTTPS/);
  });
});

describe("setup steps", () => {
  const { setupSteps, START_FOR_THIS_SESSION } = require("./run.js");

  test("settings first, then start, then the finish step that needs Wenlan running", () => {
    assert.deepEqual(setupSteps([], true), [
      ["setup", "--basic", "--config-only"],
      ["background", "on"],
      ["status", "--format", "table"],
      ["setup", "--finish"],
    ]);
  });

  test("a system that cannot keep Wenlan running starts it for this session", () => {
    assert.equal(setupSteps([], false)[1], START_FOR_THIS_SESSION);
  });

  test("the person's own setup choice is kept", () => {
    assert.deepEqual(setupSteps(["--model", "qwen3.5-4b"], true)[0], [
      "setup",
      "--model",
      "qwen3.5-4b",
      "--config-only",
    ]);
  });

  test("asking for the settings only connects nothing afterwards", () => {
    assert.deepEqual(setupSteps(["--basic", "--config-only"], true), [
      ["setup", "--basic", "--config-only"],
      ["background", "on"],
      ["status", "--format", "table"],
    ]);
  });
});
