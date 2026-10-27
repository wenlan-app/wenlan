#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0

const childProcess = require("child_process");
const crypto = require("crypto");
const fs = require("fs");
const https = require("https");
const os = require("os");
const path = require("path");
const { pipeline } = require("stream/promises");

const REPO = "wenlan-app/wenlan";
const ASSET = "wenlan-darwin-arm64.tar.gz";
const REQUESTED_TAG =
  process.env.WENLAN_RELEASE_TAG ||
  process.env.WENLAN_TAG ||
  process.env.ORIGIN_RELEASE_TAG ||
  process.env.ORIGIN_TAG ||
  "";
const BINARIES = ["wenlan", "wenlan-server", "wenlan-mcp"];
// Written into the bin dir only after a COMPLETE install. A run reuses the
// installed binaries when this stamp names the wanted tag and every binary
// still has the size it was extracted with, instead of re-downloading ~60 MB
// on every `npx wenlan` call.
const STAMP_NAME = ".wenlan-install.json";
// Each install extracts into its own `<STAGING_PREFIX><pid>-<random>` directory
// INSIDE the install dir (same filesystem, so the final rename is atomic) and
// moves the binaries into place only once the archive is fully extracted.
const STAGING_PREFIX = ".install-";
// A staging directory older than this was left by a process that was killed
// mid-install (a live install finishes in seconds to minutes).
const STALE_STAGING_MS = 24 * 60 * 60 * 1000;

const RELEASE_TAG_PATTERN = /^v\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/;

function safeTag(tag) {
  return tag.replace(/[^A-Za-z0-9._-]/g, "_");
}

function checkTag(tag) {
  if (!RELEASE_TAG_PATTERN.test(tag)) {
    throw new Error(
      `'${tag}' is not a Wenlan release tag (expected vX.Y.Z, like v0.17.0). ` +
        `See https://github.com/${REPO}/releases.`
    );
  }
  return tag;
}

function binDir() {
  if (REQUESTED_TAG) {
    return path.join(os.homedir(), ".wenlan", "releases", safeTag(REQUESTED_TAG));
  }
  return path.join(os.homedir(), ".wenlan", "bin");
}

// Bound DNS/connect/headers, inactive sockets, and the full redirect/download
// chain separately: slow but progressing downloads can take up to five minutes.
const REQUEST_IDLE_MS = 30_000;
const REQUEST_TOTAL_MS = 5 * 60_000;
const MAX_REDIRECTS = 10;

function getResponse(url, headers, options) {
  const { get = https.get, idleMs = REQUEST_IDLE_MS, deadline } = options;
  return new Promise((resolve, reject) => {
    let req;
    let response;
    let headersTimer;
    let totalTimer;
    const clearTimers = () => {
      clearTimeout(headersTimer);
      clearTimeout(totalTimer);
    };
    const fail = (error) => {
      clearTimers();
      reject(error);
      response?.destroy(error);
      req?.destroy(error);
    };
    const remaining = deadline - Date.now();
    if (remaining <= 0) {
      reject(new Error("Wenlan download timed out. Check your connection and retry."));
      return;
    }
    const timeout = () => fail(new Error("Wenlan download timed out. Check your connection and retry."));
    headersTimer = setTimeout(timeout, Math.min(idleMs, remaining));
    totalTimer = setTimeout(timeout, remaining);
    try {
      req = get(url, { headers: { "User-Agent": "wenlan", ...headers } }, (res) => {
        response = res;
        clearTimeout(headersTimer);
        res.once("end", clearTimers);
        res.once("close", clearTimers);
        // Keep an error listener even when a redirect body is being discarded.
        // pipeline also observes this error for the actual archive response.
        res.once("error", clearTimers);
        resolve(res);
      });
      req.once("error", (error) => {
        clearTimers();
        reject(error);
        response?.destroy(error);
      });
      req.setTimeout(idleMs, timeout);
    } catch (error) {
      fail(error);
    }
  });
}

