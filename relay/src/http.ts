// SPDX-License-Identifier: Apache-2.0
import { authenticateDevice, enrollDevice, refreshDevice, revokeDevice, rotateDeviceCredential } from './devices.ts';
import { approvePairing, browserPairingView, denyPairing, inspectPairing, lookupPairing, PAIRING_TTL_MS, type PairingStore } from './pairing.ts';
import { cancelOAuthPairing, finishOAuthPairing, startOAuthPairing, type OAuthEnv } from './oauth.ts';
import { authorizeFailureDocument, htmlEscape, negotiateLocale, pairingDocument, returnDocument, shell, spaceLabel, type PageLocale } from './pairing-page.ts';
import { AuthorityCapacityError } from './bounded-store.ts';
import { validSecret } from './secrets.ts';
import { listDeviceGrants, revokeDeviceGrant, validGrantId } from './grants.ts';
import { authorizationRead } from './proxy.ts';
import { approveSamplePairing, parseSampleAccount, type SampleAccount } from './sample-account.ts';
import { prepareReverseDevice } from './reverse-devices.ts';
export { pairingCSS } from './pairing-style.ts';
export { pairingJS } from './pairing-script.ts';

export interface PublicEnv extends OAuthEnv {
  SAMPLE_ACCOUNT?: string;
}

const COOKIE = '__Host-wenlan-pairing';
export class HttpFailure extends Error {
  constructor(public status: number, message: string) { super(message); }
}
export function failure(status: number, error: string): Response {
  return Response.json({ error }, { status, headers: { 'cache-control': 'no-store' } });
}

/** Buffer before the OAuth library or application parser. Never let a partial
 * or indefinitely streaming body hold storage/auth work open.
 */
export async function boundedRequest(request: Request, limit: number): Promise<Request> {
  const length = request.headers.get('content-length');
  if (length !== null && (!/^\d+$/.test(length) || Number(length) > limit)) throw new HttpFailure(413, 'Request too large');
  if (!request.body) return request;
  const reader = request.body.getReader();
  const deadline = AbortSignal.any([request.signal, AbortSignal.timeout(5000)]);
  const cancel = () => { void reader.cancel().catch(() => {}); };
  deadline.addEventListener('abort', cancel, { once: true });
  const chunks: Uint8Array[] = [];
  let size = 0;
  try {
    while (true) {
      if (deadline.aborted) throw new HttpFailure(408, 'Request timed out');
      const { value, done } = await reader.read();
      if (deadline.aborted) throw new HttpFailure(408, 'Request timed out');
      if (done) break;
      size += value.byteLength;
      if (size > limit) throw new HttpFailure(413, 'Request too large');
      chunks.push(value);
    }
  } finally {
    deadline.removeEventListener('abort', cancel);
    await reader.cancel().catch(() => {});
    reader.releaseLock();
  }
  const bytes = new Uint8Array(size);
  let offset = 0;
  for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.byteLength; }
  return new Request(request, { body: bytes });
}

/** Pairing complete/cancel accept the page's JSON request or, without
 * JavaScript, a plain same-origin form post (answered with a redirect).
 */
async function formOrJson(request: Request): Promise<'json' | 'form'> {
  const type = request.headers.get('content-type')?.split(';')[0].trim().toLowerCase();
  if (type === 'application/x-www-form-urlencoded') return 'form';
  await jsonObject(request);
  return 'json';
}
function seeOther(location: string, cookie?: string): Response {
  const headers = new Headers({ location, 'cache-control': 'no-store' });
  if (cookie) headers.set('set-cookie', cookie);
  return new Response(null, { status: 303, headers });
}
function returnPage(request: Request, outcome: 'approved' | 'cancelled', href: string): Response {
  const response = htmlResponse(returnDocument(negotiateLocale(request.headers.get('accept-language')), outcome, href));
  response.headers.set('set-cookie', clearCookie());
  return response;
}

