// SPDX-License-Identifier: Apache-2.0
import { WRITE_SCOPE, type ConnectorRoute, type QueryGrant } from './proxy.ts';
import { validSecret as validId, randomSecret, hashSecret as hash, sameHash } from './secrets.ts';
import { clientIdentity, displayName, type ClientIdentity } from './client-identity.ts';

/** Production adapters must provide serializable, durable transactions.
 * Cloudflare KV is not an implementation of this contract.
 */
export interface Transaction {
  get<T>(key: string): Promise<T | undefined>;
  put<T>(key: string, value: T): Promise<void>;
  delete(key: string): Promise<boolean>;
  list<T>(options: { prefix: string; startAfter?: string; limit: number }): Promise<Map<string, T>>;
}
export interface PairingStore {
  transaction<T>(operation: (tx: Transaction) => Promise<T>): Promise<T>;
}

/** Trusted OAuth adapter output after client/redirect/resource/PKCE validation.
 * authorizationId identifies the immutable server-stored OAuth request.
 * Never construct this by casting an incoming JSON body.
 */
export interface AuthorizationIntent {
  authorizationId: string;
  clientId: string;
  resource: string;
  scopes: readonly string[];
}

/** Trusted device-auth adapter output, not a client-supplied identity. */
export interface DeviceIdentity {
  id: string;
  subject: string;
  generation: number;
  credentialExpiresAt: number;
}

/** What the user let the app do, chosen on the desktop. Records approved
 * before this existed have no access and read as 'query'.
 */
export type WebAccess = 'query' | 'write';
interface Approval {
  subject: string;
  connectorId: string;
  space: string;
  generation: number;
  access?: WebAccess;
}
// Identity fields and userCode are optional so records written before they
// existed still read as unknown clients without a short code.
interface PairingRecord extends AuthorizationIntent, Partial<ClientIdentity> {
  browserHash: string;
  expiresAt: number;
  status: 'pending' | 'approved' | 'consumed' | 'cancelled' | 'denied';
  approval?: Approval;
  userCode?: string;
}
interface CodeIndex { pairingId: string; expiresAt: number }
interface LookupMisses { count: number; expiresAt: number }

export const PAIRING_TTL_MS = 10 * 60 * 1000;
/** Short code alphabet: no 0/O, 1/I/L or U, so it reads aloud unambiguously. */
export const USER_CODE_ALPHABET = '23456789ABCDEFGHJKMNPQRSTVWXYZ';
export const USER_CODE_LENGTH = 8;
export const LOOKUP_MISS_LIMIT = 10;
export const LOOKUP_MISS_WINDOW_MS = 10 * 60 * 1000;
const pairingKey = (id: string) => `pairing:${id}`;
const codeKey = (code: string) => `pairing-code:${code}`;
const missKey = (deviceId: string) => `pairing-lookup-miss:${deviceId}`;
export const connectorKey = (id: string) => `connector:${id}`;

function live(record: PairingRecord | undefined, now: number): record is PairingRecord {
  return !!record && Number.isFinite(record.expiresAt) && record.expiresAt > now
    && (record.status === 'pending' || record.status === 'approved');
}
/** Live for the browser: a denial stays visible until the page cancels it. */
function visible(record: PairingRecord | undefined, now: number): record is PairingRecord {
  if (!record || !Number.isFinite(record.expiresAt) || record.expiresAt <= now) return false;
  return record.status === 'pending' || record.status === 'approved' || record.status === 'denied';
}
function identityOf(record: PairingRecord) {
  return { clientName: typeof record.clientName === 'string' ? record.clientName : null,
    redirectHost: typeof record.redirectHost === 'string' ? record.redirectHost : '',
    knownClient: record.knownClient === true };
}

