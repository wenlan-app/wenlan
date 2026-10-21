// SPDX-License-Identifier: Apache-2.0
import type { PairingStore, Transaction, WebAccess } from './pairing.ts';
import { WRITE_SCOPE, type QueryGrant } from './proxy.ts';
import type { SessionIdentity } from './sessions.ts';
import { hashSecret } from './secrets.ts';
import { authenticatedDeviceTransaction, deviceKey, type DeviceRecord } from './devices.ts';

export interface GrantReceipt {
  id: string; clientId: string; subject: string; connectorId: string; space: string;
  generation: number; owner: string; active: boolean; createdAt: number; expiresAt: number;
  cleanupPending?: boolean;
  cleanupAfter?: number;
  cleanupFailures?: number;
  cleanupLease?: string;
  authorizationId: string;
  // Added later; receipts written before these fields read as unknown/null.
  clientName?: string | null;
  redirectHost?: string | null;
  knownClient?: boolean;
  lastUsedAt?: number;
  revokedAt?: number;
  // Receipts written before write access existed read as 'query'.
  access?: WebAccess;
}
export interface ClientDisplay { clientName: string | null; redirectHost: string; knownClient: boolean }
export interface ClientAuthorization extends Partial<ClientDisplay> { authorizationId: string; expiresAt: number }
export type GrantEndReason = 'revoked' | 'reset' | 'expired' | 'replaced';
export interface GrantView {
  id: string; clientId: string; space: string; createdAt: number; expiresAt: number;
  status: 'active' | 'inactive'; cleanupPending: boolean;
  clientName: string | null; redirectHost: string | null; knownClient: boolean;
  lastUsedAt: number | null;
  access: WebAccess;
  endReason?: GrantEndReason;
}
/** lastUsedAt is written at most once per window per grant. */
export const LAST_USED_WRITE_MS = 15 * 60 * 1000;
// Absolute cap; the OAuth refresh token separately lapses after 30 idle days.
const MAX_GRANT_MS = 90 * 24 * 60 * 60 * 1000;
export const validGrantId = (value: string) => /^[A-Za-z0-9_-]{16,128}$/.test(value);
const key = (subject: string, id: string) => `oauth-grant:${subject}:${id}`;
export const clientKey = async (subject: string, clientId: string) => `oauth-client:${subject}:${await hashSecret(clientId)}`;

/** Called only after consuming a valid device-approved pairing. Reauthorization
 * replaces prior consent for this client before eventual KV revocation runs.
 */
export async function replaceClientAuthorization(
  store: PairingStore, subject: string, clientId: string, authorizationId: string, client?: ClientDisplay,
) {
  const key = await clientKey(subject, clientId);
  await store.transaction(tx => tx.put(key, { authorizationId, expiresAt: Date.now() + MAX_GRANT_MS,
    ...(client ? { clientName: client.clientName, redirectHost: client.redirectHost, knownClient: client.knownClient } : {}),
  } satisfies ClientAuthorization));
}

/** The end of this client's current consent, or null once it was replaced or expired. */
async function consentEnd(tx: Transaction, subject: string, clientId: string, authorizationId: string) {
  const current = await tx.get<ClientAuthorization>(await clientKey(subject, clientId));
  return !!current && current.authorizationId === authorizationId
    && Number.isFinite(current.expiresAt) && current.expiresAt > Date.now() ? current.expiresAt : null;
}
async function currentAuthorization(tx: Transaction, subject: string, clientId: string, authorizationId: string) {
  return await consentEnd(tx, subject, clientId, authorizationId) !== null;
}
async function binding(identity: SessionIdentity, grant: Pick<QueryGrant, 'subject' | 'connectorId' | 'space' | 'generation'>) {
  return { key: key(grant.subject, identity.grantId),
    owner: await hashSecret(JSON.stringify([identity.clientId, grant.connectorId, grant.space, grant.generation])) };
}

/** The provider validates client, PKCE and resource before invoking this claim.
 * KV read/modify/write alone cannot make authorization-code consumption atomic.
 */
