// SPDX-License-Identifier: Apache-2.0
import assert from 'node:assert/strict';
import test from 'node:test';
import { createRequire } from 'node:module';
import { createHash, randomBytes } from 'node:crypto';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { setTimeout as delay } from 'node:timers/promises';
import { request as httpRequest } from 'node:http';

if (!process.env.WENLAN_RELAY_TOOLCHAIN) throw new Error('WENLAN_RELAY_TOOLCHAIN required');
const requireTool = createRequire(join(resolve(process.env.WENLAN_RELAY_TOOLCHAIN), 'package.json'));
const { Miniflare } = requireTool('miniflare');
const { build } = requireTool('esbuild');
const origin = 'https://wenlan-relay.example';
const candidate = { backendToken: 'b'.repeat(43), space: 'review' };

async function deadline(promise, label, timeoutMs = 4000) {
  let timer;
  try {
    return await Promise.race([promise, new Promise((_, reject) => {
      timer = setTimeout(() => reject(new Error(`${label} deadline`)), timeoutMs);
    })]);
  } finally { clearTimeout(timer); }
}

// A synthetic local MCP peer, not the native Rust client or a hosted reviewer.
function peer(socket) {
  const requests = [];
  const responses = new Map();
  const errors = [];
  const cancelled = new Set();
  const cancellationWaiters = new Map();
  let failNextRequest = false;
  let statusNextRequest = null;
  let heartbeatNextStream = false;
  let partialNextStream = false;
  socket.addEventListener('close', () => {
    for (const response of responses.values()) clearTimeout(response.timer);
    responses.clear();
  });
  const closed = new Promise(resolve => socket.addEventListener('close', resolve, { once: true }));
  const send = frame => socket.send(JSON.stringify({ v: 1, ...frame }));
  socket.addEventListener('message', event => {
    try {
      const frame = JSON.parse(event.data);
      if (frame.type === 'request') {
        requests.push(frame);
        assert.equal(frame.v, 1);
        if (frame.path === '/connector-info' && !frame.headers.authorization) {
          send({ type: 'response', id: frame.id, status: 401, headers: {} });
          send({ type: 'end', id: frame.id }); return;
        }
        assert.equal(frame.headers.authorization, `Bearer ${candidate.backendToken}`);
        assert.equal(frame.headers.cookie, undefined);
        if (frame.path === '/mcp' && failNextRequest) {
          failNextRequest = false;
          send({ type: 'cancel', id: frame.id });
          return;
        }
        if (frame.path === '/mcp' && statusNextRequest !== null) {
          const status = statusNextRequest;
          statusNextRequest = null;
          send({ type: 'response', id: frame.id, status, headers: {} });
          send({ type: 'end', id: frame.id });
          return;
        }
        const stream = frame.path === '/mcp' && frame.method === 'GET';
        let value;
        if (frame.path === '/connector-info') value = { contract_version: 1, server: 'wenlan-mcp',
          tool_profile: 'query-only', authentication: 'bearer', space: candidate.space };
        else {
          assert.equal(frame.path, '/mcp');
          const rpc = frame.method === 'POST' ? JSON.parse(Buffer.from(frame.body, 'base64').toString()) : {};
          value = { jsonrpc: '2.0', id: rpc.id ?? 1, result: { synthetic: true } };
        }
        responses.set(frame.id, { stream, body: stream ? (partialNextStream ? 'data: incomplete' : 'data: synthetic-heartbeat\n\n') : JSON.stringify(value), sent: false,
          heartbeat: stream && heartbeatNextStream, nextSequence: 0, timer: undefined });
        if (stream) heartbeatNextStream = false;
        if (stream) partialNextStream = false;
        send({ type: 'response', id: frame.id, status: 200, headers: {
          'content-type': stream ? 'text/event-stream' : 'application/json',
          ...(frame.path === '/mcp' ? { 'mcp-session-id': 'synthetic-private-session' } : {}),
        } });
      } else if (frame.type === 'credit') {
        const response = responses.get(frame.id);
        if (!response) return;
        assert.equal(frame.seq, response.nextSequence);
        if (response.sent) {
          if (!response.heartbeat) return;
          assert.equal(response.timer, undefined);
          response.timer = setTimeout(() => {
            response.timer = undefined;
            if (responses.get(frame.id) !== response) return;
            response.nextSequence++;
            send({ type: 'chunk', id: frame.id, seq: frame.seq, body: Buffer.from(':\n\n').toString('base64') });
          }, 1000);
          return;
        }
        response.sent = true;
        response.nextSequence++;
        send({ type: 'chunk', id: frame.id, seq: 0, body: Buffer.from(response.body).toString('base64') });
        if (!response.stream) { responses.delete(frame.id); send({ type: 'end', id: frame.id }); }
      } else if (frame.type === 'cancel') {
        clearTimeout(responses.get(frame.id)?.timer);
        responses.delete(frame.id); cancelled.add(frame.id);
        cancellationWaiters.get(frame.id)?.(); cancellationWaiters.delete(frame.id);
      }
      else assert.fail('Unexpected server frame');
    } catch (error) { errors.push(error); socket.close(1000, 'Fixture failed'); }
  });
  socket.accept();
  return { socket, requests, errors, closed,
    failNext() { failNextRequest = true; },
    statusNext(status) { statusNextRequest = status; },
    heartbeatNext() { heartbeatNextStream = true; },
    partialNext() { partialNextStream = true; },
    cancellation(id) {
      if (cancelled.has(id)) return Promise.resolve();
      return new Promise(resolve => cancellationWaiters.set(id, resolve));
    },
  };
}