/** Uniform over the alphabet: bytes at or above 240 (8 * 30) are rejected. */
export function generateUserCode(random: (size: number) => Uint8Array = size => crypto.getRandomValues(new Uint8Array(size))): string {
  let code = '';
  while (code.length < USER_CODE_LENGTH) {
    for (const byte of random(16)) {
      if (byte >= 240) continue;
      code += USER_CODE_ALPHABET[byte % USER_CODE_ALPHABET.length];
      if (code.length === USER_CODE_LENGTH) break;
    }
  }
  return code;
}

/** Accepts dashes, spaces and lowercase. Returns null when it cannot be a code. */
export function normalizeUserCode(value: unknown): string | null {
  if (typeof value !== 'string' || value.length > 32) return null;
  const code = value.replace(/[\s-]/g, '').toUpperCase();
  if (code.length !== USER_CODE_LENGTH) return null;
  for (const character of code) if (!USER_CODE_ALPHABET.includes(character)) return null;
  return code;
}

export const formatUserCode = (code: string) => `${code.slice(0, 4)}-${code.slice(4)}`;

async function dropCode(tx: Transaction, record: PairingRecord, pairingId: string) {
  if (!record.userCode) return;
  const index = await tx.get<CodeIndex>(codeKey(record.userCode));
  if (index?.pairingId === pairingId) await tx.delete(codeKey(record.userCode));
}
function matchingRoute(route: ConnectorRoute | undefined, approval: Approval, now: number): boolean {
  return !!route && route.enabled === true && Number.isFinite(route.expiresAt) && route.expiresAt > now
    && route.id === approval.connectorId && route.subject === approval.subject
    && route.space === approval.space && route.generation === approval.generation;
}

export async function beginPairing(
  store: PairingStore, intent: AuthorizationIntent, publicResource: string, now = Date.now(),
  client: { redirectUri?: string; clientName?: string | null } = {},
): Promise<{ pairingId: string; browserSecret: string; expiresAt: number; userCode: string }> {
  if (!validId(intent.authorizationId) || !intent.clientId || intent.clientId.length > 2048
    || intent.resource !== publicResource || !publicResource.startsWith('https://')
    || intent.scopes.length !== 1 || intent.scopes[0] !== 'wenlan:query') {
    throw new Error('Unsupported authorization intent');
  }
  const pairingId = randomSecret();
  const browserSecret = randomSecret();
  const browserHash = await hash(browserSecret);
  const expiresAt = now + PAIRING_TTL_MS;
  const identity = clientIdentity(client.redirectUri, client.clientName);
  const userCode = await store.transaction(async tx => {
    const authorizationKey = `authorization-pairing:${intent.authorizationId}`;
    if (await tx.get(authorizationKey)) throw new Error('Authorization already paired');
    // Regenerate on collision with a live code; an expired index is reusable.
    let code = '';
    for (let attempt = 0; ; attempt++) {
      if (attempt >= 8) throw new Error('User code unavailable');
      code = generateUserCode();
      const existing = await tx.get<CodeIndex>(codeKey(code));
      if (!existing || !Number.isFinite(existing.expiresAt) || existing.expiresAt <= now) break;
    }
    await tx.put(pairingKey(pairingId), {
      authorizationId: intent.authorizationId, clientId: intent.clientId,
      resource: intent.resource, scopes: [...intent.scopes], browserHash,
      expiresAt, status: 'pending', ...identity, userCode: code,
    } satisfies PairingRecord);
    await tx.put(authorizationKey, { pairingId, expiresAt });
    await tx.put(codeKey(code), { pairingId, expiresAt } satisfies CodeIndex);
    return code;
  });
  // Transport adapter must keep browserSecret in an HttpOnly/Secure cookie,
  // never in the pairing code, URL, model context or desktop response.
  return { pairingId, browserSecret, expiresAt, userCode };
}

export async function inspectPairing(store: PairingStore, pairingId: string, now = Date.now()) {
  if (!validId(pairingId)) return null;
  return store.transaction(async tx => {
    return pendingView(tx, pairingId, now);
  });
}