// The public page avoids the low anonymous GitHub API rate limit.
async function latestTag(options = {}) {
  const url = `https://github.com/${REPO}/releases/latest`;
  const tagPage = `https://github.com/${REPO}/releases/tag/`;
  const res = await getResponse(url, {}, {
    ...options,
    deadline: Date.now() + (options.totalMs ?? REQUEST_TOTAL_MS),
  });
  res.destroy();
  const redirected = res.statusCode >= 300 && res.statusCode < 400;
  const location = (res.headers.location || "").split(/[?#]/)[0];
  if (!redirected || !location.startsWith(tagPage)) {
    throw new Error(
      `Could not find the latest Wenlan release at ${url} (HTTP ${res.statusCode}). ` +
        "Check the network, or pin one with WENLAN_RELEASE_TAG=vX.Y.Z."
    );
  }
  return checkTag(decodeURIComponent(location.slice(tagPage.length)));
}

async function request(url, headers = {}, options = {}) {
  const deadline = Date.now() + (options.totalMs ?? REQUEST_TOTAL_MS);
  const maxRedirects = options.maxRedirects ?? MAX_REDIRECTS;
  let current = new URL(url);
  for (let redirects = 0; ; redirects += 1) {
    if (current.protocol !== "https:") {
      throw new Error("Wenlan downloads require HTTPS.");
    }
    const res = await getResponse(current.href, headers, { ...options, deadline });
    if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
      res.destroy();
      if (redirects >= maxRedirects) throw new Error("Too many Wenlan download redirects.");
      current = new URL(res.headers.location, current);
      continue;
    }
    if (res.statusCode < 200 || res.statusCode >= 300) {
      res.destroy();
      throw new Error(`HTTP ${res.statusCode} for ${current.href}`);
    }
    return res;
  }
}

async function download(url, dest) {
  const res = await request(url);
  await pipeline(res, fs.createWriteStream(dest));
}

function extractBinaries(archivePath, dir) {
  const result = childProcess.spawnSync(
    "tar",
    ["-xzf", archivePath, "-C", dir, ...BINARIES],
    { stdio: "inherit" }
  );
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`tar exited with status ${result.status}`);
}

function run(command, args) {
  const result = childProcess.spawnSync(command, args, { stdio: "inherit" });
  if (result.error) throw result.error;
  process.exitCode = result.status || 0;
  if (process.exitCode !== 0) {
    process.exit(process.exitCode);
  }
}

function printPathHint(dir) {
  const entries = (process.env.PATH || "").split(path.delimiter);
  if (entries.includes(dir)) return;

  process.stderr.write(
    `\nWenlan binaries are installed in ${dir}.\n` +
      `Add them to your shell PATH if you want to run wenlan directly:\n\n` +
      `  export PATH="${dir}:$PATH"\n\n`
  );
}

function stampPath(dir) {
  return path.join(dir, STAMP_NAME);
}

function readStamp(dir) {
  try {
    const stamp = JSON.parse(fs.readFileSync(stampPath(dir), "utf8"));
    return stamp && typeof stamp === "object" ? stamp : null;
  } catch (_) {
    return null;
  }
}

// null when the install in `dir` is a complete `tag` install that can be
// reused as-is; otherwise the reason it cannot (for tests and diagnostics).
function cacheProblem(dir, tag) {
  const stamp = readStamp(dir);
  if (!stamp) return "no install stamp";
  if (stamp.tag !== tag) return `installed ${stamp.tag}, wanted ${tag}`;
  if (stamp.asset !== ASSET) return `installed asset ${stamp.asset}, wanted ${ASSET}`;
  for (const name of BINARIES) {
    const expected = stamp.sizes && stamp.sizes[name];
    let stat;
    try {
      stat = fs.statSync(path.join(dir, name));
    } catch (_) {
      return `${name} is missing`;
    }
    if (!stat.isFile()) return `${name} is not a file`;
    if (!Number.isInteger(expected) || expected <= 0 || stat.size !== expected) {
      return `${name} is not the size it was installed with`;
    }
    if (process.platform !== "win32" && (stat.mode & 0o111) === 0) {
      return `${name} is not executable`;
    }
  }
  return null;
}

function sizesOf(dir) {
  const sizes = {};
  for (const name of BINARIES) {
    sizes[name] = fs.statSync(path.join(dir, name)).size;
  }
  return sizes;
}

// `sizes` describe the binaries THIS install extracted, not whatever is in
// `dir` by now. If a concurrent install of another release interleaved its
// own files, the next run sees a size that does not match and reinstalls,
// instead of the stamp vouching for a mixture.
function writeStamp(dir, tag, sizes) {
  const finalPath = stampPath(dir);
  const tmpPath = `${finalPath}.${process.pid}.tmp`;
  fs.writeFileSync(tmpPath, JSON.stringify({ tag, asset: ASSET, sizes }) + "\n");
  fs.renameSync(tmpPath, finalPath);
}

function removeStamp(dir) {
  try {
    fs.unlinkSync(stampPath(dir));
  } catch (_) {
    // Already absent.
  }
}

function removeTree(target) {
  try {
    fs.rmSync(target, { recursive: true, force: true });
  } catch (_) {
    // Best-effort cleanup only.
  }
}