test('device connection capacity is isolated beyond the former global 64-slot limit', { timeout: 40_000 }, async () => {
  const bundled = await build({ entryPoints: [fileURLToPath(new URL('../src/worker.ts', import.meta.url))],
    bundle: true, write: false, format: 'esm', platform: 'browser', target: 'es2022',
    external: ['cloudflare:workers'], loader: { '.png': 'binary' } });
  const runtime = new Miniflare({ modules: true, script: bundled.outputFiles[0].text,
    compatibilityDate: '2026-09-08', compatibilityFlags: ['enable_request_signal'], host: '127.0.0.1', port: 0,
    bindings: { PUBLIC_ORIGIN: origin }, kvNamespaces: ['OAUTH_KV'],
    durableObjects: { AUTHORITY: { className: 'RelayAuthority', useSQLite: true } },
    outboundService: () => { throw new Error('No external transport allowed'); },
  });
  const peers = [];
  const devices = [];
  const request = (path, init) => runtime.dispatchFetch(`${origin}${path}`, {
    ...init, credentials: 'omit', redirect: 'manual',
  });
  const headersFor = device => ({ authorization: `Bearer ${device.managementToken}`,
    'x-wenlan-device-id': device.id, 'cf-connecting-ip': device.peer });
  async function connect(device) {
    const upgraded = await request('/devices/reverse/connect', { headers: {
      ...headersFor(device), upgrade: 'websocket', 'sec-websocket-protocol': 'wenlan.reverse.v1',
    } });
    assert.equal(upgraded.status, 101, upgraded.status === 101 ? '' : await upgraded.text());
    const connected = peer(upgraded.webSocket); peers.push(connected);
    const connectionId = upgraded.headers.get('x-wenlan-connection-id');
    await deadline((async () => {
      for (;;) {
        const status = await request('/devices/reverse/status', { headers: {
          ...headersFor(device), 'x-wenlan-connection-id': connectionId,
        } });
        assert.equal(status.status, 200);
        if ((await status.json()).connected) return;
        await delay(20);
      }
    })(), 'capacity activation');
    return { ...connected, connectionId };
  }
  try {
    for (let index = 0; index < 65; index++) {
      // Distinct synthetic ingress peers exercise real enrollment without
      // disabling the existing per-IP or global daily quotas.
      const ingress = `192.0.2.${index + 1}`;
      const prepared = await request('/devices/reverse', { method: 'POST',
        headers: { 'content-type': 'application/json', 'cf-connecting-ip': ingress },
        body: JSON.stringify(candidate) });
      assert.equal(prepared.status, 201, await prepared.clone().text());
      const device = { ...await prepared.json(), peer: ingress };
      devices.push(device);
      await connect(device);
    }
    assert.equal(peers.length, 65);
    for (const connected of peers) assert.equal(connected.socket.readyState, 1);
    const original = peers[0];
    const replaced = await connect(devices[0]);
    await deadline(original.closed, 'same-device replacement at capacity');
    assert.notEqual(replaced.connectionId, undefined);
    for (const connected of peers.slice(1)) assert.equal(connected.socket.readyState, 1);
    for (const connected of peers) assert.deepEqual(connected.errors, []);
    const mismatched = await request('/devices/reverse/connect', { headers: {
      ...headersFor(devices[0]), 'x-wenlan-device-id': devices[1].id,
      upgrade: 'websocket', 'sec-websocket-protocol': 'wenlan.reverse.v1',
    } });
    assert.equal(mismatched.status, 401);
    const namespace = await runtime.getDurableObjectNamespace('AUTHORITY');
    const shard = namespace.get(namespace.idFromName(`wenlan-device:${devices[0].id}`));
    const wrongShard = await shard.fetch(`${origin}/devices/reverse/status`, { headers: {
      ...headersFor(devices[1]), 'x-wenlan-connection-id': replaced.connectionId,
    } });
    assert.equal(wrongShard.status, 403, 'a shard must reject another device even through an internal binding');
    const publicBypass = await request('/mcp', { method: 'POST', headers: {
      ...headersFor(devices[0]), 'x-wenlan-connection-id': replaced.connectionId,
      'content-type': 'application/json',
    }, body: JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'initialize' }) });
    assert.equal(publicBypass.status, 401, 'routing headers and device credentials cannot bypass public OAuth');
  } finally {
    for (const connected of peers) { try { connected.socket.close(); } catch { /* Already closed. */ } }
    await runtime.dispose();
  }
});