async function pendingView(tx: Transaction, pairingId: string, now: number) {
  const record = await tx.get<PairingRecord>(pairingKey(pairingId));
  if (!live(record, now) || record.status !== 'pending') return null;
  return { pairingId, clientId: record.clientId, resource: record.resource,
    scopes: [...record.scopes], expiresAt: record.expiresAt, ...identityOf(record) };
}

export type LookupResult =
  | { status: 'found'; view: NonNullable<Awaited<ReturnType<typeof pendingView>>> }
  | { status: 'missing' }
  | { status: 'limited'; retryAfter: number };

/** Short-code lookup for an authenticated device. Misses are counted per
 * device in a fixed window; once the limit is reached every lookup is refused
 * until the window ends, so the 39-bit code cannot be enumerated.
 */
export async function lookupPairing(
  store: PairingStore, deviceId: string, code: unknown, now = Date.now(),
): Promise<LookupResult> {
  if (!validId(deviceId)) return { status: 'missing' };
  return store.transaction(async tx => {
    const saved = await tx.get<LookupMisses>(missKey(deviceId));
    const misses = saved && Number.isFinite(saved.expiresAt) && saved.expiresAt > now
      && Number.isSafeInteger(saved.count) ? saved : null;
    if (misses && misses.count >= LOOKUP_MISS_LIMIT) {
      return { status: 'limited', retryAfter: Math.max(1, Math.ceil((misses.expiresAt - now) / 1000)) };
    }
    const normalized = normalizeUserCode(code);
    const index = normalized ? await tx.get<CodeIndex>(codeKey(normalized)) : undefined;
    const view = index && Number.isFinite(index.expiresAt) && index.expiresAt > now
      ? await pendingView(tx, index.pairingId, now) : null;
    const record = view ? await tx.get<PairingRecord>(pairingKey(view.pairingId)) : undefined;
    if (view && record?.userCode === normalized) return { status: 'found', view };
    await tx.put(missKey(deviceId), misses ? { ...misses, count: misses.count + 1 }
      : { count: 1, expiresAt: now + LOOKUP_MISS_WINDOW_MS } satisfies LookupMisses);
    return { status: 'missing' };
  });
}

/** An authenticated device declines a pending request. The browser shows the
 * denial and returns the client an access_denied error through cancel.
 */
export async function denyPairing(
  store: PairingStore, pairingId: string, device: Pick<DeviceIdentity, 'id'>, now = Date.now(),
): Promise<boolean> {
  if (!validId(pairingId) || !validId(device.id)) return false;
  return store.transaction(async tx => {
    const record = await tx.get<PairingRecord>(pairingKey(pairingId));
    if (!live(record, now) || record.status !== 'pending') return false;
    await dropCode(tx, record, pairingId);
    await tx.put(pairingKey(pairingId), { ...record, status: 'denied' } satisfies PairingRecord);
    return true;
  });
}

/** Browser-only status, authenticated by the HttpOnly cookie secret. */
export async function browserPairingView(store: PairingStore, pairingId: string, browserSecret: string, clock = Date.now) {
  if (!validId(pairingId) || !validId(browserSecret)) return null;
  const browserHash = await hash(browserSecret);
  return store.transaction(async tx => {
    const record = await tx.get<PairingRecord>(pairingKey(pairingId));
    if (!visible(record, clock()) || !sameHash(record.browserHash, browserHash)) return null;
    const identity = identityOf(record);
    return { pairingId, clientId: record.clientId, status: record.status,
      redirectHost: identity.redirectHost, knownClient: identity.knownClient,
      displayName: displayName(identity),
      ...(record.status === 'pending' && record.userCode ? { userCode: record.userCode } : {}),
      ...(record.status === 'approved' && record.approval ? { space: record.approval.space } : {}) };
  });
}

