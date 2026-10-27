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
  installBinaries,
  readStamp,
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
  const calls = { latest: 0, download: [], extract: 0, log: [] };
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
      extract: (_archive, into) => {
        calls.extract += 1;
        if (extractBody) return extractBody(into);
        for (const name of BINARIES) {
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

  test("anything but macOS Apple Silicon is refused before touching disk", async () => {
    const h = harness();
    h.opts.platform = "linux";
    await assert.rejects(installBinaries(h.opts), /macOS Apple Silicon only/);
    assert.equal(fs.existsSync(dir), false);
    assert.equal(h.calls.latest, 0);
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
