// SPDX-License-Identifier: Apache-2.0
import assert from 'node:assert/strict';
import test from 'node:test';
import { connectorBinding, forwardQuery, type ConnectorRoute, type ProxyOptions, type QueryGrant } from '../src/proxy.ts';
import { ReverseTransport, reverseQueryFetcher } from '../src/reverse-transport.ts';
import { decodeBody, decodeFrame, encodeBody, encodeFrame, type RequestFrame } from '../src/reverse-protocol.ts';
import { forwardSessionQuery } from '../src/sessions.ts';
import { MemoryStore } from './fixtures/memory-store.ts';

const origin = 'https://relay.example';
const grant: QueryGrant = { subject: 'owner', connectorId: 'device', space: 'review', generation: 1,
  scopes: ['wenlan:query'], expiresAt: Date.now() + 60_000 };
const route: ConnectorRoute = { id: 'device', subject: 'owner', space: 'review', generation: 1,
  enabled: true, expiresAt: Date.now() + 120_000, backendToken: 'b'.repeat(43), reverseConnectionId: 'r'.repeat(43) };
function rpc(method = 'tools/call', args: object = {}, session?: string, name = 'brief', extra: Record<string, string> = {}) {
  const headers = new Headers({ 'content-type': 'application/json', authorization: 'Bearer PRIVATE_OAUTH',
    cookie: 'PRIVATE_COOKIE', 'x-wenlan-space': 'private', ...extra });
  if (session) headers.set('mcp-session-id', session);
  return new Request(`${origin}/mcp`, { method: 'POST', headers, body: JSON.stringify({
    jsonrpc: '2.0', id: 1, method, params: { name, arguments: args },
  }) });
}
function setup() {
  const calls: RequestFrame[] = [];
  let tunnelCalls = 0;
  let current = { ...route };
  const active = new Set<string>();
  const transport = new ReverseTransport({
    send(text) {
      const frame = decodeFrame(text);
      if (frame.type === 'request') {
        calls.push(frame); active.add(frame.id);
        queueMicrotask(() => transport.receive(encodeFrame({ v: 1, type: 'response', id: frame.id,
          status: 200, headers: { 'content-type': 'application/json', 'mcp-session-id': 'synthetic-backend' } })));
      } else if (frame.type === 'credit' && active.delete(frame.id)) {
        queueMicrotask(() => {
          transport.receive(encodeFrame({ v: 1, type: 'chunk', id: frame.id, seq: frame.seq,
            body: encodeBody(new TextEncoder().encode('{"jsonrpc":"2.0","id":1,"result":{}}')) }));
          transport.receive(encodeFrame({ v: 1, type: 'end', id: frame.id }));
        });
      } else if (frame.type === 'cancel') active.delete(frame.id);
    },
    close() { active.clear(); },
  });
  const options: ProxyOptions = { publicOrigin: origin, loadRoute: async () => current,
    fetch: async () => { tunnelCalls++; throw new Error('Tunnel fallback must never be used'); },
    fetchReverse: reverseQueryFetcher(id => id === route.reverseConnectionId ? transport : undefined),
  };
  return { calls, options, transport, get tunnelCalls() { return tunnelCalls; },
    setRoute(value: ConnectorRoute) { current = value; } };
}

test('reverse transport retains query policy and replaces OAuth with the local backend bearer', async t => {
  const s = setup(); t.after(() => s.transport.disconnect());
  const response = await forwardQuery(rpc(), grant, s.options);
  assert.equal(response.status, 200); await response.text();
  assert.equal(s.calls.length, 1);
  assert.equal(s.tunnelCalls, 0);
  assert.equal(s.calls[0].headers.authorization, `Bearer ${route.backendToken}`);
  for (const key of ['cookie', 'x-wenlan-space']) assert.equal(s.calls[0].headers[key], undefined);
  assert.equal(JSON.parse(new TextDecoder().decode(decodeBody(s.calls[0].body))).method, 'tools/call');
  for (const request of [rpc('tools/call', { space: 'private' }), rpc('resources/read')]) {
    assert.equal((await forwardQuery(request, grant, s.options)).status, 403);
  }
  assert.equal(s.calls.length, 1);
});