async function jsonObject(request: Request): Promise<Record<string, unknown>> {
  if (request.headers.get('content-type')?.split(';')[0].trim() !== 'application/json') {
    throw new HttpFailure(415, 'JSON required');
  }
  let value;
  try { value = await request.json(); } catch { throw new HttpFailure(400, 'Invalid JSON'); }
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new HttpFailure(400, 'JSON object required');
  return value as Record<string, unknown>;
}
function stringField(body: Record<string, unknown>, name: string, max = 2048): string {
  const value = body[name];
  if (typeof value !== 'string' || !value || value.length > max) throw new HttpFailure(400, 'Invalid request fields');
  return value;
}
function candidate(body: Record<string, unknown>) {
  return { tunnelOrigin: stringField(body, 'tunnelOrigin'),
    backendToken: stringField(body, 'backendToken', 128), space: stringField(body, 'space', 256) };
}
export function deviceCredential(request: Request) {
  // Native desktop APIs do not use cookies or accept browser-origin requests.
  if (request.headers.has('origin') || request.headers.has('cookie')) throw new HttpFailure(403, 'Native device request required');
  const id = request.headers.get('x-wenlan-device-id') ?? '';
  const header = request.headers.get('authorization') ?? '';
  const token = header.startsWith('Bearer ') ? header.slice(7) : '';
  if (!validSecret(id) || !validSecret(token)) throw new HttpFailure(401, 'Device authentication required');
  return { id, token };
}
function browserCookie(request: Request) {
  const cookies = (request.headers.get('cookie') ?? '').split(';').map(value => value.trim())
    .filter(value => value.startsWith(`${COOKIE}=`));
  if (cookies.length !== 1) return null;
  const parts = cookies[0].slice(COOKIE.length + 1).split('.');
  const [id, secret] = parts;
  return parts.length === 2 && validSecret(id) && validSecret(secret) ? { id, secret } : null;
}
function setCookie(id: string, secret: string, maxAge = PAIRING_TTL_MS / 1000) {
  return `${COOKIE}=${id}.${secret}; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=${maxAge}`;
}
const clearCookie = () => `${COOKIE}=; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=0`;

function configuredSampleAccount(env: PublicEnv, publicOrigin: string): SampleAccount | null {
  if (typeof env.SAMPLE_ACCOUNT !== 'string' || env.SAMPLE_ACCOUNT.length > 4096) return null;
  try {
    const account = parseSampleAccount(JSON.parse(env.SAMPLE_ACCOUNT));
    return account && account.expiresAt > Date.now() && account.resource === `${publicOrigin}/mcp` ? account : null;
  } catch { return null; }
}

function acceptsHtml(request: Request): boolean {
  return (request.headers.get('accept') ?? '').toLowerCase().split(',')
    .some(part => part.split(';')[0].trim() === 'text/html');
}

function sameOrigin(request: Request, publicOrigin: string) {
  if (request.headers.get('origin') !== publicOrigin
    || (request.headers.has('sec-fetch-site') && request.headers.get('sec-fetch-site') !== 'same-origin')) {
    throw new HttpFailure(403, 'Same-origin request required');
  }
}

function pairingPage(view: Parameters<typeof pairingDocument>[0], locale: PageLocale, sampleAvailable = false): Response {
  return htmlResponse(pairingDocument(view, locale, sampleAvailable).html);
}

function samplePage(clientId: string, account: SampleAccount): Response {
  return authorizationPage(`<h2>Sample library</h2>
<dl><dt>Client</dt><dd>${htmlEscape(clientId)}</dd><dt>Space</dt><dd>${htmlEscape(spaceLabel('en', account.space))}</dd>
<dt>Permission</dt><dd>Read Briefs, search knowledge and inspect supporting sources.</dd></dl>
<form id="sample-login" action="/pairing/sample" method="post" data-client-id="${htmlEscape(clientId)}" data-resource="${htmlEscape(account.resource)}" data-space="${htmlEscape(account.space)}">
<label for="sample-username">Username</label><input id="sample-username" name="username" autocomplete="username" maxlength="64" required>
<label for="sample-password">Password</label><input id="sample-password" name="password" type="password" autocomplete="current-password" maxlength="64" required>
<label class="consent"><input name="approved" type="checkbox" required><span>Allow access to this Space. Searches are recorded in its activity history.</span></label>
<button type="submit" class="primary">Sign in and authorize</button></form>
<p id="notice" role="status" aria-live="polite"></p><a href="/pairing">Back to device pairing</a>`);
}