function sweepStaleStaging(dir, now = Date.now()) {
  let entries;
  try {
    entries = fs.readdirSync(dir);
  } catch (_) {
    return;
  }
  for (const entry of entries) {
    if (!entry.startsWith(STAGING_PREFIX)) continue;
    const staged = path.join(dir, entry);
    try {
      if (now - fs.statSync(staged).mtimeMs > STALE_STAGING_MS) removeTree(staged);
    } catch (_) {
      // Vanished (finished or swept by another process) between list and stat.
    }
  }
}

// Every effect is injectable so the cache logic is testable without the
// network, a real release, or the real home directory.
async function installBinaries(overrides = {}) {
  const opts = {
    requestedTag: REQUESTED_TAG,
    dir: binDir(),
    platform: process.platform,
    arch: process.arch,
    archiveDir: os.tmpdir(),
    fetchLatestTag: latestTag,
    downloadFile: download,
    extract: extractBinaries,
    rename: fs.renameSync,
    log: (message) => process.stderr.write(message),
    ...overrides,
  };

  if (opts.platform !== "darwin" || opts.arch !== "arm64") {
    throw new Error("Wenlan setup currently supports macOS Apple Silicon only.");
  }

  const dir = opts.dir;
  let tag;
  if (opts.requestedTag) {
    tag = checkTag(opts.requestedTag);
  } else {
    try {
      tag = await opts.fetchLatestTag();
    } catch (err) {
      // Offline (or GitHub unreachable) with a complete install already on
      // disk: run what is installed rather than failing a command that needs
      // no network.
      const installed = readStamp(dir);
      if (installed && installed.tag && cacheProblem(dir, installed.tag) === null) {
        opts.log(
          `Could not check for a newer Wenlan release (${err.message}); ` +
            `using the installed ${installed.tag}.\n`
        );
        return dir;
      }
      throw err;
    }
  }

  if (cacheProblem(dir, tag) === null) {
    return dir;
  }

  fs.mkdirSync(dir, { recursive: true });
  sweepStaleStaging(dir);

  // Several `npx wenlan` calls can start at once (a first run from two
  // terminals, an editor and a shell). Nothing here is shared between them
  // until the final renames: each has its own archive file and its own staging
  // directory, so one process's cleanup can never delete or overwrite what
  // another is still reading.
  const archivePath = path.join(
    opts.archiveDir,
    `${ASSET}.${process.pid}-${crypto.randomBytes(4).toString("hex")}`
  );
  const staging = fs.mkdtempSync(path.join(dir, `${STAGING_PREFIX}${process.pid}-`));
  const url = `https://github.com/${REPO}/releases/download/${tag}/${ASSET}`;
  opts.log(`Downloading Wenlan ${tag}...\n`);

  try {
    try {
      await opts.downloadFile(url, archivePath);
    } catch (err) {
      throw new Error(
        `${err.message}. Check that the release exists at https://github.com/${REPO}/releases/tag/${tag}`
      );
    }
    opts.extract(archivePath, staging);
    for (const name of BINARIES) {
      fs.chmodSync(path.join(staging, name), 0o755);
    }
    const sizes = sizesOf(staging);

    // Another process may have finished this same install while this one was
    // downloading. Use theirs rather than replacing binaries a caller may be
    // running right now.
    if (cacheProblem(dir, tag) === null) {
      return dir;
    }

    // No stamp may survive into a half-placed install: if the renames below
    // are interrupted, the next run must see "no install" and start over.
    removeStamp(dir);
    for (const name of BINARIES) {
      opts.rename(path.join(staging, name), path.join(dir, name));
    }
    writeStamp(dir, tag, sizes);
    return dir;
  } catch (err) {
    // Losing a race is not a failure when the winner left a complete install.
    if (cacheProblem(dir, tag) === null) {
      return dir;
    }
    throw err;
  } finally {
    try {
      fs.unlinkSync(archivePath);
    } catch (_) {
      // Never created, or already gone.
    }
    removeTree(staging);
  }
}

async function main() {
  const args = process.argv.slice(2);
  const dir = await installBinaries();
  const wenlan = path.join(dir, "wenlan");

  if (args[0] === "setup") {
    const setupArgs = args.slice(1);
    run(wenlan, ["setup", ...(setupArgs.length ? setupArgs : ["--basic"])]);
    run(wenlan, ["background", "on"]);
    run(wenlan, ["status", "--format", "table"]);
    printPathHint(dir);
    return;
  }

  run(wenlan, args.length ? args : ["--help"]);
}

if (require.main === module) {
  main().catch((err) => {
    console.error(`wenlan setup failed: ${err.message}`);
    process.exit(1);
  });
}

module.exports = {
  ASSET,
  BINARIES,
  STAGING_PREFIX,
  STAMP_NAME,
  cacheProblem,
  checkTag,
  extractBinaries,
  installBinaries,
  latestTag,
  request,
  readStamp,
  safeTag,
};
