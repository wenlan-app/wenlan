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
// One release archive per supported computer. `exe` is the suffix of the three
// programs; `all` means every file in the archive is installed beside them (the
// Windows zip carries the DLLs the daemon loads at start).
const TARGETS = {
  "darwin-arm64": { asset: "wenlan-darwin-arm64.tar.gz", exe: "", all: false },
  "linux-x64": { asset: "wenlan-linux-x64.tar.gz", exe: "", all: false },
  "linux-arm64": { asset: "wenlan-linux-arm64.tar.gz", exe: "", all: false },
  "win32-x64": { asset: "wenlan-windows-x64.zip", exe: ".exe", all: true },
};
const SUPPORTED =
  "macOS on Apple Silicon, Linux on x64 or ARM64, and Windows on x64";

function targetFor(platform, arch) {
  const found = TARGETS[`${platform}-${arch}`];
  if (!found) {
    throw new Error(
      `Wenlan has no build for this computer (${platform} ${arch}). ` +
        `It runs on ${SUPPORTED}.`
    );
  }
  return {
    ...found,
    binaries: ["wenlan", "wenlan-server", "wenlan-mcp"].map((name) => name + found.exe),
  };
}

const DEFAULT_TARGET = targetFor("darwin", "arm64");
const ASSET = DEFAULT_TARGET.asset;
const REQUESTED_TAG =
  process.env.WENLAN_RELEASE_TAG ||
  process.env.WENLAN_TAG ||
  process.env.ORIGIN_RELEASE_TAG ||
  process.env.ORIGIN_TAG ||
  "";
const BINARIES = DEFAULT_TARGET.binaries;
// A file Windows would not let this install replace (a running program) is
// renamed to `<OLD_PREFIX>…` instead and removed by a later run.
const OLD_PREFIX = ".old-";
const SUMS_NAME = "SHA256SUMS";
const SUMS_MAX_BYTES = 1024 * 1024;
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

async function fetchText(url, maxBytes = SUMS_MAX_BYTES) {
  const res = await request(url);
  const chunks = [];
  let size = 0;
  for await (const chunk of res) {
    size += chunk.length;
    if (size > maxBytes) {
      res.destroy();
      throw new Error(`${url} is larger than expected`);
    }
    chunks.push(chunk);
  }
  return Buffer.concat(chunks).toString("utf8");
}

function sha256Of(file) {
  return new Promise((resolve, reject) => {
    const hash = crypto.createHash("sha256");
    fs.createReadStream(file)
      .on("error", reject)
      .on("data", (chunk) => hash.update(chunk))
      .on("end", () => resolve(hash.digest("hex")));
  });
}

// The release's SHA256SUMS names every archive. A download that does not match
// is never unpacked.
async function verifyChecksum(archivePath, tag, asset, overrides = {}) {
  const opts = {
    fetchSums: fetchText,
    skip: process.env.WENLAN_SKIP_CHECKSUM === "1",
    log: (message) => process.stderr.write(message),
    ...overrides,
  };
  const url = `https://github.com/${REPO}/releases/download/${tag}/${SUMS_NAME}`;
  let sums;
  try {
    sums = await opts.fetchSums(url);
  } catch (err) {
    if (opts.skip) {
      opts.log(`Could not get ${SUMS_NAME} for ${tag}; installing unverified because WENLAN_SKIP_CHECKSUM=1.\n`);
      return;
    }
    throw new Error(
      `Could not get ${SUMS_NAME} for ${tag} (${err.message}), so the download cannot be verified. ` +
        "Check the network and run this again."
    );
  }
  let expected = "";
  for (const line of sums.split(/\r?\n/)) {
    const match = /^([0-9a-f]{64})\s+\*?(.+)$/.exec(line.trim());
    if (match && match[2] === asset) {
      expected = match[1];
      break;
    }
  }
  if (!expected) {
    throw new Error(`${SUMS_NAME} for ${tag} has no entry for ${asset}.`);
  }
  const actual = await sha256Of(archivePath);
  if (actual !== expected) {
    throw new Error(
      `The download of ${asset} does not match its checksum (expected ${expected}, got ${actual}). ` +
        "Run this again; if it repeats, do not install."
    );
  }
}

