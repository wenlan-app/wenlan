// SPDX-License-Identifier: Apache-2.0
import assert from 'node:assert/strict';
import test from 'node:test';
import { createRequire } from 'node:module';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createHash, randomBytes } from 'node:crypto';
import { setTimeout as delay } from 'node:timers/promises';

if (!process.env.WENLAN_RELAY_TOOLCHAIN) throw new Error('WENLAN_RELAY_TOOLCHAIN required');
const requireTool = createRequire(join(resolve(process.env.WENLAN_RELAY_TOOLCHAIN), 'package.json'));
const { Miniflare } = requireTool('miniflare');
const { build } = requireTool('esbuild');
const origin = 'https://wenlan-relay.example';
const resource = `${origin}/mcp`;
const callback = 'https://client.example/callback';
const candidate = { tunnelOrigin: 'https://synthetic.trycloudflare.com', backendToken: 'b'.repeat(43), space: 'review' };
const WRITE_TOOLS = ['read_page', 'list_pages', 'capture', 'write_page', 'rename_page', 'forget', 'delete_page'];

async function bundle() {
  const result = await build({ entryPoints: [fileURLToPath(new URL('../src/worker.ts', import.meta.url))],
    bundle: true, write: false, format: 'esm', platform: 'browser', target: 'es2022',
    external: ['cloudflare:workers'], loader: { '.png': 'binary' } });
  return result.outputFiles[0].text;
}

/** Shared by both transports: OAuth through the real Worker, approval with an optional access. */
function harness(runtime) {
  const request = (path, init) => runtime.dispatchFetch(`${origin}${path}`, { ...init, credentials: 'omit', redirect: 'manual' });
  const post = (path, data, headers = {}) => request(path, { method: 'POST',
    headers: { 'content-type': 'application/json', ...headers }, body: JSON.stringify(data) });
  const native = device => ({ authorization: `Bearer ${device.managementToken}`, 'x-wenlan-device-id': device.id });
  const token = data => request('/oauth/token', { method: 'POST',
    headers: { 'content-type': 'application/x-www-form-urlencoded' }, body: new URLSearchParams(data).toString() });
  const register = async name => {
    const registration = await post('/oauth/register', { client_name: name, redirect_uris: [callback],
      grant_types: ['authorization_code', 'refresh_token'], response_types: ['code'], token_endpoint_auth_method: 'none' });
    assert.equal(registration.status, 201);
    return registration.json();
  };
  /** Starts a pairing; the caller approves it (or not) and completes it. */
  async function start(client) {
    const verifier = randomBytes(32).toString('base64url');
    const params = new URLSearchParams({ response_type: 'code', client_id: client.client_id, redirect_uri: callback,
      resource, scope: 'wenlan:query', code_challenge_method: 'S256',
      code_challenge: createHash('sha256').update(verifier).digest('base64url') });
    const authorization = await request(`/authorize?${params}`);
    assert.equal(authorization.status, 303);
    const cookie = authorization.headers.get('set-cookie').split(';')[0];
    return { cookie, verifier, pairingId: cookie.split('=')[1].split('.')[0] };
  }
  const consent = (client, extra = {}) => ({ approved: true, clientId: client.client_id, resource, space: candidate.space, ...extra });
  async function finish(client, pairing) {
    const complete = await post('/pairing/complete', {}, { origin, cookie: pairing.cookie });
    assert.equal(complete.status, 200);
    const code = new URL((await complete.json()).redirectTo).searchParams.get('code');
    const result = await token({ grant_type: 'authorization_code', code, code_verifier: pairing.verifier,
      client_id: client.client_id, redirect_uri: callback, resource });
    assert.equal(result.status, 200);
    return result.json();
  }
  /** access: undefined leaves the field out, as an older desktop build does. */
  async function connect(device, name, access) {
    const client = await register(name);
    const pairing = await start(client);
    const approval = await post(`/pairings/${pairing.pairingId}/approve`,
      consent(client, access === undefined ? {} : { access }), native(device));
    assert.equal(approval.status, 200, await approval.clone().text());
    const tokens = await finish(client, pairing);
    const headers = { authorization: `Bearer ${tokens.access_token}` };
    const initialized = await post('/mcp', { jsonrpc: '2.0', id: 1, method: 'initialize' }, headers);
    assert.equal(initialized.status, 200, await initialized.clone().text());
    const session = initialized.headers.get('mcp-session-id');
    await initialized.text();
    const call = async (name, args = {}, extra = {}) => post('/mcp', { jsonrpc: '2.0', id: 2, method: 'tools/call',
      params: { name, arguments: args } }, { ...headers, 'mcp-session-id': session, ...extra });
    return { client, tokens, headers, call };
  }
  return { request, post, native, token, register, start, consent, finish, connect };
}

