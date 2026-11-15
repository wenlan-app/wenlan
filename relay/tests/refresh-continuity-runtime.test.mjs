// SPDX-License-Identifier: Apache-2.0
import assert from 'node:assert/strict';
import test from 'node:test';
import { oauthKV } from './fixtures/oauth-storage.mjs';
import { createRequire } from 'node:module';
import { createHash, randomBytes } from 'node:crypto';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { setTimeout as delay } from 'node:timers/promises';

if (!process.env.WENLAN_RELAY_TOOLCHAIN) throw new Error('WENLAN_RELAY_TOOLCHAIN required');
const requireTool = createRequire(join(resolve(process.env.WENLAN_RELAY_TOOLCHAIN), 'package.json'));
const { Miniflare } = requireTool('miniflare');
const { build } = requireTool('esbuild');
const origin = 'https://wenlan-relay.example';
const resource = `${origin}/mcp`;
const redirectUri = 'https://client.example/callback';
const candidate = { backendToken: 'b'.repeat(43), space: 'review' };

async function deadline(promise, label, timeoutMs = 4000) {
  let timer;
  try {
    return await Promise.race([promise, new Promise((_, reject) => {
      timer = setTimeout(() => reject(new Error(`${label} deadline`)), timeoutMs);
    })]);
  } finally { clearTimeout(timer); }
}

// A synthetic local MCP peer answering unary requests only.
function peer(socket) {
  const pending = new Map();
  const closed = new Promise(resolve => socket.addEventListener('close', resolve, { once: true }));
  const send = frame => socket.send(JSON.stringify({ v: 1, ...frame }));
  socket.addEventListener('message', event => {
    const frame = JSON.parse(event.data);
    if (frame.type === 'request') {
      if (frame.headers.authorization !== `Bearer ${candidate.backendToken}`) {
        send({ type: 'response', id: frame.id, status: 401, headers: {} });
        send({ type: 'end', id: frame.id }); return;
      }
      const rpc = frame.method === 'POST' ? JSON.parse(Buffer.from(frame.body, 'base64').toString()) : {};
      pending.set(frame.id, JSON.stringify(frame.path === '/connector-info'
        ? { contract_version: 1, server: 'wenlan-mcp', tool_profile: 'query-only', authentication: 'bearer', space: candidate.space }
        : { jsonrpc: '2.0', id: rpc.id ?? 1, result: { synthetic: true } }));
      send({ type: 'response', id: frame.id, status: 200, headers: { 'content-type': 'application/json',
        ...(frame.path === '/mcp' ? { 'mcp-session-id': 'synthetic-private-session' } : {}) } });
    } else if (frame.type === 'credit' && pending.has(frame.id)) {
      send({ type: 'chunk', id: frame.id, seq: 0, body: Buffer.from(pending.get(frame.id)).toString('base64') });
      pending.delete(frame.id);
      send({ type: 'end', id: frame.id });
    } else if (frame.type === 'cancel') pending.delete(frame.id);
  });
  socket.accept();
  return { socket, closed };
}

