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
const resource = `${origin}/mcp`;
const candidate = { tunnelOrigin: 'https://synthetic.trycloudflare.com', backendToken: 'b'.repeat(43), space: 'review' };

test('actual Worker supports device-scoped per-client disconnection', { timeout: 60_000 }, async t => {
  const bundle = await build({ entryPoints: [fileURLToPath(new URL('./fixtures/oauth-storage-worker.ts', import.meta.url))],
    bundle: true, write: false, format: 'esm', platform: 'browser', target: 'es2022',
    external: ['cloudflare:workers'], loader: { '.png': 'binary' } });
  let backendCalls = 0;
  const runtime = new Miniflare({ modules: true, script: bundle.outputFiles[0].text,
    compatibilityDate: '2026-09-08', host: '127.0.0.1', port: 0,
    bindings: { PUBLIC_ORIGIN: origin }, kvNamespaces: ['OAUTH_KV'],
    durableObjects: { AUTHORITY: { className: 'RelayAuthority', useSQLite: true } },
    outboundService: async request => {
      const url = new URL(request.url);
      if (url.origin !== candidate.tunnelOrigin) return new Response(null, { status: 502 });
      if (!request.headers.has('authorization')) return new Response(null, { status: 401 });
      assert.equal(request.headers.get('authorization'), `Bearer ${candidate.backendToken}`);
      if (url.pathname === '/connector-info') return Response.json({ contract_version: 1, server: 'wenlan-mcp',
        tool_profile: 'query-only', authentication: 'bearer', space: candidate.space });
      if (url.pathname !== '/mcp') return new Response(null, { status: 404 });
      backendCalls++;
      if (request.method === 'GET') return new Response(new ReadableStream({ start(controller) {
        controller.enqueue(new TextEncoder().encode('data: synthetic-heartbeat\n\n'));
      } }), { headers: { 'content-type': 'text/event-stream' } });
      const rpc = await request.json();
      return Response.json({ jsonrpc: '2.0', id: rpc.id, result: { synthetic: true } }, {
        headers: rpc.method === 'initialize' ? { 'mcp-session-id': randomBytes(16).toString('hex') } : {},
      });
    },
  });
  const request = (path, init) => runtime.dispatchFetch(`${origin}${path}`, { ...init, credentials: 'omit' });
  const post = (path, data, headers = {}) => request(path, { method: 'POST',
    headers: { 'content-type': 'application/json', ...headers }, body: JSON.stringify(data) });
  const native = device => ({ authorization: `Bearer ${device.managementToken}`, 'x-wenlan-device-id': device.id });
  const token = data => request('/oauth/token', { method: 'POST',
    headers: { 'content-type': 'application/x-www-form-urlencoded' }, body: new URLSearchParams(data).toString() });
  async function connect(device, name, registeredClient) {
    let client = registeredClient;
    if (!client) {
      const registration = await post('/oauth/register', { client_name: name,
        redirect_uris: ['https://client.example/callback'], grant_types: ['authorization_code', 'refresh_token'],
        response_types: ['code'], token_endpoint_auth_method: 'none' });
      assert.equal(registration.status, 201);
      client = await registration.json();
    }
    const verifier = randomBytes(32).toString('base64url');
    const params = new URLSearchParams({ response_type: 'code', client_id: client.client_id,
      redirect_uri: 'https://client.example/callback', resource, scope: 'wenlan:query', code_challenge_method: 'S256',
      code_challenge: createHash('sha256').update(verifier).digest('base64url') });
    const authorization = await request(`/authorize?${params}`, { redirect: 'manual' });
    assert.equal(authorization.status, 303);
    const cookie = authorization.headers.get('set-cookie').split(';')[0];
    const pairId = cookie.split('=')[1].split('.')[0];
    assert.equal((await post(`/pairings/${pairId}/approve`, {
      approved: true, clientId: client.client_id, resource, space: candidate.space,
    }, native(device))).status, 200);
    const complete = await post('/pairing/complete', {}, { origin, cookie });
    assert.equal(complete.status, 200);
    const code = new URL((await complete.json()).redirectTo).searchParams.get('code');
    const result = await token({ grant_type: 'authorization_code', code, code_verifier: verifier,
      client_id: client.client_id, redirect_uri: 'https://client.example/callback', resource });
    assert.equal(result.status, 200);
    const tokens = await result.json();
    const headers = { authorization: `Bearer ${tokens.access_token}` };
    const initialized = await post('/mcp', { jsonrpc: '2.0', id: 1, method: 'initialize' }, headers);
    assert.equal(initialized.status, 200);
    const session = initialized.headers.get('mcp-session-id');
    await initialized.text();
    return { client, tokens, headers: { ...headers, 'mcp-session-id': session } };
  }
  try {
    const enrollment = await post('/devices', candidate);
    assert.equal(enrollment.status, 201);
    const device = await enrollment.json();
    const otherEnrollment = await post('/devices', candidate);
    assert.equal(otherEnrollment.status, 201);
    const otherDevice = await otherEnrollment.json();
    const first = await connect(device, 'Synthetic first client');
    const second = await connect(device, 'Synthetic second client');
    let firstGrant;
    await t.test('native list exposes only owned grant metadata', async () => {
      const response = await request('/grants', { headers: native(device) });
      assert.equal(response.status, 200);
      assert.equal(response.headers.get('cache-control'), 'no-store');
      const page = await response.json();
      assert.equal(page.items.length, 2);
      firstGrant = page.items.find(item => item.clientId === first.client.client_id).id;
      for (const secret of [device.managementToken, first.tokens.access_token, first.tokens.refresh_token, candidate.backendToken]) {
        assert(!JSON.stringify(page).includes(secret));
      }
      assert.equal((await (await request('/grants', { headers: native(otherDevice) })).json()).items.length, 0);
      assert.equal((await request('/grants')).status, 401);
      assert.equal((await request('/grants', { headers: { ...native(device), origin } })).status, 403);
      assert.equal((await request('/grants?cursor=../invalid', { headers: native(device) })).status, 400);
    });
    await t.test('cross-device and browser-origin revoke attempts cannot affect a grant', async () => {
      assert.equal((await post(`/grants/${firstGrant}/revoke`, {}, native(otherDevice))).status, 404);
      assert.equal((await post(`/grants/${firstGrant}/revoke`, {}, { ...native(device), origin })).status, 403);
      const result = await post('/mcp', { jsonrpc: '2.0', id: 2, method: 'tools/list' }, first.headers);
      assert.equal(result.status, 200);
      await result.text();
    });
    await t.test('MCP rejects an unexpected Origin before any backend call', async () => {
      const before = backendCalls;
      const response = await post('/mcp', { jsonrpc: '2.0', id: 3, method: 'tools/list' },
        { ...first.headers, origin: 'https://attacker.example' });
      assert.equal(response.status, 403);
      assert.equal(backendCalls, before);
    });
    await t.test('overlapping refresh exchanges receive a retryable busy response and can recover', async () => {
      const responses = await Promise.all(Array.from({ length: 6 }, () => token({ grant_type: 'refresh_token',
        client_id: second.client.client_id, refresh_token: second.tokens.refresh_token, resource })));
      assert(responses.some(response => response.status === 200));
      assert(responses.some(response => response.status === 503), 'overlapping token mutations must not all enter the provider');
      const successes = [];
      for (const response of responses) {
        const value = await response.json();
        if (response.status === 200) successes.push(value);
        else {
          assert.equal(response.status, 503);
          assert.equal(value.error, 'temporarily_unavailable');
          assert.equal(response.headers.get('retry-after'), '1');
        }
      }
      // A later successful rotation may supersede an earlier response. At least
      // the current returned credential must remain renewable after the burst.
      let recovered = false;
      for (const issued of successes) {
        const response = await token({ grant_type: 'refresh_token', client_id: second.client.client_id,
          refresh_token: issued.refresh_token, resource });
        if (response.status === 200) { second.tokens = await response.json(); recovered = true; break; }
        assert.equal(response.status, 400);
      }
      assert(recovered);
      const result = await post('/mcp', { jsonrpc: '2.0', id: 6, method: 'tools/list' }, {
        ...second.headers, authorization: `Bearer ${second.tokens.access_token}`,
      });
      assert.equal(result.status, 200);
      await result.text();
    });
    await t.test('revoking one grant stops its live stream but keeps the other client connected', async () => {
      const live = await request('/mcp', { headers: first.headers });
      assert.equal(live.status, 200);
      const reader = live.body.getReader();
      assert.match(new TextDecoder().decode((await reader.read()).value), /synthetic-heartbeat/);
      const revoke = await post(`/grants/${firstGrant}/revoke`, {}, native(device));
      assert.equal(revoke.status, 200);
      assert.deepEqual(await revoke.json(), { revoked: true, cleanupPending: false });
      let timer;
      try {
        const stopped = await Promise.race([reader.read(), new Promise((_, reject) => {
          timer = setTimeout(() => reject(new Error('REVOCATION_TIMEOUT')), 4000);
        })]).then(next => next.done, error => { if (error.message === 'REVOCATION_TIMEOUT') throw error; return true; });
        assert(stopped, 'revoked session returned another chunk');
      } finally { clearTimeout(timer); await reader.cancel().catch(() => {}); }
      const before = backendCalls;
      assert([401, 403].includes((await post('/mcp', { jsonrpc: '2.0', id: 4, method: 'tools/list' }, first.headers)).status));
      assert.equal(backendCalls, before);
      assert.equal((await token({ grant_type: 'refresh_token', client_id: first.client.client_id,
        refresh_token: first.tokens.refresh_token, resource })).status, 400);
      const survivor = await post('/mcp', { jsonrpc: '2.0', id: 5, method: 'tools/list' }, second.headers);
      assert.equal(survivor.status, 200);
      await survivor.text();
      assert.equal((await post(`/grants/${firstGrant}/revoke`, {}, native(device))).status, 200);
    });
    await t.test('reauthorization rejects old credentials even if provider data reappears after revocation', async () => {
      const kv = await oauthKV(runtime);
      const [subject, grantId] = second.tokens.access_token.split(':');
      const oldGrantKey = `grant:${subject}:${grantId}`;
      const oldGrant = await kv.get(oldGrantKey);
      const oldTokens = await kv.list({ prefix: `token:${subject}:${grantId}:` });
      const snapshot = new Map([[oldGrantKey, oldGrant]]);
      for (const item of oldTokens.keys) snapshot.set(item.name, await kv.get(item.name));
      const renewed = await connect(device, 'Synthetic replacement', second.client);
      // Recreate stale/late-written library records in the synthetic KV only.
      // The authoritative consent boundary must still reject the old grant.
      for (const [key, value] of snapshot) { assert(value); await kv.put(key, value); }
      const before = backendCalls;
      const old = await post('/mcp', { jsonrpc: '2.0', id: 7, method: 'tools/list' }, second.headers);
      assert.equal(old.status, 401, 'superseded consent asks the client to re-authorize');
      assert.match(old.headers.get('www-authenticate'), /error="invalid_token"/);
      assert.equal(backendCalls, before);
      assert.equal((await token({ grant_type: 'refresh_token', client_id: second.client.client_id,
        refresh_token: second.tokens.refresh_token, resource })).status, 400);
      const current = await post('/mcp', { jsonrpc: '2.0', id: 8, method: 'tools/list' }, renewed.headers);
      assert.equal(current.status, 200);
      await current.text();
    });
  } finally { await runtime.dispose(); }
});