// Counters are per UTC hour; do not start a run that could straddle the change.
async function clearOfHourBoundary() {
  const left = 3_600_000 - (Date.now() % 3_600_000);
  if (left < 90_000) await delay(left + 1000);
}

test('actual Worker: per-app write grants over the tunnel', { timeout: 120_000 }, async t => {
  await clearOfHourBoundary();
  const lines = [];
  const backend = [];
  const runtime = new Miniflare({ modules: true, script: await bundle(),
    compatibilityDate: '2026-09-08', host: '127.0.0.1', port: 0,
    bindings: { PUBLIC_ORIGIN: origin }, kvNamespaces: ['OAUTH_KV'],
    durableObjects: { AUTHORITY: { className: 'RelayAuthority', useSQLite: true } },
    structuredWorkerdLogs: true, handleStructuredLogs: ({ message }) => { lines.push(message); },
    outboundService: async request => {
      const url = new URL(request.url);
      if (url.origin !== candidate.tunnelOrigin) return new Response(null, { status: 502 });
      if (!request.headers.has('authorization')) return new Response(null, { status: 401 });
      if (url.pathname === '/connector-info') return Response.json({ contract_version: 1, server: 'wenlan-mcp',
        tool_profile: 'query-only', authentication: 'bearer', space: candidate.space });
      const rpc = await request.json();
      backend.push({ tool: rpc.params?.name, access: request.headers.get('x-wenlan-access') });
      return Response.json({ jsonrpc: '2.0', id: rpc.id, result: { synthetic: true } }, {
        headers: rpc.method === 'initialize' ? { 'mcp-session-id': randomBytes(16).toString('hex') } : {},
      });
    },
  });
  const h = harness(runtime);
  const logged = () => lines.flatMap(line => {
    const start = line.indexOf('{');
    try { const value = JSON.parse(line.slice(start)); return value.event === 'relay_request' ? [value] : []; } catch { return []; }
  });
  const grants = async device => Object.fromEntries((await (await h.request('/grants', { headers: h.native(device) })).json())
    .items.map(item => [item.clientId, item.access]));
  const lastBackend = () => backend.at(-1);
  try {
    const enrollment = await h.post('/devices', candidate);
    assert.equal(enrollment.status, 201);
    const device = await enrollment.json();

    await t.test('approve accepts query or write, a missing access means query, and anything else is a 400 that spends nothing', async () => {
      const client = await h.register('Parsing client');
      const pairing = await h.start(client);
      const approve = body => h.post(`/pairings/${pairing.pairingId}/approve`, body, h.native(device));
      for (const bad of ['admin', 'WRITE', 'Query', '', null, true, false, 1, 0, ['write'], { access: 'write' }]) {
        assert.equal((await approve(h.consent(client, { access: bad }))).status, 400, JSON.stringify(bad));
      }
      assert.equal((await approve({ ...h.consent(client, { access: 'write' }), approved: false })).status, 400);
      // The pairing is still pending, so a valid approval now succeeds and is honoured.
      assert.equal((await approve(h.consent(client, { access: 'write' }))).status, 200);
      assert.match((await h.finish(client, pairing)).scope, /wenlan:write/);
    });

    let searchOnly;
    let writer;
    await t.test('a missing access or "query" issues the query scope only; "write" adds wenlan:write', async () => {
      searchOnly = await h.connect(device, 'Search only app');
      assert.equal(searchOnly.tokens.scope, 'wenlan:query');
      const explicit = await h.connect(device, 'Explicit query app', 'query');
      assert.equal(explicit.tokens.scope, 'wenlan:query');
      writer = await h.connect(device, 'Write app', 'write');
      assert.equal(writer.tokens.scope, 'wenlan:query wenlan:write');
    });

    await t.test('the device grant list reports each app\'s access', async () => {
      const seen = await grants(device);
      assert.equal(seen[searchOnly.client.client_id], 'query');
      assert.equal(seen[writer.client.client_id], 'write');
    });

    await t.test('a search-only app cannot call write tools and never gets x-wenlan-access, even when it sends one', async () => {
      const before = backend.length;
      for (const name of WRITE_TOOLS) assert.equal((await searchOnly.call(name)).status, 403, name);
      assert.equal(backend.length, before, 'refused calls never reach the device');
      for (const extra of [{ 'x-wenlan-access': 'write' }, {}]) {
        const ok = await searchOnly.call('brief', {}, extra);
        assert.equal(ok.status, 200); await ok.text();
        assert.deepEqual(lastBackend(), { tool: 'brief', access: null });
      }
    });

    await t.test('a write app may call the web write tools; the device sees x-wenlan-access: write', async () => {
      for (const name of WRITE_TOOLS) {
        const ok = await writer.call(name);
        assert.equal(ok.status, 200, name); await ok.text();
        assert.deepEqual(lastBackend(), { tool: name, access: 'write' });
      }
      const spoofed = await writer.call('capture', {}, { 'x-wenlan-access': 'admin' });
      assert.equal(spoofed.status, 200); await spoofed.text();
      assert.deepEqual(lastBackend(), { tool: 'capture', access: 'write' });
      const search = await writer.call('recall', { query: 'x' });
      assert.equal(search.status, 200); await search.text();
      assert.deepEqual(lastBackend(), { tool: 'recall', access: 'write' });
      for (const name of ['search', 'set_agent_trust']) assert.equal((await writer.call(name)).status, 403, name);
      assert.equal((await writer.call('capture', { space: 'other' })).status, 403);
    });

    await t.test('refresh keeps the user\'s choice, even when the client asks for the query scope alone', async () => {
      for (const scope of [undefined, 'wenlan:query']) {
        const refreshed = await h.token({ grant_type: 'refresh_token', client_id: writer.client.client_id,
          refresh_token: writer.tokens.refresh_token, resource, ...(scope ? { scope } : {}) });
        assert.equal(refreshed.status, 200, String(scope));
        writer.tokens = await refreshed.json();
        assert.equal(writer.tokens.scope, 'wenlan:query wenlan:write', String(scope));
        writer.headers = { authorization: `Bearer ${writer.tokens.access_token}` };
        const ok = await h.post('/mcp', { jsonrpc: '2.0', id: 3, method: 'initialize' }, writer.headers);
        const session = ok.headers.get('mcp-session-id'); await ok.text();
        const written = await h.post('/mcp', { jsonrpc: '2.0', id: 4, method: 'tools/call', params: { name: 'capture', arguments: {} } },
          { ...writer.headers, 'mcp-session-id': session });
        assert.equal(written.status, 200, String(scope)); await written.text();
        assert.deepEqual(lastBackend(), { tool: 'capture', access: 'write' });
      }
      assert.equal((await grants(device))[writer.client.client_id], 'write');
      // Write alone, or a scope never granted, is not a token.
      for (const scope of ['wenlan:write', 'admin']) {
        assert.equal((await h.token({ grant_type: 'refresh_token', client_id: writer.client.client_id,
          refresh_token: writer.tokens.refresh_token, resource, scope })).status, 400, scope);
      }
      // A search-only app cannot refresh its way into write.
      const refreshed = await h.token({ grant_type: 'refresh_token', client_id: searchOnly.client.client_id,
        refresh_token: searchOnly.tokens.refresh_token, resource, scope: 'wenlan:query wenlan:write' });
      if (refreshed.status === 200) assert.equal((await refreshed.json()).scope, 'wenlan:query');
      else assert.equal(refreshed.status, 400);
    });

    await t.test('the OAuth request must still be exactly wenlan:query', async () => {
      const client = await h.register('Greedy client');
      for (const scope of ['wenlan:query wenlan:write', 'wenlan:write']) {
        const params = new URLSearchParams({ response_type: 'code', client_id: client.client_id, redirect_uri: callback,
          resource, scope, code_challenge_method: 'S256',
          code_challenge: createHash('sha256').update('x').digest('base64url') });
        assert.equal((await h.request(`/authorize?${params}`)).status, 400, scope);
      }
    });

    await t.test('write tool calls have their own 60 per hour limit per grant, logged under grant-write-hour', async () => {
      const busy = await h.connect(device, 'Busy write app', 'write');
      const other = await h.connect(device, 'Other write app', 'write');
      const quiet = await h.connect(device, 'Quiet search app');
      // Refused attempts by a search-only app and query-tool calls spend none of it.
      for (let n = 0; n < 5; n++) assert.equal((await quiet.call('capture')).status, 403);
      for (let n = 0; n < 5; n++) { const ok = await busy.call('brief'); assert.equal(ok.status, 200); await ok.text(); }
      for (let n = 0; n < 60; n++) {
        const ok = await busy.call(WRITE_TOOLS[n % WRITE_TOOLS.length]);
        assert.equal(ok.status, 200, `write call ${n + 1}`); await ok.text();
      }
      const before = backend.length;
      const refused = await busy.call('capture');
      assert.equal(refused.status, 429);
      assert.ok(Number(refused.headers.get('retry-after')) >= 1);
      assert.equal(refused.headers.get('x-wenlan-limit'), null, 'the internal limit name never leaves the relay');
      assert.deepEqual(await refused.json(), { error: 'Request limit reached' });
      assert.equal(backend.length, before, 'a limited call never reaches the device');
      // Reads still work on the same grant, and other grants have their own budget.
      const read = await busy.call('recall', { query: 'x' });
      assert.equal(read.status, 200); await read.text();
      const unaffected = await other.call('capture');
      assert.equal(unaffected.status, 200); await unaffected.text();
      await delay(300);
      const limitedLines = logged().filter(entry => entry.limit === 'grant-write-hour');
      assert.equal(limitedLines.length, 1);
      assert.deepEqual({ route: limitedLines[0].route, status: limitedLines[0].status, who: limitedLines[0].who },
        { route: '/mcp', status: 429, who: 'grant' });
      assert.equal(Object.keys(limitedLines[0]).some(key => /token|ip|body|tool|name/i.test(key)), false);
      assert.equal(logged().filter(entry => entry.limit === 'grant-minute').length, 0);
    });
  } finally { await runtime.dispose(); }
});

