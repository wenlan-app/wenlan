import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { existsSync, readdirSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

if (!process.env.WENLAN_RELAY_TOOLCHAIN) throw new Error('WENLAN_RELAY_TOOLCHAIN required');
const requireTool = createRequire(join(resolve(process.env.WENLAN_RELAY_TOOLCHAIN), 'package.json'));
const { unstable_readConfig, experimental_readRawConfig } = requireTool('wrangler');
const root = fileURLToPath(new URL('../', import.meta.url));

const runtimeConfigs = [
  {
    file: 'wrangler.standalone.toml',
    name: 'wenlan-relay',
    origin: 'https://relay.wenlan.app',
    oauthKvId: 'ed4a99faf6554605913834b0fdc29028',
  },
  {
    file: 'wrangler.staging.toml',
    name: 'wenlan-relay-staging',
    origin: 'https://wenlan-relay-staging.wenlan-app.workers.dev',
    oauthKvId: 'b338f8dae5c74aa88bf914ecb8371f5a',
    previewUrls: false,
  },
];

function loadConfig(file, env) {
  const config = join(root, file);
  assert.ok(existsSync(config), `${file} must be present before parsed config tests run`);
  const { rawConfig } = experimental_readRawConfig({ config });
  const parsed = unstable_readConfig({ config, ...(env ? { env } : {}) },
    { hideWarnings: true, preserveOriginalMain: true });
  return { parsed, rawConfig };
}

function assertRuntimeConfig(parsed, expected) {
  assert.equal(parsed.name, expected.name, 'runtime name must be explicit');
  assert.equal(parsed.main, 'src/worker.ts', 'runtime must use the standalone entrypoint');
  assert.deepEqual(parsed.services, [], 'runtime must not bind a forwarding service');

  const kvNamespaces = parsed.kv_namespaces ?? [];
  assert.equal(kvNamespaces.length, 1, 'runtime must have exactly one KV namespace');
  assert.deepEqual(kvNamespaces, [{ binding: 'OAUTH_KV', id: expected.oauthKvId }],
    'runtime must bind only its expected OAuth KV namespace');

  const durableObjectBindings = parsed.durable_objects?.bindings ?? [];
  for (const binding of durableObjectBindings) {
    for (const field of ['script_name', 'environment', 'namespace_id']) {
      assert.equal(binding[field], undefined, `Durable Object must not use cross-script ${field}`);
    }
  }
  assert.deepEqual(durableObjectBindings, [{ name: 'AUTHORITY', class_name: 'RelayAuthority' }],
    'runtime must bind only its local RelayAuthority Durable Object');

  assert.equal(parsed.vars?.PUBLIC_ORIGIN, expected.origin, 'runtime origin must match its worker name');
  assert.equal(Object.hasOwn(parsed.vars ?? {}, 'SAMPLE_ACCOUNT'), false,
    'runtime vars must not configure a sample account');
  // Workers Logs keep only the relay's own content-free request lines.
  assert.equal(parsed.observability?.enabled, true, 'runtime request logs must be enabled');
  assert.equal(parsed.observability?.logs?.invocation_logs, false,
    'invocation logs would record request URLs and headers');
  assert.equal(parsed.observability?.traces?.enabled ?? false, false, 'traces must stay disabled');
  if (expected.previewUrls !== undefined) {
    assert.equal(parsed.preview_urls, expected.previewUrls, 'staging preview URLs must be disabled');
  }
}

// Include templates and named environments: an alternate config can still
// target either live Worker, even when the two canonical files are correct.
for (const file of readdirSync(root).filter(name => /^wrangler.*\.toml$/.test(name))) {
  const { rawConfig } = loadConfig(file);
  for (const env of [undefined, ...Object.keys(rawConfig.env ?? {})]) {
    test(`${file}${env ? ` (${env})` : ''} cannot forward either live relay`, () => {
      const { parsed } = loadConfig(file, env);
      if (!runtimeConfigs.some(expected => expected.name === parsed.name)) return;
      assert.equal(parsed.main, 'src/worker.ts');
      assert.deepEqual(parsed.services, []);
      assert.deepEqual(parsed.durable_objects?.bindings,
        [{ name: 'AUTHORITY', class_name: 'RelayAuthority' }]);
    });
  }
}

for (const expected of runtimeConfigs) {
  test(`${expected.file} has the expected isolated runtime shape`, () => {
    const { parsed } = loadConfig(expected.file);
    assertRuntimeConfig(parsed, expected);
  });
}

test('production and staging use independent worker identities and OAuth KV namespaces', () => {
  const [production, staging] = runtimeConfigs.map(({ file }) => loadConfig(file).parsed);
  assert.notEqual(production.name, staging.name);
  assert.notEqual(production.vars?.PUBLIC_ORIGIN, staging.vars?.PUBLIC_ORIGIN);
  assert.notEqual(production.kv_namespaces[0].id, staging.kv_namespaces[0].id);
});

test('staging does not inherit production verification or custom routes', () => {
  const production = loadConfig('wrangler.standalone.toml').parsed;
  const staging = loadConfig('wrangler.staging.toml').parsed;
  assert.equal(staging.vars?.DOMAIN_VERIFICATION_TOKEN, undefined,
    'staging must not contain the production domain verification token');
  assert.deepEqual(staging.routes ?? [], [], 'staging must not configure production custom routes');
  assert.notEqual(production.vars?.DOMAIN_VERIFICATION_TOKEN, undefined,
    'production must retain its configured domain verification token');
});

test('production has no named environment that can override its top-level deployment', () => {
  const { rawConfig } = loadConfig('wrangler.standalone.toml');
  assert.deepEqual(Object.keys(rawConfig.env ?? {}), [],
    'production must not rely on Wrangler environment inheritance');
});

test('production uses exactly the approved custom route and staging has none', () => {
  const production = loadConfig('wrangler.standalone.toml').parsed;
  const staging = loadConfig('wrangler.staging.toml').parsed;
  assert.deepEqual(production.routes ?? [], [{ pattern: 'relay.wenlan.app', custom_domain: true }],
    'production must use exactly relay.wenlan.app as its custom route');
  assert.deepEqual(staging.routes ?? [], [], 'staging must not configure a custom route');
});

test('runtime invariant rejects a cross-script Durable Object mutation', () => {
  const { parsed } = loadConfig('wrangler.standalone.toml');
  const mutated = structuredClone(parsed);
  mutated.durable_objects.bindings[0].script_name = 'legacy-origin-relay';
  assert.throws(() => assertRuntimeConfig(mutated, runtimeConfigs[0]), /cross-script script_name/);
});
