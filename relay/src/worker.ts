// SPDX-License-Identifier: Apache-2.0
import icon from '../../app/icons/128x128.png';
import { DurableObject } from 'cloudflare:workers';
import { boundedRequest, deviceCredential, failure, handlePublicRequest, HttpFailure, pairingCSS, pairingJS, type PublicEnv } from './http.ts';
import { createOAuthProvider, forwardOAuthQuery, LIMIT_HEADER } from './oauth.ts';
import { connectorKey } from './pairing.ts';
import { ACCESS_HEADER, type ConnectorRoute, type ReverseQueryRequest } from './proxy.ts';
import { hashSecret, validSecret } from './secrets.ts';
import { maintainAuthority } from './cleanup.ts';
import { boundedStore } from './bounded-store.ts';
import { domainVerification } from './domain-verification.ts';
import { ReverseChannelUnavailable, ReverseConnections } from './reverse-connections.ts';
import { authenticateReverseDevice, beginReverseActivation, commitReverseActivation, reverseConnectionCurrent, type OwnedState } from './reverse-devices.ts';
import { verifyConnectorContract } from './connector-check.ts';
import { knownClientName } from './client-identity.ts';

interface Env extends PublicEnv {
  PUBLIC_ORIGIN: string;
  AUTHORITY: DurableObjectNamespace<RelayAuthority>;
  DOMAIN_VERIFICATION_TOKEN?: string;
}

const RATE_WINDOW_CAP = 4096;
/** Authorized /mcp requests per OAuth grant per minute, after the global
 * limit. Bounds one connected client regardless of its IPs.
 */
const GRANT_REQUESTS_PER_MINUTE = 120;
/** Write tool calls (tools/call outside the query set) per write grant per
 * hour, on top of the per-minute grant limit. Bounds a runaway or misled agent.
 */
const GRANT_WRITES_PER_HOUR = 60;
/** Requests per paired device per minute on the device API. */
const DEVICE_REQUESTS_PER_MINUTE = 120;
/** Requests that present a credential skip the per-address window, because
 * AI apps call from shared servers. Each one counts here while unverified and
 * is refunded once the relay accepts it, so only rejected credentials stay
 * counted and guessing them from one address stays bounded, even when the
 * guesses arrive concurrently.
 */
const REJECTED_CREDENTIALS_PER_MINUTE = 60;
/** Client registrations per address per hour. AI apps register from their
 * own shared servers, so an allowlisted redirect host gets a larger budget.
 * The redirect host is only a claim here; the daily cap still bounds both.
 */
const REGISTRATIONS_PER_HOUR = { known: 60, other: 20 };
/** Web access enrollments (turning it on, reconnecting) per address per hour. */
const ENROLLMENTS_PER_HOUR = 10;
interface RateWindow { key: string; max: number; expires: number; name: string }
interface Admission { retryAfter: number; limit?: string; refund?: string }

/** What a request presents before the relay has checked it. */
type Claim = { kind: 'anonymous' } | { kind: 'grant' } | { kind: 'device'; id: string };

const DEVICE_PATH = /^\/(?:devices\/(?:refresh|renew|rotate|revoke|reverse\/connect|reverse\/status)|grants(?:\/[A-Za-z0-9_-]{16,128}\/revoke)?|pairings\/lookup|pairings\/[A-Za-z0-9_-]{32,128}(?:\/approve|\/deny)?)$/;

function claimOf(request: Request, path: string): Claim {
  const bearer = request.headers.get('authorization') ?? '';
  if (!bearer.startsWith('Bearer ')) return { kind: 'anonymous' };
  if (path === '/mcp') return { kind: 'grant' };
  const id = request.headers.get('x-wenlan-device-id') ?? '';
  return DEVICE_PATH.test(path) && validSecret(id) && validSecret(bearer.slice(7))
    ? { kind: 'device', id } : { kind: 'anonymous' };
}

/** True when the relay accepted the credential the request presented. A 429
 * here comes from a per-grant or per-device limit, after verification.
 */
function credentialAccepted(kind: Claim['kind'], status: number): boolean {
  if (kind === 'grant') return status !== 401;
  return status === 101 || status === 429 || (status >= 200 && status < 300);
}

/** One line per request for Workers Logs. Never request or response bodies,
 * tokens, addresses or Space names: only the route, outcome, which limit
 * fired, and short hashes that tell devices apart.
 */
