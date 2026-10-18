// SPDX-License-Identifier: Apache-2.0
// One-click pairing contract through the real Worker entry: short-code lookup,
// deny, localized pages, the authorize failure page, grant display fields,
// the per-grant /mcp limit and the 401 re-authorization challenge.
import assert from 'node:assert/strict';
import test from 'node:test';
import { createRequire } from 'node:module';
import { resolve, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createHash, randomBytes } from 'node:crypto';

if (!process.env.WENLAN_RELAY_TOOLCHAIN) throw new Error('WENLAN_RELAY_TOOLCHAIN required');
const requireTool = createRequire(join(resolve(process.env.WENLAN_RELAY_TOOLCHAIN), 'package.json'));
const { Miniflare } = requireTool('miniflare');
const { build } = requireTool('esbuild');
const origin = 'https://wenlan-relay.example';
const candidate = { tunnelOrigin: 'https://synthetic.trycloudflare.com', backendToken: 'b'.repeat(43), space: 'review' };
const callback = 'https://claude.ai/api/mcp/auth_callback';

test('one-click pairing contract through the Worker entry', { timeout: 90_000 }, async t => {
  const bundle = await build({ entryPoints: [fileURLToPath(new URL('../src/worker.ts', import.meta.url))],
    bundle: true, write: false, format: 'esm', platform: 'browser', target: 'es2022',
    external: ['cloudflare:workers'], loader: { '.png': 'binary' } });
  const runtime = new Miniflare({ modules: true, script: bundle.outputFiles[0].text,
    compatibilityDate: '2026-09-08', compatibilityFlags: ['enable_request_signal'], host: '127.0.0.1', port: 0,
    bindings: { PUBLIC_ORIGIN: origin }, kvNamespaces: ['OAUTH_KV'],
    durableObjects: { AUTHORITY: { className: 'RelayAuthority', useSQLite: true } },
    outboundService: request => {
      const url = new URL(request.url);
      if (url.origin !== candidate.tunnelOrigin || !request.headers.has('authorization')) return new Response(null, { status: 401 });
      if (url.pathname === '/connector-info') return Response.json({ contract_version: 1,
        server: 'wenlan-mcp', tool_profile: 'query-only', authentication: 'bearer', space: candidate.space });
      if (url.pathname === '/mcp') return Response.json({ jsonrpc: '2.0', id: 1, result: { synthetic: true } }, {
        headers: request.headers.has('mcp-session-id') ? {} : { 'mcp-session-id': 'synthetic-private-session' } });
      return new Response(null, { status: 404 });
    },
  });
  // A distinct peer per request keeps the per-peer limit out of the per-grant check.
  let peer = 0;
  const request = (path, init = {}) => runtime.dispatchFetch(`${origin}${path}`, { ...init, credentials: 'omit',
    headers: { 'cf-connecting-ip': `198.51.100.${++peer % 250}`, ...init.headers } });
  const post = (path, value, headers = {}) => request(path, { method: 'POST',
    headers: { 'content-type': 'application/json', ...headers }, body: JSON.stringify(value) });
  const verifier = randomBytes(32).toString('base64url');
  try {
    const enrolled = await post('/devices', candidate);
    assert.equal(enrolled.status, 201, await enrolled.clone().text());
    const device = await enrolled.json();
    const deviceHeaders = { authorization: `Bearer ${device.managementToken}`, 'x-wenlan-device-id': device.id };
    // client_name claims to be someone else; only the redirect host decides trust.
    const registration = await post('/oauth/register', { client_name: '  Totally Official\u0007 ',
      redirect_uris: [callback], grant_types: ['authorization_code', 'refresh_token'],
      response_types: ['code'], token_endpoint_auth_method: 'none' });
    assert.equal(registration.status, 201);
    const client = await registration.json();
    const authorizeQuery = (overrides = {}) => new URLSearchParams({ response_type: 'code', client_id: client.client_id,
      redirect_uri: callback, resource: `${origin}/mcp`, scope: 'wenlan:query', state: 'client-state',
      code_challenge_method: 'S256', code_challenge: createHash('sha256').update(verifier).digest('base64url'), ...overrides });
    const begin = async (headers = {}) => {
      const response = await request(`/authorize?${authorizeQuery()}`, { redirect: 'manual', headers });
      assert.equal(response.status, 303, await response.clone().text());
      const header = response.headers.get('set-cookie');
      const cookie = header.split(';')[0];
      return { header, cookie, id: cookie.split('=')[1].split('.')[0] };
    };

    await t.test('authorize failures render a page for browsers and JSON otherwise', async () => {
      const bad = `/authorize?${authorizeQuery({ client_id: 'missing-client' })}`;
      const html = await request(bad, { headers: { accept: 'text/html,application/xhtml+xml;q=0.9', 'accept-language': 'zh-TW' } });
      assert.equal(html.status, 400);
      assert.match(html.headers.get('content-type'), /^text\/html/);
      assert.match(html.headers.get('content-security-policy'), /frame-ancestors 'none'/);
      const body = await html.text();
      assert.match(body, /<html lang="zh-Hant">/);
      assert.match(body, /這個連接連結無法使用/);
      for (const accept of [undefined, 'application/json', '*/*']) {
        const json = await request(bad, { headers: accept ? { accept } : {} });
        assert.equal(json.status, 400, accept);
        assert.match(json.headers.get('content-type'), /^application\/json/);
        assert.deepEqual(await json.json(), { error: 'Authorization request rejected' });
      }
    });

    const first = await begin({ 'accept-language': 'zh-CN' });
    let userCode;
    await t.test('the pairing lives ten minutes and its page shows the allowlisted name and short code', async () => {
      for (const flag of ['HttpOnly', 'Secure', 'SameSite=Lax', 'Path=/', 'Max-Age=600']) assert(first.header.includes(flag), flag);
      for (const [language, lang, title] of [['zh-CN', 'zh-Hans', '将 Claude 连接到 Wenlan'],
        ['zh-HK', 'zh-Hant', '將 Claude 連接到 Wenlan'], ['en-US', 'en', 'Connect Claude to Wenlan']]) {
        const page = await request('/pairing', { headers: { cookie: first.cookie, 'accept-language': language } });
        assert.equal(page.headers.get('content-language'), lang);
        assert.match(page.headers.get('vary'), /accept-language/i);
        const html = await page.text();
        assert(html.includes(`<html lang="${lang}">`), language);
        assert(html.includes(`<h1>${title}</h1>`), language);
        assert(html.includes(`href="wenlan://pair?code=${first.id}"`));
        assert(!html.includes('Totally Official'), 'the DCR name never reaches the browser page');
        assert(!html.includes('class="warning"'), 'claude.ai is allowlisted');
        userCode = /<output id="user-code">([2-9A-Z]{4}-[2-9A-Z]{4})<\/output>/.exec(html)[1];
      }
    });

    let inspected;
    await t.test('device inspect and lookup return identity fields; lookup normalizes and never needs the long ID', async () => {
      const response = await request(`/pairings/${first.id}`, { headers: deviceHeaders });
      inspected = await response.json();
      assert.equal(inspected.pairingId, first.id);
      assert.equal(inspected.clientName, 'Totally Official');
      assert.equal(inspected.redirectHost, 'claude.ai');
      assert.equal(inspected.knownClient, true);
      for (const code of [userCode, userCode.toLowerCase(), userCode.replace('-', ''), ` ${userCode.replace('-', ' ').toLowerCase()} `]) {
        const lookup = await post('/pairings/lookup', { code }, deviceHeaders);
        assert.equal(lookup.status, 200, code);
        assert.equal(lookup.headers.get('cache-control'), 'no-store');
        assert.deepEqual(await lookup.json(), inspected, code);
      }
      assert.equal((await post('/pairings/lookup', { code: userCode })).status, 401, 'device auth is required');
      assert.equal((await post('/pairings/lookup', { code: userCode, pairingId: first.id }, deviceHeaders)).status, 400);
    });

    await t.test('lookup is POST only: a GET never resolves a code and never counts toward the miss limit', async () => {
      const variants = ['/pairings/lookup', `/pairings/lookup?code=${userCode}`,
        `/pairings/lookup?code=${encodeURIComponent(userCode.replace('-', '').toLowerCase())}`];
      // Eleven GETs: more than the ten-miss budget, so a GET that ran a lookup would leave this device limited.
      for (let n = 0; n < 11; n++) {
        const path = variants[n % variants.length];
        const response = await request(path, { headers: n % 2 ? deviceHeaders : { ...deviceHeaders, accept: 'application/json' } });
        assert([404, 405].includes(response.status), `${path}: ${response.status}`);
        const body = await response.text();
        assert(!body.includes(first.id) && !body.includes('pairingId'), `${path} leaked a pairing`);
      }
      const after = await post('/pairings/lookup', { code: userCode }, deviceHeaders);
      assert.equal(after.status, 200, 'the POST lookup still resolves, so no GET spent the miss budget');
      assert.deepEqual(await after.json(), inspected);
    });

    await t.test('a denied request reports denied and cancel returns access_denied to the client', async () => {
      const second = await begin();
      assert.equal((await post(`/pairings/${second.id}/deny`, {})).status, 401);
      const denied = await post(`/pairings/${second.id}/deny`, {}, deviceHeaders);
      assert.equal(denied.status, 204);
      assert.equal((await post(`/pairings/${second.id}/deny`, {}, deviceHeaders)).status, 404, 'deny is once');
      assert.equal((await request(`/pairings/${second.id}`, { headers: deviceHeaders })).status, 404);
      assert.deepEqual(await (await request('/pairing/status', { headers: { cookie: second.cookie } })).json(), { status: 'denied' });
      const page = await (await request('/pairing', { headers: { cookie: second.cookie } })).text();
      assert.match(page, /data-pairing-state="denied"/);
      assert.match(page, /You declined this in Wenlan\./);
      const cancelled = await post('/pairing/cancel', {}, { cookie: second.cookie, origin });
      assert.equal(cancelled.status, 200);
      assert.match(cancelled.headers.get('set-cookie'), /Max-Age=0/);
      const result = await cancelled.json();
      assert.equal(result.cancelled, true);
      const redirect = new URL(result.redirectTo);
      assert.equal(`${redirect.origin}${redirect.pathname}`, callback);
      assert.equal(redirect.searchParams.get('error'), 'access_denied');
      assert.equal(redirect.searchParams.get('state'), 'client-state');
      assert.equal(redirect.searchParams.get('iss'), origin);
      assert.equal(redirect.searchParams.has('code'), false);
    });

    await t.test('no-JS cancel posts get a same-origin page that links back to the client', async () => {
      const third = await begin();
      const response = await request('/pairing/cancel', { method: 'POST', body: '',
        headers: { cookie: third.cookie, origin, 'content-type': 'application/x-www-form-urlencoded' } });
      assert.equal(response.status, 200);
      const html = await response.text();
      const href = /id="return-to-client"[^>]*href="([^"]+)"/.exec(html)[1].replaceAll('&amp;', '&');
      assert.equal(new URL(href).searchParams.get('error'), 'access_denied');
    });

    let tokens;
    let grantId;
    let mcpHeaders;
    await t.test('approval completes into a grant that lists the display identity and last use', async () => {
      const approved = await post(`/pairings/${first.id}/approve`, { clientId: client.client_id, resource: `${origin}/mcp`,
        space: candidate.space, approved: true }, deviceHeaders);
      assert.equal(approved.status, 200);
      assert.equal((await post('/pairings/lookup', { code: userCode }, deviceHeaders)).status, 404, 'an approved code no longer resolves');
      const completed = await post('/pairing/complete', {}, { cookie: first.cookie, origin });
      assert.equal(completed.status, 200, await completed.clone().text());
      const redirect = new URL((await completed.json()).redirectTo);
      const exchanged = await request('/oauth/token', { method: 'POST',
        headers: { 'content-type': 'application/x-www-form-urlencoded' }, body: new URLSearchParams({
          grant_type: 'authorization_code', code: redirect.searchParams.get('code'), code_verifier: verifier,
          client_id: client.client_id, redirect_uri: callback, resource: `${origin}/mcp` }).toString() });
      assert.equal(exchanged.status, 200, await exchanged.clone().text());
      tokens = await exchanged.json();
      const before = (await (await request('/grants', { headers: deviceHeaders })).json()).items;
      assert.equal(before.length, 1);
      grantId = before[0].id;
      assert.deepEqual({ clientName: before[0].clientName, redirectHost: before[0].redirectHost,
        knownClient: before[0].knownClient, lastUsedAt: before[0].lastUsedAt, status: before[0].status },
      { clientName: 'Totally Official', redirectHost: 'claude.ai', knownClient: true, lastUsedAt: null, status: 'active' });
      assert.equal('endReason' in before[0], false);
      const started = Date.now();
      const used = await post('/mcp', { jsonrpc: '2.0', id: 0, method: 'initialize' }, { authorization: `Bearer ${tokens.access_token}` });
      assert.equal(used.status, 200, await used.clone().text());
      await used.text();
      mcpHeaders = { authorization: `Bearer ${tokens.access_token}`, 'mcp-session-id': used.headers.get('mcp-session-id') };
      const after = (await (await request('/grants', { headers: deviceHeaders })).json()).items[0];
      assert(after.lastUsedAt >= started && after.lastUsedAt <= Date.now(), String(after.lastUsedAt));
    });

    await t.test('/mcp is limited per grant with Retry-After', async () => {
      // One address, as when an AI app calls from its own servers: accepted
      // tokens are refunded from the address's credential budget, so only the
      // per-grant limit applies.
      const call = () => post('/mcp', { jsonrpc: '2.0', id: 1, method: 'tools/list' },
        { ...mcpHeaders, 'cf-connecting-ip': '198.51.100.99' });
      let served = 0;
      let limited;
      // 120 per grant per minute; a minute boundary mid-loop at most doubles the budget.
      for (let n = 0; n < 250 && !limited; n++) {
        const response = await call();
        if (response.status === 429) { limited = response; break; }
        assert.equal(response.status, 200);
        served++;
        await response.arrayBuffer();
      }
      assert(limited, 'the per-grant limit was never reached');
      // The initialize call above already spent one request of this minute.
      assert(served >= 119, `served ${served} before the limit`);
      assert.deepEqual(await limited.json(), { error: 'Request limit reached' });
      const delay = Number(limited.headers.get('retry-after'));
      assert(delay >= 1 && delay <= 60, String(delay));
    });

    await t.test('a revoked grant gets 401 with the re-authorization challenge and lists revoked', async () => {
      const revoked = await post(`/grants/${grantId}/revoke`, {}, deviceHeaders);
      assert.equal(revoked.status, 200, await revoked.clone().text());
      const listed = (await (await request('/grants', { headers: deviceHeaders })).json()).items[0];
      assert.equal(listed.status, 'inactive');
      assert.equal(listed.endReason, 'revoked');
      const denied = await post('/mcp', { jsonrpc: '2.0', id: 2, method: 'tools/list' }, mcpHeaders);
      assert.equal(denied.status, 401);
      const challenge = denied.headers.get('www-authenticate');
      assert.match(challenge, /^Bearer /);
      assert.match(challenge, /error="invalid_token"/);
      assert.match(challenge, new RegExp(`resource_metadata="${origin}/\\.well-known/oauth-protected-resource/mcp"`));
    });

    await t.test('more than ten lookup misses per device are refused with Retry-After', async () => {
      // The approved code looked up earlier was the first miss.
      for (let n = 1; n < 10; n++) {
        const miss = await post('/pairings/lookup', { code: n % 2 ? '2222-2222' : 'not a code' }, deviceHeaders);
        assert.equal(miss.status, 404, `miss ${n + 1}`);
        assert.deepEqual(await miss.json(), { error: 'Pairing unavailable' });
      }
      const fresh = await begin();
      const page = await (await request('/pairing', { headers: { cookie: fresh.cookie } })).text();
      const code = /<output id="user-code">([^<]+)<\/output>/.exec(page)[1];
      const limited = await post('/pairings/lookup', { code }, deviceHeaders);
      assert.equal(limited.status, 429);
      assert.deepEqual(await limited.json(), { error: 'Too many pairing lookups' });
      const delay = Number(limited.headers.get('retry-after'));
      assert(delay >= 1 && delay <= 600, String(delay));
      assert.equal((await request(`/pairings/${fresh.id}`, { headers: deviceHeaders })).status, 200,
        'the long pairing ID still works for the app code field');
    });
  } finally { await runtime.dispose(); }
});