function authorizationPage(body: string, state = 'sample'): Response {
  return htmlResponse(shell('en', `<h1>Connect Wenlan</h1>${body}
<noscript><p>JavaScript is required to complete this connection.</p></noscript>`, state));
}

function htmlResponse(html: string, status = 200): Response {
  return new Response(html, {
    status,
    headers: { 'content-type': 'text/html; charset=utf-8', 'cache-control': 'no-store', 'content-language': html.match(/<html lang="([^"]+)"/)?.[1] ?? 'en', vary: 'accept-language',
      'content-security-policy': "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self'; connect-src 'self'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'",
      'referrer-policy': 'no-referrer', 'x-content-type-options': 'nosniff', 'x-frame-options': 'DENY' },
  });
}

export async function handlePublicRequest(
  request: Request, env: PublicEnv, store: PairingStore, publicOrigin: string,
  revokeTokens: (grantId: string, subject: string) => Promise<void>,
): Promise<Response> {
  const url = new URL(request.url);
  if (url.pathname === '/pairing/status' && request.method === 'GET') {
    if ((request.headers.has('origin') && request.headers.get('origin') !== publicOrigin)
      || request.headers.get('sec-fetch-site') === 'cross-site') throw new HttpFailure(403, 'Same-origin request required');
    const cookie = browserCookie(request);
    const view = cookie ? await browserPairingView(store, cookie.id, cookie.secret) : null;
    return Response.json(view ? { status: view.status, ...(view.status === 'approved' && view.space ? { space: view.space } : {}) }
      : { status: 'unavailable' }, { headers: { 'cache-control': 'no-store', 'x-content-type-options': 'nosniff' } });
  }
  if (url.pathname === '/authorize' && request.method === 'GET') {
    let pair;
    try { pair = await startOAuthPairing(env.OAUTH_PROVIDER, store, request, publicOrigin); }
    catch (error) {
      if (error instanceof AuthorityCapacityError) throw error;
      // A person following a link gets a page; programmatic callers keep JSON.
      if (acceptsHtml(request)) return htmlResponse(authorizeFailureDocument(negotiateLocale(request.headers.get('accept-language'))), 400);
      throw new HttpFailure(400, 'Authorization request rejected');
    }
    return new Response(null, { status: 303, headers: {
      location: '/pairing', 'cache-control': 'no-store',
      'set-cookie': setCookie(pair.pairingId, pair.browserSecret),
    } });
  }
  if (url.pathname === '/pairing' && request.method === 'GET') {
    const cookie = browserCookie(request);
    return pairingPage(cookie ? await browserPairingView(store, cookie.id, cookie.secret) : null,
      negotiateLocale(request.headers.get('accept-language')), configuredSampleAccount(env, publicOrigin) !== null);
  }
  if (url.pathname === '/pairing/sample') {
    const account = configuredSampleAccount(env, publicOrigin);
    if (!account) return failure(404, 'Sample library unavailable');
    const cookie = browserCookie(request);
    if (request.method === 'GET') {
      const view = cookie ? await browserPairingView(store, cookie.id, cookie.secret) : null;
      return view?.status === 'pending' ? samplePage(view.clientId, account) : failure(401, 'Active pairing required');
    }
    if (request.method === 'POST') {
      sameOrigin(request, publicOrigin);
      const body = await jsonObject(request);
      if (!cookie) return failure(401, 'Sample account sign-in rejected');
      const fields = ['username', 'password', 'clientId', 'resource', 'space'];
      if (Object.keys(body).some(key => ![...fields, 'approved'].includes(key))
        || fields.some(key => typeof body[key] !== 'string' || (body[key] as string).length > 2048)) {
        return failure(401, 'Sample account sign-in rejected');
      }
      const approved = await approveSamplePairing(store, account, {
        username: body.username as string, password: body.password as string,
        clientId: body.clientId as string, resource: body.resource as string, space: body.space as string,
        approved: body.approved === true, pairingId: cookie.id, browserSecret: cookie.secret,
      });
      if (!approved) return failure(401, 'Sample account sign-in rejected');
      const redirectTo = await finishOAuthPairing(env.OAUTH_PROVIDER, store, cookie.id, cookie.secret, publicOrigin);
      return redirectTo ? Response.json({ redirectTo }, { headers: { 'cache-control': 'no-store', 'set-cookie': clearCookie() } })
        : failure(409, 'Authorization could not be completed');
    }
    return failure(405, 'Method not allowed');
  }
  if (['/pairing/complete', '/pairing/cancel'].includes(url.pathname) && request.method === 'POST') {
    sameOrigin(request, publicOrigin);
    const mode = await formOrJson(request);
    const cookie = browserCookie(request);
    if (!cookie) {
      if (mode === 'form') return seeOther('/pairing');
      throw new HttpFailure(401, 'Pairing cookie required');
    }
    if (url.pathname.endsWith('/complete')) {
      const redirectTo = await finishOAuthPairing(env.OAUTH_PROVIDER, store, cookie.id, cookie.secret, publicOrigin);
      if (mode === 'form') return redirectTo ? returnPage(request, 'approved', redirectTo) : seeOther('/pairing');
      if (redirectTo) {
        const response = Response.json({ redirectTo }, { headers: { 'cache-control': 'no-store' } });
        response.headers.set('set-cookie', clearCookie());
        return response;
      }
      // Failed completion keeps 409 for compatibility, but must not report
      // every failure as still pending. The cookie-authenticated view
      // distinguishes pending approval from an unavailable pairing without
      // exposing why the view failed.
      const view = await browserPairingView(store, cookie.id, cookie.secret);
      const error = view?.status === 'pending'
        ? 'Device approval is still pending.'
        : view?.status === 'approved'
          ? 'Authorization could not be completed. Start a new connection in your AI client.'
          : 'This pairing has expired or is no longer available. Start a new connection in your AI client.';
      return Response.json({ redirectTo: null, error, ...(view?.status !== 'pending' ? { pairingUnavailable: true } : {}) },
        { status: 409, headers: { 'cache-control': 'no-store' } });
    }
    // The client receives access_denied at its validated redirect URI.
    const redirectTo = await cancelOAuthPairing(store, cookie.id, cookie.secret, publicOrigin);
    if (mode === 'form') return redirectTo ? returnPage(request, 'cancelled', redirectTo) : seeOther('/pairing', redirectTo === '' ? clearCookie() : undefined);
    if (redirectTo !== null) {
      const response = Response.json({ cancelled: true, ...(redirectTo ? { redirectTo } : {}) }, { headers: { 'cache-control': 'no-store' } });
      response.headers.set('set-cookie', clearCookie());
      return response;
    }
    return Response.json({ cancelled: false, pairingUnavailable: true,
      error: 'This pairing has expired or is no longer available. Start a new connection in your AI client.' },
    { status: 409, headers: { 'cache-control': 'no-store' } });
  }
  if (url.pathname === '/devices/reverse' && request.method === 'POST') {
    if (request.headers.has('origin') || request.headers.has('cookie')) throw new HttpFailure(403, 'Native device request required');
    const body = await jsonObject(request);
    if (Object.keys(body).some(key => !['backendToken', 'space'].includes(key))) throw new HttpFailure(400, 'Invalid request fields');
    const credential = await prepareReverseDevice(store, {
      backendToken: stringField(body, 'backendToken', 128), space: stringField(body, 'space', 256),
    });
    return credential ? Response.json(credential, { status: 201, headers: { 'cache-control': 'no-store' } })
      : failure(422, 'Invalid connector contract');
  }
  if (url.pathname === '/devices' && request.method === 'POST') {
    if (request.headers.has('origin') || request.headers.has('cookie')) throw new HttpFailure(403, 'Native device request required');
    const credential = await enrollDevice(store, candidate(await jsonObject(request)));
    return credential ? Response.json(credential, { status: 201, headers: { 'cache-control': 'no-store' } })
      : failure(422, 'Protected connector could not be verified');
  }
  if (['/devices/refresh', '/devices/renew', '/devices/rotate', '/devices/revoke'].includes(url.pathname) && request.method === 'POST') {
    const { id, token } = deviceCredential(request);
    const body = await jsonObject(request);
    if (url.pathname.endsWith('/rotate')) {
      const credential = await rotateDeviceCredential(store, id, token);
      return credential ? Response.json(credential, { headers: { 'cache-control': 'no-store' } }) : failure(401, 'Device authentication failed');
    }
    const renew = url.pathname.endsWith('/renew');
    const refresh = renew || url.pathname.endsWith('/refresh');
    const expectedGeneration = body.expectedGeneration;
    if (renew && (typeof expectedGeneration !== 'number' || !Number.isSafeInteger(expectedGeneration) || expectedGeneration < 0)) {
      throw new HttpFailure(400, 'Invalid request fields');
    }
    const success = refresh
      ? await refreshDevice(store, id, token, candidate(body),
        { expectedGeneration: renew ? expectedGeneration as number : undefined })
      : await revokeDevice(store, id, token);
    return success ? Response.json({ success }, { headers: { 'cache-control': 'no-store' } }) : failure(401, 'Device operation rejected');
  }
  if (url.pathname === '/grants' && request.method === 'GET') {
    const { id, token } = deviceCredential(request);
    const cursor = url.searchParams.get('cursor') ?? undefined;
    if (cursor !== undefined && !validGrantId(cursor)) throw new HttpFailure(400, 'Invalid cursor');
    const page = await listDeviceGrants(store, id, token, cursor);
    return page ? Response.json(page, { headers: { 'cache-control': 'no-store' } }) : failure(401, 'Device authentication failed');
  }
  const grantMatch = /^\/grants\/([A-Za-z0-9_-]{16,128})\/revoke$/.exec(url.pathname);
  if (grantMatch && request.method === 'POST') {
    const { id, token } = deviceCredential(request);
    await jsonObject(request);
    const result = await revokeDeviceGrant(store, id, token, grantMatch[1],
      (grantId, subject) => authorizationRead(() => revokeTokens(grantId, subject)));
    return result ? Response.json(result, { status: result.cleanupPending ? 503 : 200,
      headers: { 'cache-control': 'no-store' } }) : failure(404, 'Connection unavailable');
  }
  if (url.pathname === '/pairings/lookup' && request.method === 'POST') {
    const { id, token } = deviceCredential(request);
    const identity = await authenticateDevice(store, id, token);
    if (!identity) throw new HttpFailure(401, 'Device authentication failed');
    const body = await jsonObject(request);
    if (Object.keys(body).some(key => key !== 'code') || typeof body.code !== 'string') throw new HttpFailure(400, 'Invalid request fields');
    const result = await lookupPairing(store, identity.id, body.code);
    if (result.status === 'limited') {
      const response = failure(429, 'Too many pairing lookups');
      response.headers.set('retry-after', String(result.retryAfter));
      return response;
    }
    return result.status === 'found' ? Response.json(result.view, { headers: { 'cache-control': 'no-store' } })
      : failure(404, 'Pairing unavailable');
  }
  const pairMatch = /^\/pairings\/([A-Za-z0-9_-]{32,128})(\/approve|\/deny)?$/.exec(url.pathname);
  if (pairMatch && ((request.method === 'GET' && !pairMatch[2]) || (request.method === 'POST' && pairMatch[2]))) {
    const { id, token } = deviceCredential(request);
    const identity = await authenticateDevice(store, id, token);
    if (!identity) throw new HttpFailure(401, 'Device authentication failed');
    if (pairMatch[2] === '/deny') {
      return await denyPairing(store, pairMatch[1], identity)
        ? new Response(null, { status: 204, headers: { 'cache-control': 'no-store' } })
        : failure(404, 'Pairing unavailable');
    }
    if (request.method === 'GET') {
      const view = await inspectPairing(store, pairMatch[1]);
      return view ? Response.json(view, { headers: { 'cache-control': 'no-store' } }) : failure(404, 'Pairing unavailable');
    }
    const body = await jsonObject(request);
    if (body.approved !== true) throw new HttpFailure(400, 'Explicit consent required');
    // Older desktop builds omit access and keep the search-only default.
    const access = body.access === undefined ? 'query' : body.access;
    if (access !== 'query' && access !== 'write') throw new HttpFailure(400, 'Invalid request fields');
    const success = await approvePairing(store, pairMatch[1], identity, {
      clientId: stringField(body, 'clientId'), resource: stringField(body, 'resource'), space: stringField(body, 'space', 256), access,
    });
    return success ? Response.json({ approved: true }, { headers: { 'cache-control': 'no-store' } }) : failure(409, 'Pairing approval rejected');
  }
  return failure(404, 'Not found');
}