// Each relay-side refresh rejection names its own cause. Codex logged
// "invalid_grant: Invalid refresh token", which only the library's rotation
// check produces: the grant still existed and every Wenlan check was unreached.
test('a paired grant survives a device outage and reconnect; each refresh rejection names its cause', { timeout: 60_000 }, async t => {
  const bundled = await build({ entryPoints: [fileURLToPath(new URL('./fixtures/oauth-storage-worker.ts', import.meta.url))],
    bundle: true, write: false, format: 'esm', platform: 'browser', target: 'es2022',
    external: ['cloudflare:workers'], loader: { '.png': 'binary' } });
  const runtime = new Miniflare({ modules: true, script: bundled.outputFiles[0].text,
    compatibilityDate: '2026-09-08', compatibilityFlags: ['enable_request_signal'], host: '127.0.0.1', port: 0,
    bindings: { PUBLIC_ORIGIN: origin }, kvNamespaces: ['OAUTH_KV'],
    durableObjects: { AUTHORITY: { className: 'RelayAuthority', useSQLite: true } },
    outboundService: () => new Response(null, { status: 502 }),
  });
  const sockets = [];
  const request = (path, init) => runtime.dispatchFetch(`${origin}${path}`, { ...init, credentials: 'omit', redirect: 'manual' });
  const post = (path, value, headers = {}) => request(path, { method: 'POST',
    headers: { 'content-type': 'application/json', ...headers }, body: JSON.stringify(value) });
  const refresh = (client, refreshToken) => request('/oauth/token', { method: 'POST',
    headers: { 'content-type': 'application/x-www-form-urlencoded' },
    body: new URLSearchParams({ grant_type: 'refresh_token', client_id: client.client_id,
      refresh_token: refreshToken, resource }).toString() });
  const rejection = async (response, description) => {
    assert.equal(response.status, 400);
    assert.deepEqual(await response.json(), { error: 'invalid_grant', error_description: description });
  };
  const initialize = accessToken => post('/mcp', { jsonrpc: '2.0', id: 1, method: 'initialize' },
    { authorization: `Bearer ${accessToken}` });
  try {
    const prepared = await post('/devices/reverse', candidate);
    assert.equal(prepared.status, 201, await prepared.clone().text());
    const device = await prepared.json();
    const deviceHeaders = { authorization: `Bearer ${device.managementToken}`, 'x-wenlan-device-id': device.id };
    // Reconnect reuses the same enrollment and management credential, as the app does.
    async function connectDevice() {
      const upgraded = await request('/devices/reverse/connect', { headers: {
        ...deviceHeaders, upgrade: 'websocket', 'sec-websocket-protocol': 'wenlan.reverse.v1' } });
      assert.equal(upgraded.status, 101, upgraded.status === 101 ? '' : await upgraded.text());
      const connected = peer(upgraded.webSocket);
      sockets.push(connected.socket);
      const connectionId = upgraded.headers.get('x-wenlan-connection-id');
      await deadline((async () => {
        for (;;) {
          const status = await request('/devices/reverse/status', { headers: { ...deviceHeaders, 'x-wenlan-connection-id': connectionId } });
          assert.equal(status.status, 200);
          if ((await status.json()).connected) return;
          await delay(20);
        }
      })(), 'reverse activation');
      return connected;
    }
    async function pair(registered) {
      let client = registered;
      if (!client) {
        const registration = await post('/oauth/register', { client_name: 'Synthetic refresh client',
          redirect_uris: [redirectUri], grant_types: ['authorization_code', 'refresh_token'],
          response_types: ['code'], token_endpoint_auth_method: 'none' });
        assert.equal(registration.status, 201);
        client = await registration.json();
      }
      const verifier = randomBytes(32).toString('base64url');
      const authorization = await request(`/authorize?${new URLSearchParams({ response_type: 'code',
        client_id: client.client_id, redirect_uri: redirectUri, resource, scope: 'wenlan:query',
        code_challenge_method: 'S256', code_challenge: createHash('sha256').update(verifier).digest('base64url') })}`);
      assert.equal(authorization.status, 303);
      const cookie = authorization.headers.get('set-cookie').split(';')[0];
      const pairingId = cookie.split('=')[1].split('.')[0];
      assert.equal((await post(`/pairings/${pairingId}/approve`, { approved: true, clientId: client.client_id,
        resource, space: candidate.space }, deviceHeaders)).status, 200);
      const completed = await post('/pairing/complete', {}, { cookie, origin });
      assert.equal(completed.status, 200);
      const code = new URL((await completed.json()).redirectTo).searchParams.get('code');
      const exchanged = await request('/oauth/token', { method: 'POST',
        headers: { 'content-type': 'application/x-www-form-urlencoded' },
        body: new URLSearchParams({ grant_type: 'authorization_code', code, code_verifier: verifier,
          client_id: client.client_id, redirect_uri: redirectUri, resource }).toString() });
      assert.equal(exchanged.status, 200);
      return { client, tokens: await exchanged.json() };
    }
    // Models the 900-second access TTL elapsing without waiting for it.
    async function expireAccess(tokens) {
      const kv = await oauthKV(runtime);
      const [subject, grantId] = tokens.access_token.split(':');
      const entries = await kv.list({ prefix: `token:${subject}:${grantId}:` });
      assert(entries.keys.length > 0);
      for (const entry of entries.keys) {
        const value = await kv.get(entry.name, 'json');
        await kv.put(entry.name, JSON.stringify({ ...value, expiresAt: Math.floor(Date.now() / 1000) - 1 }));
      }
    }

    let link = await connectDevice();
    const desktop = await pair();
    const initialized = await initialize(desktop.tokens.access_token);
    assert.equal(initialized.status, 200);
    await initialized.text();

    await t.test('refresh keeps working through an outage longer than the access TTL and a reverse reconnect', async () => {
      link.socket.close(1000, 'Synthetic app quit');
      await deadline(link.closed, 'device offline');
      await expireAccess(desktop.tokens);
      const expired = await initialize(desktop.tokens.access_token);
      assert.equal(expired.status, 401, 'an expired access token must prompt a refresh, not a re-pair');
      await expired.body?.cancel();
      const whileOffline = await refresh(desktop.client, desktop.tokens.refresh_token);
      assert.equal(whileOffline.status, 200, 'a disconnected device neither revokes nor rotates the grant');
      desktop.tokens = await whileOffline.json();
      const unreachable = await initialize(desktop.tokens.access_token);
      assert.equal(unreachable.status, 200);
      assert.match((await unreachable.json()).error.message, /could not reach your local device/);

      link = await connectDevice();
      await expireAccess(desktop.tokens);
      const afterReconnect = await refresh(desktop.client, desktop.tokens.refresh_token);
      assert.equal(afterReconnect.status, 200);
      desktop.tokens = await afterReconnect.json();
      const resumed = await initialize(desktop.tokens.access_token);
      assert.equal(resumed.status, 200);
      await resumed.text();
    });

    await t.test('only a superseded refresh token yields the error Codex reported', async () => {
      const shared = desktop.tokens.refresh_token;
      const first = await refresh(desktop.client, shared);
      assert.equal(first.status, 200);
      const firstTokens = await first.json();
      // A second holder of the same stored credential replays it. The library
      // accepts the immediately previous token once more and supersedes the
      // first holder's newer token.
      const second = await refresh(desktop.client, shared);
      assert.equal(second.status, 200);
      const secondTokens = await second.json();
      await rejection(await refresh(desktop.client, firstTokens.refresh_token), 'Invalid refresh token');
      const current = await refresh(desktop.client, secondTokens.refresh_token);
      assert.equal(current.status, 200);
      desktop.tokens = await current.json();
    });

    // Restores stale library records to show the authority still rejects them
    // when eager KV cleanup has not happened yet.
    async function snapshotGrant(tokens) {
      const kv = await oauthKV(runtime);
      const [subject, grantId] = tokens.refresh_token.split(':');
      const key = `grant:${subject}:${grantId}`;
      const value = await kv.get(key);
      assert(value);
      return () => kv.put(key, value);
    }

    await t.test('re-pairing one DCR client invalidates only that client, never as a rotation failure', async () => {
      const other = await pair();
      const replaced = desktop.tokens;
      const restore = await snapshotGrant(replaced);
      const renewed = await pair(desktop.client);
      // completeAuthorization (src/oauth.ts) revokes the same client's earlier grants.
      await rejection(await refresh(desktop.client, replaced.refresh_token), 'Grant not found');
      await restore();
      await rejection(await refresh(desktop.client, replaced.refresh_token), 'Wenlan authorization is no longer valid');
      const unaffected = await refresh(other.client, other.tokens.refresh_token);
      assert.equal(unaffected.status, 200, 'another client keeps its grant');
      const current = await refresh(desktop.client, renewed.tokens.refresh_token);
      assert.equal(current.status, 200);
      desktop.tokens = await current.json();
    });

    await t.test('device revocation is reported as revoked, never as a rotation failure', async () => {
      assert.equal((await post('/devices/revoke', {}, deviceHeaders)).status, 200);
      await rejection(await refresh(desktop.client, desktop.tokens.refresh_token), 'Wenlan authorization has been revoked');
    });
  } finally {
    for (const socket of sockets) { try { socket.close(); } catch { /* Already closed. */ } }
    await runtime.dispose();
  }
});