interface RequestLog {
  route: string;
  method: string;
  status?: number;
  ms?: number;
  who: Claim['kind'];
  device?: string;
  app?: string;
  limit?: string;
  error?: string;
}

function routeOf(path: string): string {
  return path.replace(/^\/pairings\/[A-Za-z0-9_-]{32,128}/, '/pairings/:id')
    .replace(/^\/grants\/[A-Za-z0-9_-]{16,128}\//, '/grants/:id/').slice(0, 64);
}

function errorName(error: unknown): string {
  return error instanceof Error ? error.constructor.name.slice(0, 64) : 'unknown';
}

function logRequest(entry: RequestLog): void {
  console.log(JSON.stringify({ event: 'relay_request', ...entry }));
}

function limited(retryAfter: number): Response {
  const response = failure(429, 'Request limit reached');
  response.headers.set('retry-after', String(retryAfter));
  return response;
}

function windowOf(key: string, name: string, max: number, expires: number): RateWindow {
  return { key, name, max, expires };
}

/** One v1 authorization authority: device state and consent share a transaction
 * domain. Do not shard these independently or bind to the legacy route service.
 * Limits below are conservative initial limits, not a production capacity SLA.
 */
export class RelayAuthority extends DurableObject<Env> {
  private provider: ReturnType<typeof createOAuthProvider<Env>>;
  private store: ReturnType<typeof boundedStore>;
  private authorizationMutationActive = false;
  private connections: ReverseConnections;
  private central: boolean;
  constructor(private state: DurableObjectState, env: Env) {
    super(state, env);
    this.central = state.id.equals(env.AUTHORITY.idFromName('wenlan-v1'));
    state.storage.sql.exec('CREATE TABLE IF NOT EXISTS rate_window (key TEXT PRIMARY KEY, count INTEGER NOT NULL, expires INTEGER NOT NULL)');
    state.storage.sql.exec('CREATE INDEX IF NOT EXISTS rate_expiry ON rate_window(expires)');
    this.store = boundedStore(state.storage);
    this.connections = new ReverseConnections(state, {
      authenticate: (id, token) => this.authority().reverseAuthenticate(id, token),
      current: (id, connection, generation) => this.authority().reverseCurrent(id, connection, generation),
      activate: async (id, token, connection, probe, connected) => {
        const before = await this.authority().reverseBegin(id, token);
        if (!before || !connected() || !await verifyConnectorContract(before.contract, probe) || !connected()) return null;
        return this.authority().reverseCommit(id, token, connection, before);
      },
    }, id => !this.central && state.id.equals(env.AUTHORITY.idFromName(`wenlan-device:${id}`)));
    const loadRoute = async (id: string) => await state.storage.get<ConnectorRoute>(connectorKey(id)) ?? null;
    this.provider = createOAuthProvider<Env>(env.PUBLIC_ORIGIN, {
      fetch: (request, env) => handlePublicRequest(request, env, this.store, env.PUBLIC_ORIGIN,
        (grantId, subject) => this.provider.revokeStoredGrant(env, grantId, subject)),
    }, {
      fetch: (request, env) => forwardOAuthQuery(request, env.OAUTH_PROVIDER, env.PUBLIC_ORIGIN, loadRoute, this.store,
        undefined, (id, request, deviceId) => this.reverseFetch(deviceId, id, request),
        { limitGrant: grantId => this.limitGrant(grantId), limitWrite: grantId => this.limitGrantWrite(grantId) }),
    }, loadRoute, this.store);
  }

  private authority() { return this.env.AUTHORITY.get(this.env.AUTHORITY.idFromName('wenlan-v1')); }
  private shard(id: string) {
    if (!validSecret(id)) throw new Error('Invalid device');
    return this.env.AUTHORITY.get(this.env.AUTHORITY.idFromName(`wenlan-device:${id}`));
  }

  // Binding-only RPC. The public HTTP entrypoint always addresses the central
  // authority; it cannot call these methods or select a device object.
  async reverseAuthenticate(id: string, token: string): Promise<boolean> {
    return this.central && authenticateReverseDevice(this.store, id, token);
  }
  async reverseBegin(id: string, token: string): Promise<OwnedState | null> {
    return this.central ? beginReverseActivation(this.store, id, token) : null;
  }
  async reverseCommit(id: string, token: string, connection: string, before: OwnedState): Promise<ConnectorRoute | null> {
    return this.central ? commitReverseActivation(this.store, id, token, connection, before,
      { connected: () => true }) : null;
  }
  async reverseCurrent(id: string, connection: string, generation: number): Promise<boolean> {
    return this.central && reverseConnectionCurrent(this.store, id, connection, generation);
  }
  async closeReverseDevice(id: string): Promise<void> {
    if (this.central || !this.state.id.equals(this.env.AUTHORITY.idFromName(`wenlan-device:${id}`))) return;
    this.connections.closeDevice(id);
  }

  private async reverseFetch(deviceId: string, connectionId: string, request: ReverseQueryRequest): Promise<Response> {
    const headers = new Headers(request.headers);
    // A caller must never mint the internal upstream marker; only the device
    // object may attach it after a missing-channel rejection.
    headers.delete('x-wenlan-upstream-unavailable');
    headers.set('x-wenlan-device-id', deviceId);
    headers.set('x-wenlan-connection-id', connectionId);
    const response = await this.shard(deviceId).fetch(new Request(`${this.env.PUBLIC_ORIGIN}/mcp`, {
      method: request.method, headers, body: request.body?.byteLength ? request.body : undefined, signal: request.signal,
    }));
    if (response.status === 503 && response.headers.get('x-wenlan-upstream-unavailable') === '1') {
      await response.body?.cancel().catch(() => {});
      throw new Error('Upstream unavailable');
    }
    return response;
  }

  private async channelFetch(request: Request): Promise<Response> {
    const id = request.headers.get('x-wenlan-device-id') ?? '';
    if (!validSecret(id) || !this.state.id.equals(this.env.AUTHORITY.idFromName(`wenlan-device:${id}`))) {
      return failure(403, 'Device is not authorized');
    }
    const path = new URL(request.url).pathname;
    if (path === '/devices/reverse/connect' && request.method === 'GET') {
      await this.state.storage.setAlarm(Date.now() + 60_000);
      return this.connections.open(request);
    }
    if (path === '/devices/reverse/status' && request.method === 'GET') return this.connections.status(request);
    if (path === '/mcp' && ['GET', 'POST', 'DELETE'].includes(request.method)) {
      const buffered = await boundedRequest(request, 64 * 1024);
      const headers = new Headers();
      // x-wenlan-access is set only by forwardQuery from a verified write grant.
      for (const name of ['authorization', 'accept', 'content-type', 'mcp-session-id', 'mcp-protocol-version', 'last-event-id', ACCESS_HEADER]) {
        const value = request.headers.get(name);
        if (value !== null) headers.set(name, value);
      }
      const body = new Uint8Array(await buffered.arrayBuffer());
      try {
        return await this.connections.fetch(request.headers.get('x-wenlan-connection-id') ?? '', {
          method: request.method, headers, body, signal: request.signal,
        });
      } catch (error) {
        // Internal marker only for the typed missing-channel / dispatch
        // rejection. Authority exceptions rethrow into the outer sanitized
        // failure path and are never blanket-normalized here.
        if (!(error instanceof ReverseChannelUnavailable)) throw error;
        const unavailable = failure(503, 'Wenlan connection unavailable');
        unavailable.headers.set('x-wenlan-upstream-unavailable', '1');
        return unavailable;
      }
    }
    return failure(404, 'Not found');
  }

  private async peer(request: Request): Promise<string> {
    return hashSecret((request.headers.get('cf-connecting-ip') ?? 'unknown').slice(0, 128));
  }

  /** Runs before the body is read. Anonymous requests are keyed by address;
   * credentialed ones by their device, with rejected credentials counted per
   * address. Client registration is limited separately once its body is read.
   */
  private async limit(request: Request, claim: Claim): Promise<Admission> {
    const requestedPath = new URL(request.url).pathname;
    const path = requestedPath === '/devices/reverse' ? '/devices' : requestedPath;
    const peer = await this.peer(request);
    const now = Date.now();
    const minute = Math.floor(now / 60_000);
    const hour = Math.floor(now / 3_600_000);
    const nextMinute = (minute + 1) * 60_000;
    const windows: RateWindow[] = [windowOf(`global:${minute}`, 'global-minute', 600, nextMinute)];
    if (claim.kind === 'anonymous') {
      windows.push(windowOf(`peer:${peer}:${minute}`, 'address-minute', 120, nextMinute));
    } else {
      windows.push(windowOf(`rejected:${peer}:${minute}`, 'rejected-credentials-minute',
        REJECTED_CREDENTIALS_PER_MINUTE, nextMinute));
      if (claim.kind === 'device') windows.push(windowOf(`device:${await hashSecret(claim.id)}:${minute}`,
        'device-minute', DEVICE_REQUESTS_PER_MINUTE, nextMinute));
    }
    if (path === '/devices') windows.push(windowOf(`${path}:${peer}:${hour}`, 'enroll-address-hour',
      ENROLLMENTS_PER_HOUR, (hour + 1) * 3_600_000));
    if (path === '/pairing/sample' && request.method === 'POST') windows.push(
      windowOf(`sample-peer:${peer}:${minute}`, 'sample-address-minute', 10, nextMinute),
      windowOf(`sample-global:${minute}`, 'sample-global-minute', 60, nextMinute),
    );
    if (request.method === 'POST' && path === '/devices') {
      const day = Math.floor(now / 86_400_000);
      windows.push(windowOf(`path-global:${path}:day:${day}`, 'enroll-day', 100, (day + 1) * 86_400_000));
    }
    const admission = await this.consume(windows, now);
    return claim.kind === 'anonymous' || admission.retryAfter ? admission : { ...admission, refund: `rejected:${peer}:${minute}` };
  }

  /** Client registration (DCR) limits, applied once the bounded body is read
   * so the redirect host can pick the budget. All of its windows are checked
   * together, so a request over its hourly budget never spends the daily one.
   */
  private async limitRegistration(request: Request, buffered: Request): Promise<Admission & { app: string }> {
    let app: string | null = null;
    if (request.method === 'POST') {
      try {
        const uris = (await buffered.clone().json() as { redirect_uris?: unknown }).redirect_uris;
        const names = Array.isArray(uris) && uris.length ? uris.map(uri => knownClientName(uri)) : [null];
        app = names.every(name => name !== null && name === names[0]) ? names[0] : null;
      } catch { app = null; }
    }
    const path = '/oauth/register';
    const peer = await this.peer(request);
    const now = Date.now();
    const hour = Math.floor(now / 3_600_000);
    const windows: RateWindow[] = [app
      ? windowOf(`${path}:${app}:${peer}:${hour}`, 'register-address-hour', REGISTRATIONS_PER_HOUR.known, (hour + 1) * 3_600_000)
      : windowOf(`${path}:${peer}:${hour}`, 'register-address-hour', REGISTRATIONS_PER_HOUR.other, (hour + 1) * 3_600_000)];
    if (request.method === 'POST') {
      const day = Math.floor(now / 86_400_000);
      windows.push(windowOf(`path-global:${path}:day:${day}`, 'register-day', 250, (day + 1) * 86_400_000));
    }
    return { ...await this.consume(windows, now), app: app ?? 'other' };
  }

  /** Returns an accepted credential's provisional count to its address. */
  private refund(key: string): void {
    this.state.storage.sql.exec('UPDATE rate_window SET count = count - 1 WHERE key = ? AND count > 0', key);
  }

  /** Runs only after the bearer token was verified, keyed by its grant. */
  private async limitGrant(grantId: string): Promise<number> {
    const now = Date.now();
    const minute = Math.floor(now / 60_000);
    const grant = await hashSecret(grantId.slice(0, 256));
    return (await this.consume([windowOf(`grant:${grant}:${minute}`, 'grant-minute', GRANT_REQUESTS_PER_MINUTE,
      (minute + 1) * 60_000)], now)).retryAfter;
  }

  /** Write tool calls of a write grant, after the token is verified. */
  private async limitGrantWrite(grantId: string): Promise<number> {
    const now = Date.now();
    const hour = Math.floor(now / 3_600_000);
    const grant = await hashSecret(grantId.slice(0, 256));
    return (await this.consume([windowOf(`grant-write:${grant}:${hour}`, 'grant-write-hour', GRANT_WRITES_PER_HOUR,
      (hour + 1) * 3_600_000)], now)).retryAfter;
  }

  /** Admits only when every window has room, then counts the request in each.
   * Returns the first window that is full.
   */
  private async consume(windows: RateWindow[], now: number): Promise<Admission> {
    const admission = this.state.storage.transactionSync((): Admission => {
      const rows = windows.map(window => ({
        window,
        row: this.state.storage.sql.exec<{ count: number }>('SELECT count FROM rate_window WHERE key = ?', window.key).toArray()[0],
      }));
      for (const { window, row } of rows) {
        if (row && row.count >= window.max) {
          return { retryAfter: Math.max(1, Math.ceil((window.expires - now) / 1000)), limit: window.name };
        }
      }

      let missing = rows.filter(item => !item.row).length;
      let rowCount = this.state.storage.sql.exec<{ count: number }>('SELECT COUNT(*) AS count FROM rate_window').one().count;
      if (rowCount + missing > RATE_WINDOW_CAP) {
        this.state.storage.sql.exec(
          'DELETE FROM rate_window WHERE key IN (SELECT key FROM rate_window WHERE expires <= ? LIMIT 256)', now);
        rowCount = this.state.storage.sql.exec<{ count: number }>('SELECT COUNT(*) AS count FROM rate_window').one().count;
        missing = windows.filter(window => !this.state.storage.sql.exec(
          'SELECT key FROM rate_window WHERE key = ?', window.key).toArray()[0]).length;
        if (rowCount + missing > RATE_WINDOW_CAP) return { retryAfter: 60, limit: 'table-full' };
      }

      for (const window of windows) this.state.storage.sql.exec(
        'INSERT INTO rate_window (key,count,expires) VALUES (?,1,?) ON CONFLICT(key) DO UPDATE SET count=count+1',
        window.key, window.expires);
      return { retryAfter: 0 };
    });
    const nextAlarm = await this.state.storage.getAlarm();
    if (nextAlarm === null || nextAlarm > now + 60_000) await this.state.storage.setAlarm(now + 60_000);
    return admission;
  }

  async alarm(): Promise<void> {
    const now = Date.now();
    // Set the recovery alarm first: an exception or eviction must not abandon
    // pending retention work when no new client requests arrive.
    await this.state.storage.setAlarm(now + 60_000);
    if (!this.central) {
      await this.connections.sweep();
      if (this.state.getWebSockets().length === 0) await this.state.storage.deleteAlarm();
      return;
    }
    const maintenance = await maintainAuthority(this.store,
      (grantId, subject) => this.provider.revokeStoredGrant(this.env, grantId, subject), now);
    await this.connections.sweep();
    this.state.storage.sql.exec('DELETE FROM rate_window WHERE key IN (SELECT key FROM rate_window WHERE expires <= ? LIMIT 256)', now);
    const next = this.state.storage.sql.exec<{ expires: number | null }>('SELECT MIN(expires) AS expires FROM rate_window').one().expires;
    if (!maintenance.more && next === null) await this.state.storage.deleteAlarm();
  }

  async fetch(request: Request): Promise<Response> {
    if (!this.central) return this.respond(request);
    const started = Date.now();
    const entry: RequestLog = { route: routeOf(new URL(request.url).pathname), method: request.method.slice(0, 8), who: 'anonymous' };
    const admitted: { refund?: string } = {};
    const response = await this.respond(request, entry, admitted);
    entry.status = response.status;
    entry.ms = Date.now() - started;
    if (admitted.refund && credentialAccepted(entry.who, response.status)) {
      try { this.refund(admitted.refund); } catch { /* The window may already be reclaimed. */ }
    }
    logRequest(entry);
    return response;
  }

  private async respond(request: Request, entry?: RequestLog, admitted?: { refund?: string }): Promise<Response> {
    try {
      const url = new URL(request.url);
      if (url.origin !== this.env.PUBLIC_ORIGIN || url.href.length > 8192) return failure(400, 'Invalid request target');
      if (!this.central) return await this.channelFetch(request);
      if (url.pathname === '/mcp' && request.headers.has('origin') && request.headers.get('origin') !== this.env.PUBLIC_ORIGIN) {
        return failure(403, 'MCP origin is not authorized');
      }
      if (!['GET', 'POST', 'DELETE'].includes(request.method)) return failure(405, 'Method not allowed');
      if ((request.headers.get('authorization')?.length ?? 0) > 4096 || (request.headers.get('cookie')?.length ?? 0) > 4096) {
        return failure(431, 'Request headers too large');
      }
      const claim = claimOf(request, url.pathname);
      if (entry) {
        entry.who = claim.kind;
        if (claim.kind === 'device') entry.device = (await hashSecret(claim.id)).slice(0, 12);
        if (url.pathname === '/authorize') entry.app = knownClientName(url.searchParams.get('redirect_uri')) ?? 'other';
      }
      const admission = await this.limit(request, claim);
      if (entry) entry.limit = admission.limit;
      if (admitted) admitted.refund = admission.refund;
      if (admission.retryAfter) return limited(admission.retryAfter);
      if (url.pathname.startsWith('/devices/reverse') && (url.search || (request.method === 'GET' && request.body))) {
        return failure(400, 'Invalid connection request');
      }
      if (['/devices/reverse/connect', '/devices/reverse/status'].includes(url.pathname) && request.method === 'GET') {
        const { id, token } = deviceCredential(request);
        if (!await authenticateReverseDevice(this.store, id, token)) return failure(401, 'Device authentication failed');
        return await this.shard(id).fetch(request);
      }
      if (request.method === 'GET') {
        if (url.pathname === '/icon.png') return new Response(icon, { headers: { 'content-type': 'image/png' } });
        if (url.pathname === '/pairing.css') return new Response(pairingCSS, { headers: { 'content-type': 'text/css; charset=utf-8' } });
        if (url.pathname === '/pairing.js') return new Response(pairingJS, { headers: { 'content-type': 'text/javascript; charset=utf-8' } });
      }
      const buffered = await boundedRequest(request, url.pathname === '/mcp' ? 64 * 1024 : 16 * 1024);
      if (url.pathname === '/oauth/register') {
        const registration = await this.limitRegistration(request, buffered);
        if (entry) { entry.app = registration.app; entry.limit = registration.limit; }
        if (registration.retryAfter) return limited(registration.retryAfter);
      }
      const ctx = { waitUntil: (promise: Promise<unknown>) => this.state.waitUntil(promise),
        passThroughOnException() {}, props: {} } as ExecutionContext;
      const exchanging = url.pathname === '/oauth/token' && request.method === 'POST';
      const completing = ['/pairing/complete', '/pairing/sample'].includes(url.pathname) && request.method === 'POST';
      const mutating = exchanging || completing;
      // Issuance and reauthorization share this authority. Do not interleave
      // the provider's KV read/modify/write flows, or hold this guard while
      // buffering an untrusted body or streaming an MCP response.
      if (mutating && this.authorizationMutationActive) {
        const response = failure(503, exchanging ? 'temporarily_unavailable' : 'Authorization temporarily unavailable');
        response.headers.set('retry-after', '1');
        return response;
      }
      if (mutating) this.authorizationMutationActive = true;
      try {
        const response = await this.provider.fetch(buffered, this.env, ctx);
        if (response.ok && request.method === 'POST'
          && ['/devices/revoke', '/devices/rotate', '/devices/refresh', '/devices/renew'].includes(url.pathname)) {
          const id = request.headers.get('x-wenlan-device-id') ?? '';
          await this.shard(id).closeReverseDevice(id);
        }
        // The write limiter names its window here; the client never sees it.
        const refused = response.headers.get(LIMIT_HEADER);
        if (refused !== null) { response.headers.delete(LIMIT_HEADER); if (entry) entry.limit = refused.slice(0, 64); }
        response.headers.set('x-content-type-options', 'nosniff');
        response.headers.set('referrer-policy', 'no-referrer');
        return response;
      } finally { if (mutating) this.authorizationMutationActive = false; }
    } catch (error) {
      if (error instanceof HttpFailure) return failure(error.status, error.message);
      if (entry) entry.error = errorName(error);
      return failure(503, 'Wenlan connection unavailable');
    }
  }

  webSocketMessage(socket: WebSocket, message: string | ArrayBuffer): void { this.connections.message(socket, message); }
  webSocketClose(socket: WebSocket): void { this.connections.disconnect(socket); }
  webSocketError(socket: WebSocket): void { this.connections.disconnect(socket); }
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    try {
      const origin = new URL(env.PUBLIC_ORIGIN);
      if (origin.protocol !== 'https:' || origin.origin !== env.PUBLIC_ORIGIN
        || new URL(request.url).origin !== origin.origin) return failure(400, 'Invalid request target');
      const verification = domainVerification(request, env.DOMAIN_VERIFICATION_TOKEN);
      if (verification) return verification;
      return await env.AUTHORITY.get(env.AUTHORITY.idFromName('wenlan-v1')).fetch(request);
    } catch (error) {
      logRequest({ route: routeOf(new URL(request.url).pathname), method: request.method.slice(0, 8),
        who: 'anonymous', status: 503, error: errorName(error) });
      return failure(503, 'Wenlan connection unavailable');
    }
  },
};
