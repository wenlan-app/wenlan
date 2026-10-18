// SPDX-License-Identifier: Apache-2.0
import assert from 'node:assert/strict';
import test from 'node:test';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import { join, resolve } from 'node:path';

const toolchain = process.env.WENLAN_RELAY_TOOLCHAIN;
if (!toolchain) throw new Error('Set WENLAN_RELAY_TOOLCHAIN to an installed Wrangler project directory');
const requireTool = createRequire(join(resolve(toolchain), 'package.json'));
const { Miniflare } = requireTool('miniflare');
const { build } = requireTool('esbuild');
const origin = 'https://wenlan-relay.example';
// Freeze time mid-hour so no window rolls over during the test.
const fixtureTime = Math.floor(Date.now() / 3_600_000) * 3_600_000 + 30 * 60_000;
const fixtureSource = `
export { default, RelayAuthority } from './src/worker.ts';
Date.now = () => ${fixtureTime};
`;

test('limits follow the caller identity and every request leaves a content-free log line', { timeout: 120_000 }, async t => {
  const bundle = await build({
    stdin: { contents: fixtureSource, resolveDir: fileURLToPath(new URL('../', import.meta.url)), sourcefile: 'limits-fixture.ts' },
    bundle: true, write: false, format: 'esm', platform: 'browser', target: 'es2022',
    external: ['cloudflare:workers'], loader: { '.png': 'binary' },
  });
  const lines = [];
  const runtime = new Miniflare({
    modules: true, script: bundle.outputFiles[0].text,
    compatibilityDate: '2026-09-08', host: '127.0.0.1', port: 0,
    bindings: { PUBLIC_ORIGIN: origin }, kvNamespaces: ['OAUTH_KV'],
    durableObjects: { AUTHORITY: { className: 'RelayAuthority', useSQLite: true } },
    outboundService: () => new Response('Synthetic outbound disabled', { status: 503 }),
    structuredWorkerdLogs: true,
    handleStructuredLogs: ({ message }) => { lines.push(message); },
  });
  const request = (path, init = {}) => runtime.dispatchFetch(`${origin}${path}`, { ...init, credentials: 'omit' });
  const register = async (ip, redirectUri) => {
    const response = await request('/oauth/register', {
      method: 'POST', headers: { 'content-type': 'application/json', 'cf-connecting-ip': ip },
      body: JSON.stringify({ redirect_uris: [redirectUri], client_name: 'Test', token_endpoint_auth_method: 'none' }),
    });
    await response.arrayBuffer();
    return response;
  };
  const logged = () => lines.flatMap(line => {
    const start = line.indexOf('{');
    if (start < 0) return [];
    try {
      const value = JSON.parse(line.slice(start));
      return value.event === 'relay_request' ? [value] : [];
    } catch { return []; }
  });
  const settle = () => new Promise(done => setTimeout(done, 200));

  try {
    await t.test('rejected bearer tokens are limited per address without spending its anonymous budget', async () => {
      const ip = '198.51.100.7';
      const forged = 'forged-token-value-that-must-never-be-logged';
      const call = () => request('/mcp', { method: 'POST',
        headers: { authorization: `Bearer ${forged}`, 'content-type': 'application/json', 'cf-connecting-ip': ip },
        body: JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'tools/list' }) });
      for (let n = 0; n < 60; n++) {
        const response = await call();
        assert.equal(response.status, 401, `attempt ${n + 1}`);
        await response.arrayBuffer();
      }
      const blocked = await call();
      assert.equal(blocked.status, 429);
      assert(Number(blocked.headers.get('retry-after')) >= 1);
      await blocked.arrayBuffer();
      // The same address still reaches anonymous routes: its address budget was untouched.
      for (let n = 0; n < 100; n++) {
        const css = await request('/pairing.css', { headers: { 'cf-connecting-ip': ip } });
        assert.equal(css.status, 200, `anonymous request ${n + 1}`);
        await css.arrayBuffer();
      }
      await settle();
      const entries = logged().filter(entry => entry.route === '/mcp');
      assert(entries.some(entry => entry.status === 401 && entry.who === 'grant'));
      assert(entries.some(entry => entry.status === 429 && entry.limit === 'rejected-credentials-minute'));
    });

    await t.test('concurrent guesses from one address cannot exceed the rejected-credential budget', async () => {
      const responses = await Promise.all(Array.from({ length: 100 }, (_, n) => request('/mcp', { method: 'POST',
        headers: { authorization: `Bearer guess-${n}`, 'content-type': 'application/json', 'cf-connecting-ip': '198.51.100.8' },
        body: '{}' })));
      for (const response of responses) await response.arrayBuffer();
      assert.equal(responses.filter(response => response.status === 401).length, 60);
      assert.equal(responses.filter(response => response.status === 429).length, 40);
    });

    await t.test('allowlisted AI apps get a larger registration budget per address than unknown clients', async () => {
      const shared = '203.0.113.9';
      for (let n = 0; n < 60; n++) {
        const response = await register(shared, 'https://claude.ai/api/mcp/auth_callback');
        assert.notEqual(response.status, 429, `Claude registration ${n + 1}`);
      }
      const claudeBlocked = await register(shared, 'https://claude.ai/api/mcp/auth_callback');
      assert.equal(claudeBlocked.status, 429);
      // Another app's budget on the same address is separate.
      assert.notEqual((await register(shared, 'https://chatgpt.com/connector_platform_oauth_redirect')).status, 429);

      const unknown = '203.0.113.10';
      for (let n = 0; n < 20; n++) {
        assert.notEqual((await register(unknown, 'https://example.org/callback')).status, 429, `unknown registration ${n + 1}`);
      }
      assert.equal((await register(unknown, 'https://example.org/callback')).status, 429);
      // A look-alike host shares the unknown-client budget.
      assert.equal((await register(unknown, 'https://claude.ai.example.org/callback')).status, 429);

      await settle();
      const entries = logged().filter(entry => entry.route === '/oauth/register');
      assert(entries.some(entry => entry.app === 'Claude' && entry.status === 429 && entry.limit === 'register-address-hour'));
      assert(entries.some(entry => entry.app === 'other' && entry.status === 429));
    });

    await t.test('web access enrollment allows ten attempts per address per hour', async () => {
      const ip = '192.0.2.40';
      for (let n = 0; n < 10; n++) {
        const response = await request('/devices', { method: 'POST',
          headers: { 'content-type': 'application/json', 'cf-connecting-ip': ip }, body: '{}' });
        assert.equal(response.status, 400, `enrollment ${n + 1}`);
        await response.arrayBuffer();
      }
      const blocked = await request('/devices', { method: 'POST',
        headers: { 'content-type': 'application/json', 'cf-connecting-ip': ip }, body: '{}' });
      assert.equal(blocked.status, 429);
      await blocked.arrayBuffer();
      await settle();
      assert(logged().some(entry => entry.route === '/devices' && entry.limit === 'enroll-address-hour'));
    });

    await t.test('forged device credentials count as rejected and log only a short device hash', async () => {
      const ip = '192.0.2.41';
      const id = 'd'.repeat(40);
      const token = 't'.repeat(48);
      const response = await request('/grants', { headers: {
        authorization: `Bearer ${token}`, 'x-wenlan-device-id': id, 'cf-connecting-ip': ip } });
      assert.equal(response.status, 401);
      await response.arrayBuffer();
      await settle();
      const entry = logged().find(item => item.route === '/grants');
      assert.equal(entry.who, 'device');
      assert.match(entry.device, /^[0-9a-f]{12}$/);
    });

    await t.test('log lines carry no credentials, addresses or bodies', async () => {
      await settle();
      const entries = logged();
      assert(entries.length > 100, `only ${entries.length} log lines`);
      const text = lines.join('\n');
      for (const secret of ['forged-token-value', 'guess-', 'd'.repeat(40), 't'.repeat(48), '198.51.100.7', '203.0.113.9', 'example.org', 'Bearer']) {
        assert(!text.includes(secret), `log output contains ${secret}`);
      }
      for (const entry of entries) {
        assert.deepEqual(Object.keys(entry).filter(key =>
          !['event', 'route', 'method', 'status', 'ms', 'who', 'device', 'app', 'limit', 'error'].includes(key)), []);
        assert.equal(typeof entry.status, 'number');
      }
    });
  } finally { await runtime.dispose(); }
});