/** Called only after trusted identity authentication AND explicit consent. */
export async function approvePairing(
  store: PairingStore, pairingId: string, device: DeviceIdentity,
  consent: { clientId: string; resource: string; space: string; access?: WebAccess }, clock = Date.now,
): Promise<boolean> {
  if (!validId(pairingId) || !validId(device.id)) return false;
  return store.transaction(async tx => {
    const record = await tx.get<PairingRecord>(pairingKey(pairingId));
    if (!live(record, clock()) || record.status !== 'pending'
      || record.clientId !== consent.clientId || record.resource !== consent.resource) return false;
    const route = await tx.get<ConnectorRoute>(connectorKey(device.id));
    const now = clock();
    if (!live(record, now) || !route || route.enabled !== true
      || !Number.isFinite(route.expiresAt) || route.expiresAt <= now
      || !Number.isFinite(device.credentialExpiresAt) || device.credentialExpiresAt <= now
      || route.id !== device.id || route.subject !== device.subject
      || route.generation !== device.generation
      || route.space !== consent.space || !route.space.trim()
      || !Number.isSafeInteger(route.generation) || route.generation < 0) return false;
    await tx.put(pairingKey(pairingId), { ...record, status: 'approved', approval: {
      subject: route.subject, connectorId: route.id, space: route.space,
      generation: route.generation, access: consent.access === 'write' ? 'write' : 'query',
    } } satisfies PairingRecord);
    return true;
  });
}

export interface ConsumedPairing {
  authorizationId: string;
  clientId: string;
  resource: string;
  // The OAuth adapter supplies verified token expiry separately on each call.
  grant: Omit<QueryGrant, 'expiresAt'>;
  client: { clientName: string | null; redirectHost: string; knownClient: boolean };
}

/** Consumes once before calling the OAuth library. A later issuance failure
 * requires a fresh pairing; never replay completeAuthorization after ambiguity.
 */
export async function consumePairing(
  store: PairingStore, pairingId: string, browserSecret: string, clock = Date.now,
): Promise<ConsumedPairing | null> {
  if (!validId(pairingId) || !validId(browserSecret)) return null;
  const browserHash = await hash(browserSecret);
  return store.transaction(async tx => {
    const record = await tx.get<PairingRecord>(pairingKey(pairingId));
    if (!live(record, clock()) || record.status !== 'approved' || !record.approval
      || !sameHash(record.browserHash, browserHash)) return null;
    const route = await tx.get<ConnectorRoute>(connectorKey(record.approval.connectorId));
    const now = clock();
    if (!live(record, now) || !matchingRoute(route, record.approval, now)) return null;
    await dropCode(tx, record, pairingId);
    await tx.put(pairingKey(pairingId), { ...record, status: 'consumed' } satisfies PairingRecord);
    // The request scope is always wenlan:query; the user's choice adds write.
    const { access, ...approval } = record.approval;
    return {
      authorizationId: record.authorizationId, clientId: record.clientId, resource: record.resource,
      grant: { ...approval, scopes: access === 'write' ? [...record.scopes, WRITE_SCOPE] : [...record.scopes] },
      client: identityOf(record),
    };
  });
}

export async function cancelPairing(
  store: PairingStore, pairingId: string, browserSecret: string, now = Date.now(),
): Promise<boolean> {
  return await cancelPairingRequest(store, pairingId, browserSecret, now) !== null;
}

/** Returns the cancelled request's authorizationId, or null when unavailable. */
export async function cancelPairingRequest(
  store: PairingStore, pairingId: string, browserSecret: string, now = Date.now(),
): Promise<string | null> {
  if (!validId(pairingId) || !validId(browserSecret)) return null;
  const browserHash = await hash(browserSecret);
  return store.transaction(async tx => {
    const record = await tx.get<PairingRecord>(pairingKey(pairingId));
    if (!visible(record, now) || !sameHash(record.browserHash, browserHash)) return null;
    await dropCode(tx, record, pairingId);
    await tx.put(pairingKey(pairingId), { ...record, status: 'cancelled' } satisfies PairingRecord);
    return record.authorizationId;
  });
}
