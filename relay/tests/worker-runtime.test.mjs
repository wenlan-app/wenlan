// SPDX-License-Identifier: Apache-2.0
import assert from 'node:assert/strict';
import test from 'node:test';
import { createRequire } from 'node:module';
import { resolve, join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createHash, randomBytes } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { setTimeout as delay } from 'node:timers/promises';

if (!process.env.WENLAN_RELAY_TOOLCHAIN) throw new Error('WENLAN_RELAY_TOOLCHAIN required');
const requireTool = createRequire(join(resolve(process.env.WENLAN_RELAY_TOOLCHAIN), 'package.json'));
const { Miniflare } = requireTool('miniflare');
const { build } = requireTool('esbuild');
const origin = 'https://wenlan-relay.example';
const candidate = { tunnelOrigin: 'https://synthetic.trycloudflare.com',
  backendToken: 'b'.repeat(43), space: 'review' };

const realExpiry = process.env.WENLAN_TEST_REAL_PAIRING_EXPIRY === '1';
test('actual Worker entry owns enrollment, safe cookies, consent and OAuth forwarding', {
  timeout: realExpiry ? 670_000 : 60_000,
}, async t => {
  const packagedEntry = process.env.WENLAN_RELAY_BUNDLE_DIR
    ? join(resolve(process.env.WENLAN_RELAY_BUNDLE_DIR), 'worker.js') : null;
  const bundle = packagedEntry ? null : await build({ entryPoints: [fileURLToPath(new URL('../src/worker.ts', import.meta.url))],
    bundle: true, write: false, format: 'esm', platform: 'browser', target: 'es2022',
    external: ['cloudflare:workers'], loader: { '.png': 'binary' } });
  const source = packagedEntry ? await readFile(packagedEntry, 'utf8') : bundle.outputFiles[0].text;
  assert(!source.includes('origin-relay'));
  assert(!source.includes('LEGACY_RELAY'));
  let backendCalls = 0;
  let streaming = false;
  const runtime = new Miniflare({ modules: true,
    ...(packagedEntry ? { scriptPath: packagedEntry, modulesRoot: dirname(packagedEntry),
      modulesRules: [{ type: 'Data', include: ['**/*.png'] }] } : { script: source }),
    compatibilityDate: '2026-09-08', compatibilityFlags: ['enable_request_signal'], host: '127.0.0.1', port: 0,
    bindings: { PUBLIC_ORIGIN: origin }, kvNamespaces: ['OAUTH_KV'],
    durableObjects: { AUTHORITY: { className: 'RelayAuthority', useSQLite: true } },
    outboundService: request => {
      const url = new URL(request.url);
      if (url.origin !== candidate.tunnelOrigin) return new Response(null, { status: 502 });
      if (!request.headers.has('authorization')) return new Response(null, { status: 401 });
      assert.equal(request.headers.get('authorization'), `Bearer ${candidate.backendToken}`);
      if (url.pathname === '/connector-info') return Response.json({ contract_version: 1,
        server: 'wenlan-mcp', tool_profile: 'query-only', authentication: 'bearer', space: candidate.space });
      if (url.pathname === '/mcp') {
        backendCalls++;
        if (streaming) return new Response(new ReadableStream({ start(controller) {
          controller.enqueue(new TextEncoder().encode('data: synthetic-heartbeat\n\n'));
        } }), { headers: { 'content-type': 'text/event-stream' } });
        return Response.json({ jsonrpc: '2.0', id: 1, result: { synthetic: true } }, {
          headers: request.headers.has('mcp-session-id') ? {} : { 'mcp-session-id': 'synthetic-private-session' },
        });
      }
      return new Response(null, { status: 404 });
    },
  });
  const request = (path, init) => runtime.dispatchFetch(`${origin}${path}`, { ...init, credentials: 'omit' });
  const post = (path, value, headers = {}) => request(path, { method: 'POST',
    headers: { 'content-type': 'application/json', ...headers }, body: JSON.stringify(value) });
  let device;
  let client;
  let pairId;
  let cookie;
  const verifier = randomBytes(32).toString('base64url');
  const deviceHeaders = () => ({ authorization: `Bearer ${device.managementToken}`, 'x-wenlan-device-id': device.id });
  try {
    await t.test('only protected native connector enrollment succeeds', async () => {
      assert.equal((await post('/devices', candidate, { origin })).status, 403);
      const response = await post('/devices', candidate);
      assert.equal(response.status, 201, await response.clone().text());
      device = await response.json();
      assert.equal(response.headers.get('cache-control'), 'no-store');
      assert.equal((await post('/devices/refresh', candidate, { 'x-wenlan-device-id': device.id })).status, 401);
    });
    const registration = await post('/oauth/register', { client_name: 'Synthetic client',
      redirect_uris: ['https://client.example/callback'], grant_types: ['authorization_code', 'refresh_token'],
      response_types: ['code'], token_endpoint_auth_method: 'none' });
    assert.equal(registration.status, 201);
    client = await registration.json();
    await t.test('authorization renders the real page without exposing the browser secret', async () => {
      const query = new URLSearchParams({ response_type: 'code', client_id: client.client_id,
        redirect_uri: 'https://client.example/callback', resource: `${origin}/mcp`, scope: 'wenlan:query',
        code_challenge_method: 'S256', code_challenge: createHash('sha256').update(verifier).digest('base64url') });
      const response = await request(`/authorize?${query}`, { redirect: 'manual' });
      assert.equal(response.status, 303, await response.clone().text());
      assert.equal(response.headers.get('location'), '/pairing');
      const header = response.headers.get('set-cookie');
      assert.match(header, /^__Host-wenlan-pairing=/);
      for (const flag of ['HttpOnly', 'Secure', 'SameSite=Lax', 'Path=/', 'Max-Age=600']) assert(header.includes(flag));
      cookie = header.split(';')[0];
      const secret = cookie.split('=')[1].split('.')[1];
      assert.equal(await response.text(), '');
      const page = await request('/pairing', { headers: { cookie } });
      const html = await page.text();
      assert(!html.includes(secret));
      pairId = cookie.split('=')[1].split('.')[0];
      assert.match(html, /Waiting for you in Wenlan\.\.\./);
      assert.match(html, /id="continue"[^>]*disabled/);
      assert.match(html, /https:\/\/wenlan.app\/docs\/data-and-privacy/);
      assert.match(html, /<output id="user-code">[2-9A-HJKMNP-TV-Z]{4}-[2-9A-HJKMNP-TV-Z]{4}<\/output>/, 'the short code is the fallback');
      assert.match(html, /class="warning"/, 'client.example is not an allowlisted client');
      assert.match(html, /Connect client\.example to Wenlan/, 'an unknown client is named by its redirect host');
      assert(!html.includes('Synthetic client'), 'the self-declared DCR name is never displayed');
      const openLink = /id="open-in-wenlan"[^>]*href="([^"]+)"/.exec(html);
      assert.equal(openLink?.[1], `wenlan://pair?code=${pairId}`, 'the app link carries only the pairing code, never the browser secret');
      const pendingStatus = await request('/pairing/status', { headers: { cookie } });
      assert.equal(pendingStatus.headers.get('cache-control'), 'no-store');
      assert.deepEqual(await pendingStatus.json(), { status: 'pending' });
      assert.deepEqual(await (await request('/pairing/status')).json(), { status: 'unavailable' });
      assert.deepEqual(await (await request('/pairing/status', { headers: { cookie: `${cookie}; ${cookie}` } })).json(), { status: 'unavailable' });
      assert.equal((await request('/pairing/status', { headers: { cookie, origin: 'https://attacker.example' } })).status, 403);
      assert.match(page.headers.get('content-security-policy'), /frame-ancestors 'none'/);
      const iconResponse = await request('/icon.png');
      assert.equal(iconResponse.headers.get('content-type'), 'image/png');
      assert.deepEqual(Buffer.from(await iconResponse.arrayBuffer()),
        await readFile(new URL('../../app/icons/128x128.png', import.meta.url)));
    });
    await t.test('cookie spoofing, cross-origin completion and ID-only access fail', async () => {
      assert.equal((await post('/pairing/complete', {}, { cookie, origin: 'https://attacker.example' })).status, 403);
      assert.equal((await post('/pairing/complete', {}, { cookie })).status, 403);
      assert.equal((await post('/pairing/complete', {}, { origin })).status, 401);
      assert.equal((await post('/pairing/complete', {}, { cookie: `${cookie}; ${cookie}`, origin })).status, 401);
      assert.equal((await post('/pairing/complete', {}, { cookie: `${cookie}.`, origin })).status, 401);
      const pending = await post('/pairing/complete', {}, { cookie, origin });
      assert.equal(pending.status, 409);
      assert.deepEqual(await pending.json(), { redirectTo: null, error: 'Device approval is still pending.' });
      assert.equal((await request(`/pairings/${pairId}`)).status, 401);
    });
    await t.test('authenticated explicit device consent is required', async () => {
      const view = await (await request(`/pairings/${pairId}`, { headers: deviceHeaders() })).json();
      assert.equal(view.clientId, client.client_id);
      assert(!JSON.stringify(view).includes(device.managementToken));
      const consent = { clientId: client.client_id, resource: `${origin}/mcp`, space: candidate.space };
      assert.equal((await post(`/pairings/${pairId}/approve`, consent, deviceHeaders())).status, 400);
      assert.equal((await post(`/pairings/${pairId}/approve`, { ...consent, approved: true }, deviceHeaders())).status, 200);
      const page = await (await request('/pairing', { headers: { cookie } })).text();
      assert.match(page, /Allowed\. Taking you back to client\.example\.\.\./);
      assert.match(page, /<dl id="approved-space" data-space="review"><dt>Space<\/dt><dd>review<\/dd><\/dl>/);
      assert.doesNotMatch(page, /id="continue"[^>]*disabled/);
      assert.deepEqual(await (await request('/pairing/status', { headers: { cookie } })).json(),
        { status: 'approved', space: candidate.space });
    });
    let tokens;
    await t.test('browser completion returns the validated callback and clears its cookie', async () => {
      const response = await post('/pairing/complete', {}, { cookie, origin, 'sec-fetch-site': 'same-origin' });
      assert.equal(response.status, 200);
      assert.match(response.headers.get('set-cookie'), /Max-Age=0/);
      const redirect = new URL((await response.json()).redirectTo);
      assert.equal(redirect.origin, 'https://client.example');
      assert.equal(redirect.searchParams.get('iss'), origin);
      const replay = await post('/pairing/complete', {}, { cookie, origin });
      assert.deepEqual(await (await request('/pairing/status', { headers: { cookie } })).json(), { status: 'unavailable' });
      assert.equal(replay.status, 409);
      assert.deepEqual(await replay.json(), { redirectTo: null, pairingUnavailable: true,
        error: 'This pairing has expired or is no longer available. Start a new connection in your AI client.' });
      const result = await request('/oauth/token', { method: 'POST',
        headers: { 'content-type': 'application/x-www-form-urlencoded' }, body: new URLSearchParams({
          grant_type: 'authorization_code', code: redirect.searchParams.get('code'), code_verifier: verifier,
          client_id: client.client_id, redirect_uri: 'https://client.example/callback', resource: `${origin}/mcp`,
        }).toString() });
      assert.equal(result.status, 200, await result.clone().text());
      tokens = await result.json();
      const initialized = await post('/mcp', { jsonrpc: '2.0', id: 0, method: 'initialize' },
        { authorization: `Bearer ${tokens.access_token}` });
      assert.equal(initialized.status, 200, await initialized.clone().text());
      await initialized.text();
      const session = initialized.headers.get('mcp-session-id');
      assert.notEqual(session, 'synthetic-private-session');
      const mcpHeaders = { authorization: `Bearer ${tokens.access_token}`, 'mcp-session-id': session };
      const query = { jsonrpc: '2.0', id: 1, method: 'tools/call', params: { name: 'brief', arguments: {} } };
      assert.equal((await post('/mcp', query, mcpHeaders)).status, 200);
      assert.equal(backendCalls, 2);
      streaming = true;
      const live = await request('/mcp', { headers: mcpHeaders });
      assert.equal(live.status, 200);
      const reader = live.body.getReader();
      assert.match(new TextDecoder().decode((await reader.read()).value), /synthetic-heartbeat/);
      assert.equal((await post('/devices/revoke', {}, deviceHeaders())).status, 200);
      const retried = await post('/devices/revoke', {}, deviceHeaders());
      assert.equal(retried.status, 200);
      assert.deepEqual(await retried.json(), { success: true });
      assert.equal((await post('/devices/refresh', candidate, deviceHeaders())).status, 401);
      let timer;
      const start = Date.now();
      try {
        const ended = await Promise.race([reader.read(), new Promise((_, reject) => {
          timer = setTimeout(() => reject(new Error('REVOCATION_TIMEOUT')), 4000);
        })]).then(next => next.done, error => {
          if (error.message === 'REVOCATION_TIMEOUT') throw error;
          return true;
        });
        // workerd may surface an errored response stream as EOF at the HTTP
        // client. Both termination forms must stop data within the same bound.
        assert.equal(ended, true, 'data arrived after the revocation barrier');
        assert(Date.now() - start < 4000);
      } finally { clearTimeout(timer); await reader.cancel().catch(() => {}); }
      assert.equal((await post('/mcp', query, mcpHeaders)).status, 401);
      assert.equal(backendCalls, 3);
    });
    await t.test('cancelled and unknown pairings report unavailable, not pending', async () => {
      const query = new URLSearchParams({ response_type: 'code', client_id: client.client_id,
        redirect_uri: 'https://client.example/callback', resource: `${origin}/mcp`, scope: 'wenlan:query',
        code_challenge_method: 'S256', code_challenge: createHash('sha256').update(verifier).digest('base64url') });
      const authorized = await request(`/authorize?${query}`, { redirect: 'manual' });
      assert.equal(authorized.status, 303);
      const secondCookie = authorized.headers.get('set-cookie').split(';')[0];
      const secondId = secondCookie.split('=')[1].split('.')[0];
      assert.notEqual(secondId, pairId);
      const forged = `__Host-wenlan-pairing=${secondId}.${randomBytes(32).toString('hex')}`;
      const forgedResult = await post('/pairing/complete', {}, { cookie: forged, origin });
      assert.equal(forgedResult.status, 409);
      assert.deepEqual(await forgedResult.json(), { redirectTo: null, pairingUnavailable: true,
        error: 'This pairing has expired or is no longer available. Start a new connection in your AI client.' });
      const stillPending = await post('/pairing/complete', {}, { cookie: secondCookie, origin });
      assert.equal(stillPending.status, 409);
      assert.deepEqual(await stillPending.json(), { redirectTo: null, error: 'Device approval is still pending.' });
      const cancelled = await post('/pairing/cancel', {}, { cookie: secondCookie, origin });
      assert.equal(cancelled.status, 200);
      const cancelledBody = await cancelled.json();
      assert.equal(cancelledBody.cancelled, true);
      assert.equal(new URL(cancelledBody.redirectTo).searchParams.get('error'), 'access_denied');
      assert.match(cancelled.headers.get('set-cookie'), /Max-Age=0/);
      const afterCancel = await post('/pairing/complete', {}, { cookie: secondCookie, origin });
      assert.equal(afterCancel.status, 409);
      assert.deepEqual(await afterCancel.json(), { redirectTo: null, pairingUnavailable: true,
        error: 'This pairing has expired or is no longer available. Start a new connection in your AI client.' });
      const recancel = await post('/pairing/cancel', {}, { cookie: secondCookie, origin });
      assert.equal(recancel.status, 409);
      assert.deepEqual(await recancel.json(), { cancelled: false, pairingUnavailable: true,
        error: 'This pairing has expired or is no longer available. Start a new connection in your AI client.' });
      const unknownCookie = `__Host-wenlan-pairing=${randomBytes(32).toString('hex')}.${randomBytes(32).toString('hex')}`;
      const unknown = await post('/pairing/complete', {}, { cookie: unknownCookie, origin });
      assert.equal(unknown.status, 409);
      assert.deepEqual(await unknown.json(), { redirectTo: null, pairingUnavailable: true,
        error: 'This pairing has expired or is no longer available. Start a new connection in your AI client.' });
    });
    await t.test('pairing TTL expiry reports unavailable', {
      skip: realExpiry ? false : 'Set WENLAN_TEST_REAL_PAIRING_EXPIRY=1 for the ten-minute wall-clock check',
    }, async () => {
      const query = new URLSearchParams({ response_type: 'code', client_id: client.client_id,
        redirect_uri: 'https://client.example/callback', resource: `${origin}/mcp`, scope: 'wenlan:query',
        code_challenge_method: 'S256', code_challenge: createHash('sha256').update(verifier).digest('base64url') });
      const authorized = await request(`/authorize?${query}`, { redirect: 'manual' });
      assert.equal(authorized.status, 303);
      const expiryCookie = authorized.headers.get('set-cookie').split(';')[0];
      assert.match(authorized.headers.get('set-cookie'), /Max-Age=600/);
      const pending = await post('/pairing/complete', {}, { cookie: expiryCookie, origin });
      assert.equal(pending.status, 409);
      assert.deepEqual(await pending.json(), { redirectTo: null, error: 'Device approval is still pending.' });
      const started = Date.now();
      console.log('WENLAN_REAL_EXPIRY_WAIT_STARTED');
      await delay(601_000);
      assert(Date.now() - started >= 600_000, 'expiry must be exercised through actual elapsed time');
      const expired = await post('/pairing/complete', {}, { cookie: expiryCookie, origin });
      assert.equal(expired.status, 409);
      assert.deepEqual(await expired.json(), { redirectTo: null, pairingUnavailable: true,
        error: 'This pairing has expired or is no longer available. Start a new connection in your AI client.' });
      const page = await request('/pairing', { headers: { cookie: expiryCookie } });
      assert.match(await page.text(), /This request expired\. Start connecting again from your AI app\./);
    });
    await t.test('bounds, unsupported methods and enrollment rate limits are enforced', async () => {
      assert.equal((await request('/devices', { method: 'PUT' })).status, 405);
      assert.equal((await post('/devices', { data: 'x'.repeat(20_000) })).status, 413);
      const oversized = await request('/oauth/token', { method: 'POST', headers: { 'content-type': 'application/json' },
        body: new ReadableStream({ start(controller) { controller.enqueue(new TextEncoder().encode('x'.repeat(20_000))); controller.close(); } }), duplex: 'half' });
      assert.equal(oversized.status, 413);
      // Earlier failed enrollments still consume the abuse budget.
      let limited = false;
      for (let i = 0; i < 11; i++) {
        const response = await post('/devices', candidate);
        if (response.status === 429) {
          limited = true;
          const delay = Number(response.headers.get('retry-after'));
          assert(delay > 0 && delay <= 3600);
        }
      }
      assert.equal(limited, true);
    });
  } finally { await runtime.dispose(); }
});