export async function claimAuthorizationGrant(
  store: PairingStore, identity: SessionIdentity,
  grant: Pick<QueryGrant, 'subject' | 'connectorId' | 'space' | 'generation'> & { scopes?: readonly string[] },
  authorizationId: string,
): Promise<boolean> {
  const bound = await binding(identity, grant);
  return store.transaction(async tx => {
    if (!await currentAuthorization(tx, grant.subject, identity.clientId, authorizationId)) return false;
    const existing = await tx.get<GrantReceipt>(bound.key);
    if (existing) {
      await tx.put(bound.key, { ...existing, active: false });
      return false;
    }
    // Display identity was fixed at consent from the validated redirect URI.
    const consent = await tx.get<ClientAuthorization>(await clientKey(grant.subject, identity.clientId));
    const now = Date.now();
    await tx.put(bound.key, { subject: grant.subject, connectorId: grant.connectorId, space: grant.space,
      generation: grant.generation, id: identity.grantId, clientId: identity.clientId,
      authorizationId,
      owner: bound.owner, active: true, createdAt: now, expiresAt: now + MAX_GRANT_MS,
      clientName: typeof consent?.clientName === 'string' ? consent.clientName : null,
      redirectHost: typeof consent?.redirectHost === 'string' && consent.redirectHost ? consent.redirectHost : null,
      knownClient: consent?.knownClient === true,
      access: grant.scopes?.includes(WRITE_SCOPE) ? 'write' : 'query' } satisfies GrantReceipt);
    return true;
  });
}

export async function listDeviceGrants(store: PairingStore, deviceId: string, credential: string, cursor?: string) {
  if (cursor !== undefined && !validGrantId(cursor)) return null;
  return authenticatedDeviceTransaction(store, deviceId, credential, async (tx, device) => {
    const prefix = `oauth-grant:${device.subject}:`;
    const records = await tx.list<GrantReceipt>({ prefix,
      ...(cursor ? { startAfter: key(device.subject, cursor) } : {}), limit: 26 });
    const page = [...records.values()].slice(0, 25);
    const now = Date.now();
    const items: GrantView[] = [];
    for (const record of page) {
      if (record.subject !== device.subject || record.connectorId !== device.id) continue;
      const consent = await consentEnd(tx, record.subject, record.clientId, record.authorizationId);
      const active = record.active && record.generation === device.generation && record.expiresAt > now && consent !== null;
      // The latest possible end: the grant, its consent and the device credential each cap it.
      const expiresAt = Math.min(record.expiresAt, consent ?? record.expiresAt, device.credentialExpiresAt);
      const endReason = active ? undefined
        : await grantEndReason(tx, record, device.generation, now);
      items.push({ id: record.id, clientId: record.clientId, space: record.space,
        createdAt: record.createdAt, expiresAt, cleanupPending: record.cleanupPending === true,
        status: active ? 'active' : 'inactive',
        clientName: typeof record.clientName === 'string' ? record.clientName : null,
        redirectHost: typeof record.redirectHost === 'string' && record.redirectHost ? record.redirectHost : null,
        knownClient: record.knownClient === true,
        lastUsedAt: Number.isFinite(record.lastUsedAt) ? record.lastUsedAt! : null,
        access: record.access === 'write' ? 'write' : 'query',
        ...(endReason ? { endReason } : {}) });
    }
    return { items, ...(records.size > 25 ? { cursor: page.at(-1)!.id } : {}) };
  });
}

/** Why an inactive grant ended, from current state. An explicit revoke wins;
 * then a device generation change (Space changed or Web access renewed); then
 * consent superseded by a newer authorization of the same client; then expiry.
 */