// Windows ships bsdtar as System32\tar.exe, which reads zip. A `tar` found on
// PATH there can be GNU tar from Git Bash, which does not.
function tarCommand(platform = process.platform, env = process.env) {
  if (platform !== "win32") return "tar";
  return path.win32.join(env.SystemRoot || env.windir || "C:\\Windows", "System32", "tar.exe");
}

function extractBinaries(archivePath, dir, target = DEFAULT_TARGET) {
  const args = target.all
    ? ["-xf", archivePath, "-C", dir]
    : ["-xzf", archivePath, "-C", dir, ...target.binaries];
  const result = childProcess.spawnSync(tarCommand(), args, { stdio: "inherit" });
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

function pathHint(dir, platform = process.platform, env = process.env) {
  const delimiter = platform === "win32" ? ";" : ":";
  const same = (a, b) => (platform === "win32" ? a.toLowerCase() === b.toLowerCase() : a === b);
  const entries = (env.PATH || env.Path || "").split(delimiter);
  if (entries.some((entry) => same(entry.replace(/[\\/]+$/, ""), dir))) return "";

  const line =
    platform === "win32"
      ? `  [Environment]::SetEnvironmentVariable("Path", "${dir};" + [Environment]::GetEnvironmentVariable("Path", "User"), "User")`
      : `  export PATH="${dir}:$PATH"`;
  const where = platform === "win32" ? "Run this once in PowerShell, then open a new terminal" : "Add this to your shell profile";
  return (
    `\nWenlan is installed in ${dir}.\n` +
    `To type wenlan directly instead of npx wenlan: ${where}.\n\n${line}\n\n`
  );
}

function printPathHint(dir) {
  const hint = pathHint(dir);
  if (hint) process.stderr.write(hint);
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
function cacheProblemFor(dir, tag, target = DEFAULT_TARGET) {
  const stamp = readStamp(dir);
  if (!stamp) return "no install stamp";
  if (stamp.tag !== tag) return `installed ${stamp.tag}, wanted ${tag}`;
  if (stamp.asset !== target.asset) return `installed asset ${stamp.asset}, wanted ${target.asset}`;
  for (const name of target.binaries) {
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
    if (target.exe === "" && process.platform !== "win32" && (stat.mode & 0o111) === 0) {
      return `${name} is not executable`;
    }
  }
  return null;
}

function sizesOf(dir, target = DEFAULT_TARGET) {
  const sizes = {};
  for (const name of target.binaries) {
    sizes[name] = fs.statSync(path.join(dir, name)).size;
  }
  return sizes;
}

// `sizes` describe the binaries THIS install extracted, not whatever is in
// `dir` by now. If a concurrent install of another release interleaved its
// own files, the next run sees a size that does not match and reinstalls,
// instead of the stamp vouching for a mixture.
function writeStamp(dir, tag, sizes, target = DEFAULT_TARGET) {
  const finalPath = stampPath(dir);
  const tmpPath = `${finalPath}.${process.pid}.tmp`;
  fs.writeFileSync(tmpPath, JSON.stringify({ tag, asset: target.asset, sizes }) + "\n");
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
    if (entry.startsWith(OLD_PREFIX)) {
      // Still running if this fails; a later run tries again.
      try {
        fs.unlinkSync(path.join(dir, entry));
      } catch (_) {
        // In use.
      }
      continue;
    }
    if (!entry.startsWith(STAGING_PREFIX)) continue;
    const staged = path.join(dir, entry);
    try {
      if (now - fs.statSync(staged).mtimeMs > STALE_STAGING_MS) removeTree(staged);
    } catch (_) {
      // Vanished (finished or swept by another process) between list and stat.
    }
  }
}

// Windows refuses to replace a program that is running, but lets it be renamed.
// The running daemon keeps the old file until it restarts; the new one takes
// its name now.
function place(from, to, rename) {
  try {
    rename(from, to);
  } catch (err) {
    if (!["EPERM", "EBUSY", "EACCES"].includes(err.code) || !fs.existsSync(to)) throw err;
    const aside = path.join(
      path.dirname(to),
      `${OLD_PREFIX}${process.pid}-${crypto.randomBytes(4).toString("hex")}-${path.basename(to)}`
    );
    fs.renameSync(to, aside);
    rename(from, to);
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
    verify: verifyChecksum,
    extract: extractBinaries,
    rename: fs.renameSync,
    log: (message) => process.stderr.write(message),
    ...overrides,
  };

  const target = targetFor(opts.platform, opts.arch);
  const ASSET = target.asset;
  const cacheProblem = (where, wanted) => cacheProblemFor(where, wanted, target);

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
    await opts.verify(archivePath, tag, ASSET);
    opts.extract(archivePath, staging, target);
    for (const name of target.binaries) {
      fs.chmodSync(path.join(staging, name), 0o755);
    }
    const sizes = sizesOf(staging, target);
    // The three programs go last, after whatever they load at start.
    const extras = target.all
      ? fs.readdirSync(staging).filter((name) => !target.binaries.includes(name))
      : [];

    // Another process may have finished this same install while this one was
    // downloading. Use theirs rather than replacing binaries a caller may be
    // running right now.
    if (cacheProblem(dir, tag) === null) {
      return dir;
    }

    // No stamp may survive into a half-placed install: if the renames below
    // are interrupted, the next run must see "no install" and start over.
    removeStamp(dir);
    for (const name of [...extras, ...target.binaries]) {
      place(path.join(staging, name), path.join(dir, name), opts.rename);
    }
    writeStamp(dir, tag, sizes, target);
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

// A Linux system with no systemd user session (a container, WSL without
// systemd, some SSH logins) cannot register a background service at all, and
// asking it to leaves a half-registered unit behind.
function hasUserServiceManager(platform = process.platform, spawnSync = childProcess.spawnSync) {
  if (platform !== "linux") return true;
  const probe = spawnSync("systemctl", ["--user", "show-environment"], { stdio: "ignore" });
  return !probe.error && probe.status === 0;
}

const START_WAIT_MS = 30_000;

// Starts the daemon as an ordinary process that outlives this command, and says
// plainly that nothing will start it again after a restart.
async function startForThisSession(dir, overrides = {}) {
  const opts = {
    spawn: childProcess.spawn,
    healthy: (wenlan) =>
      childProcess.spawnSync(wenlan, ["status", "--format", "json"], { stdio: "ignore" }).status === 0,
    waitMs: START_WAIT_MS,
    log: (message) => process.stderr.write(message),
    ...overrides,
  };
  const server = path.join(dir, "wenlan-server");
  const wenlan = path.join(dir, "wenlan");
  const logPath = path.join(path.dirname(dir), "wenlan-server.log");
  let up = opts.healthy(wenlan);
  if (!up) {
    const out = fs.openSync(logPath, "a");
    const child = opts.spawn(server, [], { detached: true, stdio: ["ignore", out, out] });
    child.unref();
    fs.closeSync(out);
  }

  const deadline = Date.now() + opts.waitMs;
  while (!up && Date.now() < deadline) {
    await new Promise((resolve) => setTimeout(resolve, 500));
    up = opts.healthy(wenlan);
  }
  if (!up) {
    throw new Error(
      `Wenlan is installed but did not start. Its output is in ${logPath}. ` +
        `Start it yourself with: ${server}`
    );
  }
  opts.log(
    "\nThis system cannot keep Wenlan running in the background (no systemd user session),\n" +
      "so Wenlan was started for now. After this computer restarts, start it again with:\n\n" +
      `  ${server} &\n\n`
  );
}

async function main() {
  const args = process.argv.slice(2);
  const dir = await installBinaries();
  const wenlan = path.join(dir, targetFor(process.platform, process.arch).binaries[0]);

  if (args[0] === "setup") {
    const setupArgs = args.slice(1);
    run(wenlan, ["setup", ...(setupArgs.length ? setupArgs : ["--basic"])]);
    if (hasUserServiceManager()) {
      run(wenlan, ["background", "on"]);
    } else {
      await startForThisSession(dir);
    }
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
  cacheProblem: cacheProblemFor,
  checkTag,
  pathHint,
  place,
  tarCommand,
  targetFor,
  verifyChecksum,
  extractBinaries,
  installBinaries,
  latestTag,
  hasUserServiceManager,
  startForThisSession,
  request,
  readStamp,
  safeTag,
};