// Includes the bounded 45-second frame flood plus the sequential lifecycle cases.
test('actual Worker reverse enrollment, OAuth routing, reconnect and revocation use no tunnel', { timeout: 120_000 }, async t => {
  const bundled = await build({ entryPoints: [fileURLToPath(new URL('../src/worker.ts', import.meta.url))],
    bundle: true, write: false, format: 'esm', platform: 'browser', target: 'es2022',
    external: ['cloudflare:workers'], loader: { '.png': 'binary' } });
  let outboundCalls = 0;
  const runtime = new Miniflare({ modules: true, script: bundled.outputFiles[0].text,
    compatibilityDate: '2026-09-08', compatibilityFlags: ['enable_request_signal'], host: '127.0.0.1', port: 0,
    unsafeDirectSockets: [{ host: '127.0.0.1', port: 0, proxy: true }],
    bindings: { PUBLIC_ORIGIN: origin }, kvNamespaces: ['OAUTH_KV'],
    durableObjects: { AUTHORITY: { className: 'RelayAuthority', useSQLite: true } },
    outboundService: () => { outboundCalls++; return new Response(null, { status: 502 }); },
  });
  const peers = [];
  const request = (path, init) => runtime.dispatchFetch(`${origin}${path}`, { ...init, credentials: 'omit', redirect: 'manual' });
  const post = (path, value, headers = {}) => request(path, { method: 'POST',
    headers: { 'content-type': 'application/json', ...headers }, body: JSON.stringify(value) });
  try {
    const prepared = await post('/devices/reverse', candidate);
    assert.equal(prepared.status, 201, await prepared.clone().text());
    const device = await prepared.json();
    assert(device.pendingUntil < device.expiresAt);
    const deviceHeaders = { authorization: `Bearer ${device.managementToken}`, 'x-wenlan-device-id': device.id };
    const upgradeHeaders = { ...deviceHeaders, upgrade: 'websocket', 'sec-websocket-protocol': 'wenlan.reverse.v1' };
    await t.test('upgrade requires native credentials and an exact wire protocol', async () => {
      assert.equal((await request('/devices/reverse/connect', { headers: { upgrade: 'websocket' } })).status, 401);
      assert.equal((await request('/devices/reverse/connect', { headers: { ...upgradeHeaders, origin } })).status, 403);
      assert.equal((await request('/devices/reverse/connect', { headers: { ...upgradeHeaders, cookie: 'x' } })).status, 403);
      assert.equal((await request('/devices/reverse/connect', { headers: { ...upgradeHeaders, 'sec-websocket-protocol': 'other' } })).status, 400);
      assert.equal((await request('/devices/reverse/connect?token=forbidden', { headers: upgradeHeaders })).status, 400);
    });

    const registration = await post('/oauth/register', { client_name: 'Reverse synthetic client',
      redirect_uris: ['https://client.example/callback'], grant_types: ['authorization_code', 'refresh_token'],
      response_types: ['code'], token_endpoint_auth_method: 'none' });
    assert.equal(registration.status, 201);
    const client = await registration.json();
    const verifier = randomBytes(32).toString('base64url');
    const params = new URLSearchParams({ response_type: 'code', client_id: client.client_id,
      redirect_uri: 'https://client.example/callback', resource: `${origin}/mcp`, scope: 'wenlan:query',
      code_challenge_method: 'S256', code_challenge: createHash('sha256').update(verifier).digest('base64url') });
    const authorization = await request(`/authorize?${params}`, { redirect: 'manual' });
    assert.equal(authorization.status, 303);
    const cookie = authorization.headers.get('set-cookie').split(';')[0];
    const pairingId = cookie.split('=')[1].split('.')[0];
    const consent = { approved: true, clientId: client.client_id, resource: `${origin}/mcp`, space: candidate.space };
    assert.equal((await post(`/pairings/${pairingId}/approve`, consent, deviceHeaders)).status, 401,
      'pending management credentials must not approve OAuth');

    async function connect() {
      const upgraded = await request('/devices/reverse/connect', { headers: upgradeHeaders });
      assert.equal(upgraded.status, 101, upgraded.status === 101 ? '' : await upgraded.text());
      assert.equal(upgraded.headers.get('sec-websocket-protocol'), 'wenlan.reverse.v1');
      const connectionId = upgraded.headers.get('x-wenlan-connection-id');
      assert.match(connectionId, /^[a-f0-9]{64}$/);
      const connected = peer(upgraded.webSocket); peers.push(connected);
      for (const wait of [0, 10, 20, 40, 80, 160, 320, 640, 1280]) {
        if (wait) await delay(wait);
        const status = await request('/devices/reverse/status', { headers: {
          ...deviceHeaders, 'x-wenlan-connection-id': connectionId,
        } });
        assert.equal(status.status, 200);
        if ((await status.json()).connected) return { ...connected, connectionId };
      }
      assert.deepEqual(connected.errors, []);
      assert.fail('Verified reverse activation deadline');
    }
    let connected = await connect();
    assert.equal(connected.requests.filter(frame => frame.path === '/connector-info').length, 2);
    assert.equal((await post(`/pairings/${pairingId}/approve`, consent, deviceHeaders)).status, 200);
    const completed = await post('/pairing/complete', {}, { cookie, origin });
    assert.equal(completed.status, 200);
    const callback = new URL((await completed.json()).redirectTo);
    const exchange = await request('/oauth/token', { method: 'POST', headers: { 'content-type': 'application/x-www-form-urlencoded' },
      body: new URLSearchParams({ grant_type: 'authorization_code', code: callback.searchParams.get('code'),
        code_verifier: verifier, client_id: client.client_id, redirect_uri: 'https://client.example/callback', resource: `${origin}/mcp` }).toString() });
    assert.equal(exchange.status, 200);
    const tokens = await exchange.json();
    const oauthHeaders = { authorization: `Bearer ${tokens.access_token}` };
    async function initialize() {
      const response = await post('/mcp', { jsonrpc: '2.0', id: 1, method: 'initialize' }, oauthHeaders);
      assert.equal(response.status, 200, await response.clone().text()); await response.text();
      const session = response.headers.get('mcp-session-id');
      assert.notEqual(session, 'synthetic-private-session');
      return { ...oauthHeaders, 'mcp-session-id': session };
    }
    let mcpHeaders = await initialize();
    const query = { jsonrpc: '2.0', id: 2, method: 'tools/call', params: { name: 'brief', arguments: {} } };
    const queried = await post('/mcp', query, mcpHeaders);
    assert.equal(queried.status, 200); await queried.text();
    await t.test('legacy refresh and renew reject reverse devices without probing or closing their socket', async () => {
      const beforeCalls = outboundCalls;
      for (const path of ['/devices/refresh', '/devices/renew']) {
        const rejected = await post(path, {
          ...candidate, tunnelOrigin: 'https://legacy.trycloudflare.com', expectedGeneration: 0,
        }, deviceHeaders);
        assert.equal(rejected.status, 401);
        await rejected.text();
        assert.equal(outboundCalls, beforeCalls);
        const stillUsable = await post('/mcp', query, mcpHeaders);
        assert.equal(stillUsable.status, 200);
        await stillUsable.text();
        assert.deepEqual(connected.errors, []);
      }
    });
    for (const params of [{ name: 'capture', arguments: {} }, { name: 'brief', arguments: { space: 'private' } }]) {
      assert.equal((await post('/mcp', { ...query, params }, mcpHeaders)).status, 403);
    }

    await t.test('a native upstream failure affects only its request and exposes no private error', async () => {
      connected.failNext();
      const failed = await post('/mcp', query, mcpHeaders);
      assert.equal(failed.status, 200);
      assert.equal(failed.headers.get('cache-control'), 'no-store');
      assert.equal(failed.headers.get('www-authenticate'), null);
      assert.deepEqual(await failed.json(), { jsonrpc: '2.0', id: query.id, result: {
        content: [{ type: 'text', text: 'Wenlan could not reach your local device. Make sure Wenlan is running and the device is online, then try again. This does not mean your authorization was revoked.' }],
        isError: true,
      } });
      const next = await post('/mcp', query, mcpHeaders);
      assert.equal(next.status, 200); await next.text();
      assert.deepEqual(connected.errors, []);
    });

    await t.test('an unmarked HTTP503 stays a sanitized HTTP502', async () => {
      connected.statusNext(503);
      const unmarked = await post('/mcp', query, mcpHeaders);
      assert.equal(unmarked.status, 502);
      assert.equal(unmarked.headers.get('www-authenticate'), null);
      assert.equal(unmarked.headers.get('x-wenlan-upstream-unavailable'), null);
      const text = await unmarked.text();
      assert.equal(text, '{"error":"Connector unavailable"}');
      assert(!text.includes('result'));
      assert(!text.includes('isError'));
      const recovered = await post('/mcp', query, mcpHeaders);
      assert.equal(recovered.status, 200); await recovered.text();
      assert.deepEqual(connected.errors, []);
    });

    await t.test('reconnect preserves consent but invalidates the old MCP session', async () => {
      const old = connected;
      old.socket.close(1000, 'Synthetic reconnect');
      await deadline(old.closed, 'old socket close');
      const offline = await post('/mcp', query, mcpHeaders);
      assert.equal(offline.status, 200);
      const offlineResult = await offline.json();
      assert.equal(offlineResult.id, query.id);
      assert.equal(offlineResult.result.isError, true);
      assert.match(offlineResult.result.content[0].text, /could not reach your local device/);
      assert.equal(offline.headers.get('www-authenticate'), null);
      assert.equal(offline.headers.get('x-wenlan-upstream-unavailable'), null);
      const unavailableInit = await post('/mcp', { jsonrpc: '2.0', id: 71, method: 'initialize' }, oauthHeaders);
      assert.equal(unavailableInit.status, 200);
      assert.equal(unavailableInit.headers.get('mcp-session-id'), null);
      const initError = await unavailableInit.json();
      assert.equal(initError.id, 71);
      assert.equal(initError.error.code, -32000);
      assert.match(initError.error.message, /could not reach your local device/);
      assert.equal(initError.result, undefined);
      connected = await connect();
      assert.notEqual(connected.connectionId, old.connectionId);
      assert.equal((await post('/mcp', query, mcpHeaders)).status, 404);
      mcpHeaders = await initialize();
    });

    await t.test('late cancelled frames remain bounded by the live channel budget', async () => {
      const response = await request('/mcp', { headers: mcpHeaders });
      assert.equal(response.status, 200);
      const reader = response.body.getReader();
      await deadline(reader.read(), 'cancel fixture event');
      const id = connected.requests.at(-1).id;
      const deleted = await request('/mcp', { method: 'DELETE', headers: mcpHeaders });
      assert.equal(deleted.status, 200);
      await deleted.text();
      await deadline(connected.cancellation(id), 'upstream cancellation');
      await reader.cancel().catch(() => {});
      const late = JSON.stringify({ v: 1, type: 'end', id });
      for (let index = 0; index < 10; index++) connected.socket.send(late);
      mcpHeaders = await initialize();
      const stillUsable = await post('/mcp', query, mcpHeaders);
      assert.equal(stillUsable.status, 200); await stillUsable.text();
      // Yield between batches so the synthetic peer can receive the close frame
      // instead of queuing thousands more sends behind an already closed channel.
      const flooded = connected;
      let sent = 0;
      let closed = false;
      flooded.closed.then(() => { closed = true; });
      try {
        await deadline((async () => {
          // This is a frame-count bound, not the four-second cancellation SLA.
          // Shared Linux runners need time to deliver 16k+ WebSocket events.
          // Stay below one minute so 32,769 frames cover at most one rollover.
          while (!closed && sent < 32_769) {
            const end = Math.min(sent + 128, 32_769);
            for (; sent < end; sent++) flooded.socket.send(late);
            await delay(0);
          }
          await flooded.closed;
        })(), 'frame budget close', 45_000);
      } finally {
        closed = true;
        try { flooded.socket.close(); } catch { /* Already closed. */ }
        connected = await connect();
        mcpHeaders = await initialize();
      }
    });

    await t.test('an explicit TCP reset cancels the upstream request within four seconds', async () => {
      const direct = await runtime.unsafeGetDirectURL();
      const id = await new Promise((resolve, reject) => {
        let reset = false;
        const request = httpRequest({ hostname: direct.hostname, port: direct.port,
          path: `${origin}/mcp`, headers: mcpHeaders, agent: false }, response => {
          if (response.statusCode !== 200) {
            response.resume(); reject(new Error(`Reset fixture HTTP ${response.statusCode}`)); return;
          }
          response.on('error', error => { if (!reset) reject(error); });
          response.once('data', () => {
            const id = connected.requests.at(-1).id;
            reset = true;
            response.socket.resetAndDestroy();
            resolve(id);
          });
        });
        request.setTimeout(4000, () => request.destroy(new Error('Reset fixture deadline')));
        request.on('error', error => { if (!reset) reject(error); });
        request.end();
      });
      await deadline(connected.cancellation(id), 'TCP reset propagation');
    });

    await t.test('one-second SSE heartbeat propagates downstream abort within four seconds', async () => {
      connected.heartbeatNext();
      const abort = new AbortController();
      const response = await request('/mcp', { headers: mcpHeaders, signal: abort.signal });
      assert.equal(response.status, 200);
      const reader = response.body.getReader();
      try {
        await deadline(reader.read(), 'heartbeat fixture event');
        const id = connected.requests.at(-1).id;
        abort.abort();
        await reader.cancel().catch(() => {});
        await deadline(connected.cancellation(id), 'heartbeat abort propagation');
      } finally {
        await reader.cancel().catch(() => {});
        const deleted = await request('/mcp', { method: 'DELETE', headers: mcpHeaders });
        assert.equal(deleted.status, 200); await deleted.text();
        mcpHeaders = await initialize();
      }
    });

    await t.test('relay keepalive cancels an idle upstream request within four seconds of HTTP abort', async () => {
      const abort = new AbortController();
      const response = await request('/mcp', { headers: mcpHeaders, signal: abort.signal });
      assert.equal(response.status, 200);
      const reader = response.body.getReader();
      try {
        await deadline(reader.read(), 'abort fixture event');
        const id = connected.requests.at(-1).id;
        abort.abort();
        await reader.cancel().catch(() => {});
        await deadline(connected.cancellation(id), 'downstream abort propagation');
      } finally {
        await reader.cancel().catch(() => {});
        const deleted = await request('/mcp', { method: 'DELETE', headers: mcpHeaders });
        assert.equal(deleted.status, 200); await deleted.text();
        mcpHeaders = await initialize();
      }
    });

    await t.test('a partial SSE event stays unmodified and abort cleanup retains the real transport deadline', async () => {
      connected.partialNext();
      const abort = new AbortController();
      const started = performance.now();
      const response = await request('/mcp', { headers: mcpHeaders, signal: abort.signal });
      assert.equal(response.status, 200);
      const reader = response.body.getReader();
      try {
        assert.equal(new TextDecoder().decode((await deadline(reader.read(), 'partial deadline fixture event')).value), 'data: incomplete');
        const id = connected.requests.at(-1).id;
        abort.abort();
        await reader.cancel().catch(() => {});
        // Injecting a comment here would corrupt a partial data field. Retain
        // the independent 30s transport cap without shortening it for this test.
        await deadline(connected.cancellation(id), 'silent transport cleanup', 35_000);
        assert(performance.now() - started < 35_000, 'silent request must not outlive the transport cap plus scheduling margin');
        const deleted = await request('/mcp', { method: 'DELETE', headers: mcpHeaders });
        assert.equal(deleted.status, 200);
        await deleted.text();
        mcpHeaders = await initialize();
      } finally {
        await reader.cancel().catch(() => {});
      }
    });

    await t.test('revocation closes the socket and streaming response without leaking more data', async () => {
      const response = await request('/mcp', { headers: mcpHeaders });
      assert.equal(response.status, 200);
      const reader = response.body.getReader();
      assert.match(new TextDecoder().decode((await deadline(reader.read(), 'first event')).value), /synthetic-heartbeat/);
      assert.equal((await post('/devices/revoke', {}, deviceHeaders)).status, 200);
      await deadline(connected.closed, 'revoked socket close');
      let ended;
      try { ended = (await deadline(reader.read(), 'revoked stream')).done; }
      catch (error) { if (/deadline/.test(error.message)) throw error; ended = true; }
      assert.equal(ended, true);
      await reader.cancel().catch(() => {});
      assert.equal((await post('/mcp', query, mcpHeaders)).status, 401);
      assert.equal((await request('/devices/reverse/connect', { headers: upgradeHeaders })).status, 401);
    });

    await t.test('tunnel and reverse preparation share the same enrollment quota', async () => {
      // Ten per address per hour across both routes: alternating them reaches
      // the limit within eleven attempts, which separate quotas would not.
      let limited;
      for (let n = 0; n < 11 && !limited; n++) {
        const reverse = n % 2 === 1;
        const response = reverse ? await post('/devices/reverse', { ...candidate, backendToken: 'short' })
          : await post('/devices', {});
        if (response.status === 429) limited = response;
        else assert.equal(response.status, reverse ? 422 : 400);
      }
      assert(limited, 'the shared enrollment quota was never reached');
      assert(Number(limited.headers.get('retry-after')) > 0);
    });
    assert.equal(outboundCalls, 0);
    for (const connected of peers) assert.deepEqual(connected.errors, []);
  } finally {
    for (const connected of peers) { try { connected.socket.close(); } catch { /* Already closed. */ } }
    await runtime.dispose();
  }
});