// A synthetic local MCP peer: records every request frame's headers.
function peer(socket, seen) {
  const send = frame => socket.send(JSON.stringify({ v: 1, ...frame }));
  const bodies = new Map();
  socket.addEventListener('message', event => {
    const frame = JSON.parse(event.data);
    if (frame.type === 'request') {
      seen.push(frame);
      if (frame.path === '/connector-info' && !frame.headers.authorization) {
        send({ type: 'response', id: frame.id, status: 401, headers: {} });
        send({ type: 'end', id: frame.id }); return;
      }
      const rpc = frame.path === '/mcp' && frame.method === 'POST'
        ? JSON.parse(Buffer.from(frame.body, 'base64').toString()) : {};
      bodies.set(frame.id, frame.path === '/connector-info'
        ? { contract_version: 1, server: 'wenlan-mcp', tool_profile: 'query-only', authentication: 'bearer', space: candidate.space }
        : { jsonrpc: '2.0', id: rpc.id ?? 1, result: { synthetic: true } });
      send({ type: 'response', id: frame.id, status: 200, headers: {
        'content-type': 'application/json',
        // A fresh backend session per initialize: the relay refuses one claimed twice.
        ...(frame.path === '/mcp' ? { 'mcp-session-id': frame.headers['mcp-session-id'] ?? randomBytes(16).toString('hex') } : {}),
      } });
    } else if (frame.type === 'credit') {
      const body = bodies.get(frame.id);
      if (!body) return;
      bodies.delete(frame.id);
      send({ type: 'chunk', id: frame.id, seq: 0, body: Buffer.from(JSON.stringify(body)).toString('base64') });
      send({ type: 'end', id: frame.id });
    }
  });
  socket.accept();
}

