// SPDX-License-Identifier: Apache-2.0
// Bounded reviewer preparation CLI (setup tooling, not an MCP tool/backend).
// Uses EXISTING seedReviewerViaApi + checkDaemonUrl helpers only.
// No retries, rollback, deletion, portal writes, or auth secrets.

import { closeSync, fchmodSync, ftruncateSync, openSync, writeSync, readFileSync, lstatSync } from 'node:fs';
import { isAbsolute, dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { seedReviewerViaApi, checkDaemonUrl } from '../tests/fixtures/reviewer-api-seed.mjs';

export const HISTORICAL_IDS = [
  'mem_atlas-auth',
  'mem_atlas-unavailable',
  'mem_private-sentinel',
  'page_atlas-auth',
  'page_atlas-unavailable',
  'page_private-sentinel',
];

const ID_RE = /^[A-Za-z0-9][A-Za-z0-9_-]{0,127}$/;

const KNOWN_FLAGS = new Set(['--daemon-url', '--knowledge-path', '--output', '--confirm-synthetic-library', '--help']);

export function renderHelp() {
  return [
    'Usage: node relay/reviewer/prepare.mjs --daemon-url <http://127.0.0.1:PORT> --knowledge-path <abs> --output <abs> --confirm-synthetic-library',
    '',
    'Prepares synthetic reviewer fixtures via the existing daemon API seed helper.',
    'Reviewer setup tooling only; not submit-ready and not a backend/MCP tool.',
    '',
    'Required (no defaults):',
    '  --daemon-url <origin>          Explicit canonical http://127.0.0.1:<nondefault-port> origin.',
    '  --knowledge-path <abs>         Expected absolute live knowledge path.',
    '  --output <abs>                 Absolute receipt output path. Reserved exclusively (0600); existing file/symlink/dir fails.',
    '  --confirm-synthetic-library    Explicit consent flag (no value). Required.',
    '',
    '  --help                         Print this help. Performs no network access and no writes.',
  ].join('\n');
}

export function parseArgs(argv) {
  const raw = Array.isArray(argv) ? argv : [];
  const seen = new Map();
  const values = {};
  let help = false;
  for (let i = 0; i < raw.length; i++) {
    const token = raw[i];
    let name;
    let inlineValue;
    const eq = token.indexOf('=');
    if (eq !== -1) {
      name = token.slice(0, eq);
      inlineValue = token.slice(eq + 1);
    } else {
      name = token;
      inlineValue = undefined;
    }
    if (!name.startsWith('--')) throw new Error(`unknown argument: ${token}`);
    if (!KNOWN_FLAGS.has(name)) throw new Error(`unknown argument: ${name}`);
    if (seen.has(name)) throw new Error(`duplicate argument: ${name}`);
    seen.set(name, true);
    if (name === '--help') {
      if (inlineValue !== undefined) throw new Error('--help takes no value');
      help = true;
      continue;
    }
    if (name === '--confirm-synthetic-library') {
      if (inlineValue !== undefined) throw new Error('--confirm-synthetic-library takes no value');
      values.confirmSyntheticLibrary = true;
      continue;
    }
    let value = inlineValue;
    if (value === undefined) {
      value = raw[i + 1];
      i += 1;
      if (value === undefined || value.startsWith('--')) {
        throw new Error(`missing value for ${name}`);
      }
    }
    if (value.length === 0) throw new Error(`missing value for ${name}`);
    if (name === '--daemon-url') values.daemonUrl = value;
    else if (name === '--knowledge-path') values.knowledgePath = value;
    else if (name === '--output') values.output = value;
  }
  if (help) {
    if (raw.length !== 1) throw new Error('--help takes no other arguments');
    return { help: true };
  }
  const missing = [];
  if (!values.daemonUrl) missing.push('--daemon-url');
  if (!values.knowledgePath) missing.push('--knowledge-path');
  if (!values.output) missing.push('--output');
  if (!values.confirmSyntheticLibrary) missing.push('--confirm-synthetic-library');
  if (missing.length > 0) throw new Error(`missing required arguments: ${missing.join(', ')}`);
  return {
    daemonUrl: values.daemonUrl,
    knowledgePath: values.knowledgePath,
    output: values.output,
    confirmSyntheticLibrary: true,
  };
}

export function defaultTemplatePath() {
  return resolve(dirname(fileURLToPath(import.meta.url)), '../chatgpt-app-submission.json');
}

function isNonEmptyString(value) {
  return typeof value === 'string' && value.trim().length > 0;
}

// Structural template check only: counts plus required fields. Runs before any
// file reservation or daemon call, so malformed templates never seed or write.
export function validateTemplate(template) {
  if (!template || typeof template !== 'object' || Array.isArray(template)) {
    throw new Error('submission template is malformed');
  }
  if (!Array.isArray(template.test_cases) || template.test_cases.length !== 5) {
    throw new Error('submission template must keep five positive test cases');
  }
  if (!Array.isArray(template.negative_test_cases) || template.negative_test_cases.length !== 3) {
    throw new Error('submission template must keep three negative test cases');
  }
  for (const [index, entry] of template.test_cases.entries()) {
    if (!entry || typeof entry !== 'object') throw new Error(`submission template test_cases[${index}] is malformed`);
    for (const field of ['description', 'user_prompt', 'expected_output']) {
      if (!isNonEmptyString(entry[field])) {
        throw new Error(`submission template test_cases[${index}].${field} must be nonempty`);
      }
    }
    if (!['brief', 'recall', 'get_page_sources'].includes(entry.tools_triggered)) {
      throw new Error(`submission template test_cases[${index}].tools_triggered must name a query-only tool`);
    }
  }
  for (const [index, entry] of template.negative_test_cases.entries()) {
    if (!entry || typeof entry !== 'object') {
      throw new Error(`submission template negative_test_cases[${index}] is malformed`);
    }
    for (const field of ['description', 'user_prompt']) {
      if (!isNonEmptyString(entry[field])) {
        throw new Error(`submission template negative_test_cases[${index}].${field} must be nonempty`);
      }
    }
    if (entry.tools_triggered !== null) {
      throw new Error(`submission template negative_test_cases[${index}].tools_triggered must be null`);
    }
  }
  const publicCases = JSON.stringify(template.test_cases);
  for (const token of ['mem_atlas-auth', 'page_atlas-auth', 'page_atlas-unavailable']) {
    if (!containsExactToken(publicCases, token)) {
      throw new Error(`submission template missing fixture binding: ${token}`);
    }
  }
  return template;
}

function escapeRegExp(text) {
  return text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

// Exact identifier token: not embedded in a longer [A-Za-z0-9_-] run.
function exactTokenPattern(token) {
  return new RegExp(`(?<![A-Za-z0-9_-])${escapeRegExp(token)}(?![A-Za-z0-9_-])`, 'g');
}

export function containsExactToken(text, token) {
  return exactTokenPattern(token).test(text);
}

// One-pass substitution over exact identifier tokens only: a single replace
// call, so substituted values are never re-scanned (no cascading).
function replaceTokensInString(value, mapping) {
  const pattern = new RegExp(
    `(?<![A-Za-z0-9_-])(${HISTORICAL_IDS.map(escapeRegExp).join('|')})(?![A-Za-z0-9_-])`,
    'g',
  );
  return value.replace(pattern, (match) => mapping[match]);
}

function deepReplaceStrings(node, mapping) {
  if (typeof node === 'string') return replaceTokensInString(node, mapping);
  if (Array.isArray(node)) return node.map((entry) => deepReplaceStrings(entry, mapping));
  if (node !== null && typeof node === 'object') {
    const out = {};
    for (const [key, val] of Object.entries(node)) out[key] = deepReplaceStrings(val, mapping);
    return out;
  }
  return node;
}

export function validateMapping(mapping) {
  if (mapping === null || typeof mapping !== 'object' || Array.isArray(mapping)) {
    throw new Error('seed returned malformed mapping');
  }
  for (const token of HISTORICAL_IDS) {
    const value = mapping[token];
    if (typeof value !== 'string' || !ID_RE.test(value)) {
      throw new Error(`malformed generated ID for ${token}`);
    }
    if (HISTORICAL_IDS.includes(value)) {
      throw new Error(`generated ID for ${token} must not be a historical token`);
    }
  }
  const values = HISTORICAL_IDS.map((token) => mapping[token]);
  if (new Set(values).size !== values.length) throw new Error('duplicate generated ID in mapping');
  return mapping;
}

export function transformSubmission(template, mapping) {
  validateTemplate(template);
  validateMapping(mapping);
  const beforeTools = template.test_cases.map((c) => c?.tools_triggered);
  const testCases = deepReplaceStrings(template.test_cases, mapping);
  const negativeTestCases = deepReplaceStrings(template.negative_test_cases, mapping);
  const afterTools = testCases.map((c) => c?.tools_triggered);
  if (JSON.stringify(beforeTools) !== JSON.stringify(afterTools)) {
    throw new Error('tool names must be preserved');
  }
  const negBeforeTools = template.negative_test_cases.map((c) => c?.tools_triggered);
  const negAfterTools = negativeTestCases.map((c) => c?.tools_triggered);
  if (JSON.stringify(negBeforeTools) !== JSON.stringify(negAfterTools)) {
    throw new Error('negative tool names must be preserved');
  }
  const serialized = JSON.stringify({ testCases, negativeTestCases });
  for (const token of HISTORICAL_IDS) {
    if (containsExactToken(serialized, token)) throw new Error(`residual historical token: ${token}`);
  }
  // Private-sentinel IDs must not leak into public prompts unless the template had them.
  const templateSerialized = JSON.stringify({
    t: template.test_cases,
    n: template.negative_test_cases,
  });
  for (const token of ['mem_private-sentinel', 'page_private-sentinel']) {
    if (!containsExactToken(templateSerialized, token) && containsExactToken(serialized, mapping[token])) {
      throw new Error(`private-sentinel ID leaked into public test cases: ${token}`);
    }
  }
  return { testCases, negativeTestCases };
}

// Rewrite the receipt through the SAME fd: truncate then write at position 0.
// Never reopens by path, so a swapped-in symlink cannot redirect the write.
export function writeJsonAtZero(fd, payload, write = writeSync) {
  const bytes = Buffer.from(`${JSON.stringify(payload, null, 2)}\n`, 'utf8');
  ftruncateSync(fd, 0);
  let offset = 0;
  while (offset < bytes.length) {
    const written = write(fd, bytes, offset, bytes.length - offset, offset);
    if (!Number.isInteger(written) || written <= 0 || written > bytes.length - offset) {
      throw new Error('receipt write did not make valid progress');
    }
    offset += written;
  }
}

export async function prepare(parsed, deps = {}) {
  const { daemonUrl, knowledgePath, output, confirmSyntheticLibrary } = parsed ?? {};
  if (confirmSyntheticLibrary !== true) throw new Error('missing --confirm-synthetic-library consent');
  if (typeof daemonUrl !== 'string' || daemonUrl.length === 0) throw new Error('missing --daemon-url');
  if (typeof knowledgePath !== 'string' || !isAbsolute(knowledgePath)) {
    throw new Error('--knowledge-path must be an absolute path');
  }
  if (typeof output !== 'string' || !isAbsolute(output)) throw new Error('--output must be an absolute path');
  const origin = checkDaemonUrl(daemonUrl);
  const seedFn = deps.seedFn ?? seedReviewerViaApi;
  const fetchImpl = deps.fetchImpl ?? globalThis.fetch;
  const templatePath = deps.templatePath ?? defaultTemplatePath();
  let template;
  try {
    template = JSON.parse(readFileSync(templatePath, 'utf8'));
  } catch (err) {
    throw new Error(`cannot read submission template: ${err?.message ?? err}`);
  }
  // Template shape is validated BEFORE reserving the output file or seeding,
  // so a malformed template causes neither writes nor daemon calls.
  validateTemplate(template);

  // Reserve output BEFORE any daemon mutation. Existing path/symlink/dir fails here.
  let lstatOk = false;
  try {
    lstatSync(output);
    lstatOk = true;
  } catch (err) {
    if (err?.code !== 'ENOENT') throw err;
  }
  if (lstatOk) throw new Error('refusing to overwrite existing output (file, symlink, or directory)');
  const fd = openSync(output, 'wx', 0o600);
  const failedReceipt = (stage, err) => ({
    status: 'failed',
    stage,
    daemonUrl: origin,
    knowledgePath,
    error: err?.message ?? String(err),
    partialDataMayRemain: true,
    warning: 'Partial synthetic data may remain in the daemon. No automatic rerun was performed; inspect daemon state before retrying with a fresh output path.',
  });
  try {
    try {
      fchmodSync(fd, 0o600);
    } catch {}
    writeJsonAtZero(fd, { status: 'preparing', daemonUrl: origin, knowledgePath });
    console.log(`preparing reviewer fixtures at ${origin} ...`);
    let seedResult;
    try {
      seedResult = await seedFn({ daemonUrl: origin, expectedKnowledgePath: knowledgePath, fetchImpl });
    } catch (err) {
      writeJsonAtZero(fd, failedReceipt('seed', err));
      throw new Error(`reviewer preparation failed during seed; partial data may remain: ${err?.message ?? err}`);
    }
    let testCases;
    let negativeTestCases;
    try {
      validateMapping(seedResult?.mapping);
      ({ testCases, negativeTestCases } = transformSubmission(template, seedResult.mapping));
    } catch (err) {
      writeJsonAtZero(fd, failedReceipt('validate', err));
      throw err;
    }
    const receipt = {
      status: 'prepared',
      daemonUrl: origin,
      knowledgePath,
      fixture: seedResult,
      test_cases: testCases,
      negative_test_cases: negativeTestCases,
      verification: {
        installation: 'unchecked',
        chatgpt: 'unchecked',
        oauth: 'unchecked',
        relay: 'unchecked',
        reviewAcceptance: 'unchecked',
      },
      notes: [
        'Normal authored pages/sources only. This receipt prepares fixtures; it does not claim installation, ChatGPT, OAuth, relay, or review acceptance.',
        'Do not call this submission ready.',
      ],
    };
    writeJsonAtZero(fd, receipt);
    return receipt;
  } finally {
    closeSync(fd);
  }
}

async function main(argv) {
  let parsed;
  try {
    parsed = parseArgs(argv);
  } catch (err) {
    console.error(`reviewer prepare: ${err?.message ?? err}`);
    console.error(renderHelp());
    process.exitCode = 2;
    return;
  }
  if (parsed?.help) {
    console.log(renderHelp());
    return;
  }
  try {
    const receipt = await prepare(parsed);
    console.log(`reviewer prepared: ${receipt.test_cases.length} cases -> ${parsed.output}`);
  } catch (err) {
    console.error(`reviewer prepare failed: ${err?.message ?? err}`);
    process.exitCode = 1;
  }
}

const invokedAsMain = (() => {
  try {
    const entry = process.argv[1];
    if (!entry) return false;
    return resolve(entry) === fileURLToPath(import.meta.url);
  } catch {
    return false;
  }
})();

if (invokedAsMain) {
  await main(process.argv.slice(2));
}