test('reverse transport carries x-wenlan-access for a write grant only, never from the client', async t => {
  const s = setup(); t.after(() => s.transport.disconnect());
  const writeGrant = { ...grant, scopes: ['wenlan:query', 'wenlan:write'] };
  const spoof = { 'x-wenlan-access': 'write' };
  const send = async (as: QueryGrant, name: string, extra: Record<string, string> = {}) => {
    const response = await forwardQuery(rpc('tools/call', {}, undefined, name, extra), as, s.options);
    await response.text();
    return response.status;
  };
  assert.equal(await send(writeGrant, 'capture'), 200);
  assert.equal(s.calls.at(-1)!.headers['x-wenlan-access'], 'write');
  assert.equal(await send(writeGrant, 'capture', { 'x-wenlan-access': 'admin' }), 200);
  assert.equal(s.calls.at(-1)!.headers['x-wenlan-access'], 'write');
  assert.equal(await send(grant, 'brief', spoof), 200);
  assert.equal(s.calls.at(-1)!.headers['x-wenlan-access'], undefined);
  const before = s.calls.length;
  assert.equal(await send(grant, 'capture', spoof), 403);
  assert.equal(s.calls.length, before);
  assert.equal(s.tunnelCalls, 0);
});

test('missing channels and ambiguous or invalid route transports never fall back to a tunnel', async t => {
  const s = setup(); t.after(() => s.transport.disconnect());
  for (const invalid of [
    { ...route, tunnelOrigin: 'https://old.trycloudflare.com' },
    { ...route, reverseConnectionId: 'bad' },
    { ...route, reverseConnectionId: undefined },
  ]) {
    s.setRoute(invalid);
    assert.equal((await forwardQuery(rpc(), grant, s.options)).status, 503);
  }
  s.setRoute({ ...route, reverseConnectionId: 'x'.repeat(43) });
  const missing = await forwardQuery(rpc(), grant, s.options);
  assert.equal(missing.status, 200);
  assert.equal((await missing.json()).result.isError, true);
  s.setRoute(route);
  assert.equal((await forwardQuery(rpc(), grant, { ...s.options, fetchReverse: undefined })).status, 503);
  assert.equal(s.calls.length, 0);
  assert.equal(s.tunnelCalls, 0);
});

test('reverse connection replacement invalidates the old MCP session before forwarding', async t => {
  const s = setup(); t.after(() => s.transport.disconnect());
  const store = new MemoryStore();
  const identity = { grantId: 'grant', clientId: 'client' };
  const initialized = await forwardSessionQuery(rpc('initialize'), grant, identity, store, s.options);
  assert.equal(initialized.status, 200); await initialized.text();
  const session = initialized.headers.get('mcp-session-id')!;
  assert.notEqual(session, 'synthetic-backend');
  s.setRoute({ ...route, reverseConnectionId: 'x'.repeat(43) });
  assert.equal((await forwardSessionQuery(rpc('tools/list', {}, session), grant, identity, store, s.options)).status, 404);
  assert.equal(s.calls.length, 1);
});

test('connection replacement during a response rejects its next chunk', async () => {
  let current = { ...route };
  let controller: ReadableStreamDefaultController<Uint8Array>;
  let cancelled = false;
  const options: ProxyOptions = { publicOrigin: origin, loadRoute: async () => current,
    fetchReverse: async () => new Response(new ReadableStream<Uint8Array>({
      start(c) { controller = c; }, cancel() { cancelled = true; },
    })),
  };
  const response = await forwardQuery(rpc(), grant, options);
  assert.equal(response.status, 200);
  current = { ...route, reverseConnectionId: 'x'.repeat(43) };
  const failure = assert.rejects(response.body!.getReader().read());
  controller!.enqueue(new Uint8Array([1]));
  await failure;
  assert.equal(cancelled, true);
});

test('existing tunnel session binding bytes are unchanged', () => {
  const tunnel = { ...route, reverseConnectionId: undefined, tunnelOrigin: 'https://old.trycloudflare.com' };
  assert.equal(connectorBinding(tunnel), JSON.stringify([tunnel.id, tunnel.tunnelOrigin, tunnel.backendToken, tunnel.generation]));
  assert.notEqual(connectorBinding(route), connectorBinding({ ...route, reverseConnectionId: 'x'.repeat(43) }));
});