export async function grantEndReason(
  tx: Transaction, record: GrantReceipt, deviceGeneration: number, now: number,
): Promise<GrantEndReason> {
  if (Number.isFinite(record.revokedAt)) return 'revoked';
  if (record.generation !== deviceGeneration) return 'reset';
  const consent = await tx.get<ClientAuthorization>(await clientKey(record.subject, record.clientId));
  if (consent && consent.authorizationId !== record.authorizationId) return 'replaced';
  if (!Number.isFinite(record.expiresAt) || record.expiresAt <= now || !consent
    || !Number.isFinite(consent.expiresAt) || consent.expiresAt <= now) return 'expired';
  // Deactivated without a recorded cause (for example a replayed code).
  return 'revoked';
}

/** Records use of an active grant, at most once per LAST_USED_WRITE_MS. */
export async function touchGrant(
  store: PairingStore, identity: SessionIdentity,
  grant: Pick<QueryGrant, 'subject' | 'connectorId' | 'space' | 'generation'>, now = Date.now(),
): Promise<boolean> {
  const bound = await binding(identity, grant);
  return store.transaction(async tx => {
    const receipt = await tx.get<GrantReceipt>(bound.key);
    if (!receipt || !receipt.active || receipt.owner !== bound.owner) return false;
    if (Number.isFinite(receipt.lastUsedAt) && now - receipt.lastUsedAt! < LAST_USED_WRITE_MS) return false;
    await tx.put(bound.key, { ...receipt, lastUsedAt: now });
    return true;
  });
}

/** Authoritative denial is committed before any eventually consistent token
 * deletion. A failed cleanup is recorded and can be retried idempotently.
 */
export async function revokeDeviceGrant(
  store: PairingStore, deviceId: string, credential: string, grantId: string,
  revokeTokens: (grantId: string, subject: string) => Promise<void>,
): Promise<{ revoked: true; cleanupPending: boolean } | null> {
  if (!validGrantId(grantId)) return null;
  const subject = await authenticatedDeviceTransaction(store, deviceId, credential, async (tx, device) => {
    const receipt = await tx.get<GrantReceipt>(key(device.subject, grantId));
    if (!receipt || receipt.subject !== device.subject || receipt.connectorId !== device.id) return null;
    await tx.put(key(device.subject, grantId), { ...receipt, active: false, cleanupPending: true,
      ...(receipt.active && !Number.isFinite(receipt.revokedAt) ? { revokedAt: Date.now() } : {}) });
    return device.subject;
  });
  if (!subject) return null;
  try {
    await revokeTokens(grantId, subject);
    await store.transaction(async tx => {
      const receipt = await tx.get<GrantReceipt>(key(subject, grantId));
      if (receipt && !receipt.active) await tx.put(key(subject, grantId), { ...receipt, cleanupPending: false });
    });
    return { revoked: true, cleanupPending: false };
  } catch { return { revoked: true, cleanupPending: true }; }
}

/** When an active grant must end at the latest (grant receipt, consent or
 * device credential, whichever is first), or null when it is no longer active.
 */
export async function authorizationGrantEnd(
  store: PairingStore, identity: SessionIdentity,
  grant: Pick<QueryGrant, 'subject' | 'connectorId' | 'space' | 'generation'>,
): Promise<number | null> {
  const bound = await binding(identity, grant);
  return store.transaction(async tx => {
    const receipt = await tx.get<GrantReceipt>(bound.key);
    if (!receipt || !receipt.active || receipt.owner !== bound.owner
      || !Number.isFinite(receipt.expiresAt) || receipt.expiresAt <= Date.now()) return null;
    const device = await tx.get<DeviceRecord>(deviceKey(receipt.connectorId));
    if (!device || device.id !== receipt.connectorId || device.subject !== receipt.subject
      || !Number.isFinite(device.expiresAt) || device.expiresAt <= Date.now()) return null;
    const consent = await consentEnd(tx, receipt.subject, receipt.clientId, receipt.authorizationId);
    return consent === null ? null : Math.min(receipt.expiresAt, consent, device.expiresAt);
  });
}

export async function authorizationGrantActive(
  store: PairingStore, identity: SessionIdentity,
  grant: Pick<QueryGrant, 'subject' | 'connectorId' | 'space' | 'generation'>,
): Promise<boolean> {
  return await authorizationGrantEnd(store, identity, grant) !== null;
}