test('actual Worker: write grants carry x-wenlan-access over the reverse channel', { timeout: 120_000 }, async t => {
  const runtime = new Miniflare({ modules: true, script: await bundle(),
    compatibilityDate: '2026-09-08', compatibilityFlags: ['enable_request_signal'], host: '127.0.0.1', port: 0,
    unsafeDirectSockets: [{ host: '127.0.0.1', port: 0, proxy: true }],
    bindings: { PUBLIC_ORIGIN: origin }, kvNamespaces: ['OAUTH_KV'],
    durableObjects: { AUTHORITY: { className: 'RelayAuthority', useSQLite: true } },
    outboundService: () => new Response(null, { status: 502 }),
  });
  const h = harness(runtime);
  const seen = [];
  try {
    const prepared = await h.post('/devices/reverse', { backendToken: candidate.backendToken, space: candidate.space });
    assert.equal(prepared.status, 201);
    const device = await prepared.json();
    const headers = { ...h.native(device), upgrade: 'websocket', 'sec-websocket-protocol': 'wenlan.reverse.v1' };
    const upgraded = await h.request('/devices/reverse/connect', { headers });
    assert.equal(upgraded.status, 101);
    const connectionId = upgraded.headers.get('x-wenlan-connection-id');
    peer(upgraded.webSocket, seen);
    for (const wait of [0, 10, 20, 40, 80, 160, 320, 640, 1280]) {
      if (wait) await delay(wait);
      const status = await h.request('/devices/reverse/status', { headers: { ...h.native(device), 'x-wenlan-connection-id': connectionId } });
      assert.equal(status.status, 200);
      if ((await status.json()).connected) break;
    }
    const writer = await h.connect(device, 'Reverse write app', 'write');
    const searchOnly = await h.connect(device, 'Reverse search app');
    const lastMcp = () => seen.filter(frame => frame.path === '/mcp' && frame.method === 'POST').at(-1);
    const toolOf = frame => JSON.parse(Buffer.from(frame.body, 'base64').toString()).params?.name;

    await t.test('a write grant reaches the device with x-wenlan-access: write, whatever the client sent', async () => {
      for (const extra of [{}, { 'x-wenlan-access': 'admin' }]) {
        const ok = await writer.call('capture', {}, extra);
        assert.equal(ok.status, 200); await ok.text();
        assert.equal(toolOf(lastMcp()), 'capture');
        assert.equal(lastMcp().headers['x-wenlan-access'], 'write');
      }
    });
    await t.test('a search-only grant never carries the header, even when the client sends one', async () => {
      const ok = await searchOnly.call('brief', {}, { 'x-wenlan-access': 'write' });
      assert.equal(ok.status, 200); await ok.text();
      assert.equal(toolOf(lastMcp()), 'brief');
      assert.equal(lastMcp().headers['x-wenlan-access'], undefined);
      const count = seen.length;
      assert.equal((await searchOnly.call('capture', {}, { 'x-wenlan-access': 'write' })).status, 403);
      assert.equal(seen.length, count);
    });
  } finally { await runtime.dispose(); }
});
