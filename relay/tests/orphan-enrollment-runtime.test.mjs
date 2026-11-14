// SPDX-License-Identifier: Apache-2.0
import assert from 'node:assert/strict';
import test from 'node:test';
import { oauthKV } from './fixtures/oauth-storage.mjs';
import { createRequire } from 'node:module';
import { resolve, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createHash, randomBytes } from 'node:crypto';

if (!process.env.WENLAN_RELAY_TOOLCHAIN) throw new Error('WENLAN_RELAY_TOOLCHAIN required');
const requireTool = createRequire(join(resolve(process.env.WENLAN_RELAY_TOOLCHAIN), 'package.json'));
const { Miniflare } = requireTool('miniflare');
const { build } = requireTool('esbuild');
const origin = 'https://wenlan-relay.example';
const candidate = { tunnelOrigin: 'https://synthetic.trycloudflare.com',
  backendToken: 'b'.repeat(43), space: 'review' };

test('lost enrollment response leaves no accessible grant and expires through real authority cleanup', { timeout: 60_000 }, async t => {
  const buildOptions = { bundle: true, write: false, format: 'esm', platform: 'browser',
    target: 'es2022', external: ['cloudflare:workers'], loader: { '.png': 'binary' } };
  const bundle = await build({ ...buildOptions,
    entryPoints: [fileURLToPath(new URL('./fixtures/maintenance-worker.ts', import.meta.url))] });
  const production = await build({ ...buildOptions,
    entryPoints: [fileURLToPath(new URL('../src/worker.ts', import.meta.url))] });
  assert(!production.outputFiles[0].text.includes('/__fixture'));
  assert(!production.outputFiles[0].text.includes('MaintenanceFixture'));
  let online = true;
  let backendCalls = 0;
  const runtime = new Miniflare({ modules: true, script: bundle.outputFiles[0].text,
    compatibilityDate: '2026-09-08', host: '127.0.0.1', port: 0,
    bindings: { PUBLIC_ORIGIN: origin }, kvNamespaces: ['OAUTH_KV'],
    durableObjects: { AUTHORITY: { className: 'MaintenanceFixture', useSQLite: true } },
    outboundService: request => {
      backendCalls++;
      const url = new URL(request.url);
      assert.equal(url.origin, candidate.tunnelOrigin, 'unexpected external request');
      if (!online) return new Response(null, { status: 502 });
      if (!request.headers.has('authorization')) return new Response(null, { status: 401 });
      assert.equal(request.headers.get('authorization'), `Bearer ${candidate.backendToken}`);
      if (url.pathname === '/connector-info') return Response.json({ contract_version: 1,
        server: 'wenlan-mcp', tool_profile: 'query-only', authentication: 'bearer', space: candidate.space });
      assert.fail('an unapproved orphan must never reach backend MCP');
    } });
  const fixture = async (operation, records) => {
    const namespace = await runtime.getDurableObjectNamespace('AUTHORITY');
    const response = await namespace.get(namespace.idFromName('wenlan-v1')).fetch(`${origin}/__fixture`, {
      method: 'POST', body: JSON.stringify({ operation, records }),
    });
    assert.equal(response.status, 200);
    return response.json();
  };
  const request = (path, init) => runtime.dispatchFetch(`${origin}${path}`, { ...init, credentials: 'omit' });
  const post = (path, value, headers = {}) => request(path, { method: 'POST',
    headers: { 'content-type': 'application/json', ...headers }, body: JSON.stringify(value) });
  let device;
  let route;
  let cookie;
  let pairId;
  let client;
  try {
    await t.test('server commits enrollment while caller discards its only management credential', async () => {
      const before = Date.now();
      const enrolled = await post('/devices', candidate);
      assert.equal(enrolled.status, 201);
      // Model the post-commit lost-response boundary; never parse or retain the token.
      await enrolled.body.cancel();
      const state = (await fixture('inspect')).records;
      const devices = Object.entries(state).filter(([key]) => key.startsWith('device:'));
      assert.equal(devices.length, 1);
      device = devices[0][1];
      route = state[`connector:${device.id}`];
      assert.equal(device.subject, device.id);
      assert.match(device.credentialHash, /^[a-f0-9]{64}$/);
      assert.equal(Object.hasOwn(device, 'managementToken'), false);
      assert.equal(device.expiresAt - route.expiresAt, 89 * 24 * 60 * 60 * 1000);
      assert(route.expiresAt >= before + 24 * 60 * 60 * 1000);
      assert(route.expiresAt <= Date.now() + 24 * 60 * 60 * 1000);
      assert.equal(backendCalls, 2, 'only anonymous denial and protected connector validation may run');
    });
    assert(device && route, 'enrollment setup must succeed before testing orphan access');
    const registration = await post('/oauth/register', { client_name: 'Synthetic orphan test',
      redirect_uris: ['https://client.example/callback'], grant_types: ['authorization_code', 'refresh_token'],
      response_types: ['code'], token_endpoint_auth_method: 'none' });
    assert.equal(registration.status, 201);
    client = await registration.json();
    const query = new URLSearchParams({ response_type: 'code', client_id: client.client_id,
      redirect_uri: 'https://client.example/callback', resource: `${origin}/mcp`, scope: 'wenlan:query',
      code_challenge_method: 'S256',
      code_challenge: createHash('sha256').update(randomBytes(32).toString('base64url')).digest('base64url') });
    const authorization = await request(`/authorize?${query}`, { redirect: 'manual' });
    assert.equal(authorization.status, 303);
    cookie = authorization.headers.get('set-cookie').split(';')[0];
    pairId = cookie.split('=')[1].split('.')[0];
    await authorization.body?.cancel();
    await t.test('ID, guessed token and backend credential cannot recover ownership or authorize data', async () => {
      const headers = [
        { 'x-wenlan-device-id': device.id },
        { 'x-wenlan-device-id': device.id, authorization: `Bearer ${'g'.repeat(43)}` },
        { 'x-wenlan-device-id': device.id, authorization: `Bearer ${candidate.backendToken}` },
      ];
      for (const credential of headers) {
        assert.equal((await request(`/pairings/${pairId}`, { headers: credential })).status, 401);
        assert.equal((await post(`/pairings/${pairId}/approve`, { approved: true,
          clientId: client.client_id, resource: `${origin}/mcp`, space: candidate.space }, credential)).status, 401);
        assert.equal((await post('/devices/refresh', candidate, credential)).status, 401);
        assert.equal((await post('/devices/rotate', {}, credential)).status, 401);
        assert.equal((await post('/devices/revoke', {}, credential)).status, 401);
        assert.equal((await request('/grants', { headers: credential })).status, 401);
        assert.equal((await post(`/mcp?device_id=${device.id}`, { jsonrpc: '2.0', id: 1,
          method: 'tools/call', params: { name: 'brief', arguments: {} } }, credential)).status, 401);
      }
      const completion = await post('/pairing/complete', {}, { cookie, origin });
      assert.equal(completion.status, 409);
      assert.equal(completion.headers.get('location'), null);
      assert.equal(backendCalls, 2, 'denied attempts must not probe or call the backend');
      const state = (await fixture('inspect')).records;
      assert.deepEqual(state[`device:${device.id}`], device);
      assert.deepEqual(state[`connector:${device.id}`], route);
      assert.equal(Object.keys(state).some(key => key.startsWith('oauth-grant:')), false);
      const kv = await oauthKV(runtime);
      assert.equal((await kv.list({ prefix: 'grant:' })).keys.length, 0);
      assert.equal((await kv.list({ prefix: 'token:' })).keys.length, 0);
    });
    await t.test('offline orphan stays inaccessible; route expiry and physical retention are distinct', async () => {
      online = false;
      assert.equal((await post('/mcp', { jsonrpc: '2.0', id: 2, method: 'initialize' },
        { authorization: `Bearer ${candidate.backendToken}` })).status, 401);
      assert.equal(backendCalls, 2);
      const state = (await fixture('inspect')).records;
      const expired = {};
      for (const [key, record] of Object.entries(state)) {
        if (['pairing:', 'oauth-request:', 'authorization-pairing:'].some(prefix => key.startsWith(prefix))) {
          expired[key] = { ...record, expiresAt: Date.now() - 1 };
        }
      }
      assert(Object.keys(expired).some(key => key.startsWith('pairing:')));
      // Simulate expiry in isolated storage, not a claim of 30-day wall-clock observation.
      await fixture('seed', { ...expired, [`connector:${device.id}`]: { ...route, expiresAt: Date.now() - 1 } });
      await fixture('alarm');
      const retained = (await fixture('inspect')).records;
      assert(retained[`connector:${device.id}`], 'live management credentials retain offline recovery state');
      assert(retained[`device:${device.id}`]);
      for (const key of Object.keys(expired)) assert.equal(retained[key], undefined);
      await fixture('seed', { [`device:${device.id}`]: { ...device, expiresAt: Date.now() - 1 } });
      await fixture('alarm');
      const final = (await fixture('inspect')).records;
      assert.equal(final[`device:${device.id}`], undefined);
      assert.equal(final[`connector:${device.id}`], undefined);
      assert.equal(Object.keys(final).some(key => key.startsWith('oauth-client:')), false,
        'no device/client consent was approved');
      const kv = await oauthKV(runtime);
      const clients = await kv.list({ prefix: 'client:' });
      const registered = clients.keys.find(key => key.name === `client:${client.client_id}`);
      assert(registered, 'DCR has a separate retention lifetime in provider SQL');
      assert(registered.expiration > Math.floor(Date.now() / 1000) + 89 * 24 * 60 * 60);
      assert.equal(backendCalls, 2);
    });
  } finally { await runtime.dispose(); }
});
