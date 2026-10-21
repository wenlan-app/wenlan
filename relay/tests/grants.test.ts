// SPDX-License-Identifier: Apache-2.0
import assert from 'node:assert/strict';
import test from 'node:test';
import { authorizationGrantActive, authorizationGrantEnd, claimAuthorizationGrant, clientKey, listDeviceGrants, revokeDeviceGrant, replaceClientAuthorization, type ClientAuthorization } from '../src/grants.ts';
import { enrollDevice, rotateDeviceCredential, authenticateDevice, deviceKey, type DeviceRecord } from '../src/devices.ts';
import { MemoryStore } from './fixtures/memory-store.ts';

const identity = { grantId: 'grant-a', clientId: 'client-a' };
const grant = { subject: 'device', connectorId: 'device', space: 'review', generation: 1 };
const authorizationId = 'c'.repeat(64);
const DAY_MS = 24 * 60 * 60 * 1000;

/** The synthetic grant's device record; its credential outlives every grant unless a test says otherwise. */
async function seedDevice(store: MemoryStore, expiresAt = Date.now() + 365 * DAY_MS) {
  await store.transaction(tx => tx.put(deviceKey(grant.connectorId), { id: grant.connectorId, subject: grant.subject,
    credentialHash: 'h'.repeat(64), enabled: true, revision: 0, expiresAt } satisfies DeviceRecord));
}

test('an unclaimed or differently bound grant is never active', async () => {
  const store = new MemoryStore();
  await seedDevice(store);
  assert.equal(await authorizationGrantActive(store, identity, grant), false);
  await replaceClientAuthorization(store, grant.subject, identity.clientId, authorizationId);
  assert.equal(await claimAuthorizationGrant(store, identity, grant, authorizationId), true);
  assert.equal(await authorizationGrantActive(store, identity, grant), true);
  for (const changed of [{ ...grant, space: 'private' }, { ...grant, connectorId: 'other' },
    { ...grant, generation: 2 }, { ...grant, subject: 'other' }]) {
    assert.equal(await authorizationGrantActive(store, identity, changed), false);
  }
  assert.equal(await authorizationGrantActive(store, { ...identity, clientId: 'other' }, grant), false);
});

test('concurrent claims have one winner and replay revokes the grant durably', async () => {
  const store = new MemoryStore();
  await replaceClientAuthorization(store, grant.subject, identity.clientId, authorizationId);
  const results = await Promise.all(Array.from({ length: 20 }, () => claimAuthorizationGrant(store, identity, grant, authorizationId)));
  assert.equal(results.filter(Boolean).length, 1);
  assert.equal(await authorizationGrantActive(store, identity, grant), false);
  assert.equal(await claimAuthorizationGrant(store, identity, grant, authorizationId), false);
});

test('grant receipts stay active up to the 90-day cap and never after it', async t => {
  const store = new MemoryStore();
  const start = Date.now();
  t.mock.timers.enable({ apis: ['Date'], now: start });
  await seedDevice(store);
  await replaceClientAuthorization(store, grant.subject, identity.clientId, authorizationId);
  assert.equal(await claimAuthorizationGrant(store, identity, grant, authorizationId), true);
  assert.equal(await authorizationGrantEnd(store, identity, grant), start + 90 * DAY_MS);
  t.mock.timers.setTime(start + 31 * DAY_MS);
  assert.equal(await authorizationGrantActive(store, identity, grant), true, 'use past 30 days no longer ends the grant');
  t.mock.timers.setTime(start + 90 * DAY_MS);
  assert.equal(await authorizationGrantActive(store, identity, grant), false);
  assert.equal(await authorizationGrantEnd(store, identity, grant), null);
});

test('the grant end is the earlier of the receipt and its consent', async t => {
  const store = new MemoryStore();
  const start = Date.now();
  t.mock.timers.enable({ apis: ['Date'], now: start });
  await seedDevice(store);
  await replaceClientAuthorization(store, grant.subject, identity.clientId, authorizationId);
  t.mock.timers.setTime(start + 2 * DAY_MS);
  assert.equal(await claimAuthorizationGrant(store, identity, grant, authorizationId), true);
  assert.equal(await authorizationGrantEnd(store, identity, grant), start + 90 * DAY_MS, 'consent was given two days earlier');
  // A record written before this change keeps its stored 30-day end.
  const key = await clientKey(grant.subject, identity.clientId);
  await store.transaction(tx => tx.put(key, { authorizationId, expiresAt: start + 30 * DAY_MS } satisfies ClientAuthorization));
  assert.equal(await authorizationGrantEnd(store, identity, grant), start + 30 * DAY_MS);
});

test('a grant ends no later than its device credential, before any cleanup sweep', async t => {
  const store = new MemoryStore();
  const start = Date.now();
  t.mock.timers.enable({ apis: ['Date'], now: start });
  await seedDevice(store, start + 10 * DAY_MS);
  await replaceClientAuthorization(store, grant.subject, identity.clientId, authorizationId);
  assert.equal(await claimAuthorizationGrant(store, identity, grant, authorizationId), true);
  assert.equal(await authorizationGrantEnd(store, identity, grant), start + 10 * DAY_MS);
  t.mock.timers.setTime(start + 10 * DAY_MS);
  assert.equal(await authorizationGrantEnd(store, identity, grant), null);
  assert.equal(await authorizationGrantActive(store, identity, grant), false);
  await seedDevice(store, start + 365 * DAY_MS);
  assert.equal(await authorizationGrantActive(store, identity, grant), true, 'only the credential end denied it');
  await store.transaction(tx => tx.delete(deviceKey(grant.connectorId)));
  assert.equal(await authorizationGrantEnd(store, identity, grant), null, 'a removed device ends the grant');
});

async function managed() {
  const store = new MemoryStore();
  const candidate = { space: 'review', tunnelOrigin: 'https://demo.trycloudflare.com', backendToken: 'b'.repeat(43) };
  const fetcher = (async (_url, init) => new Headers(init?.headers).has('authorization')
    ? Response.json({ contract_version: 1, server: 'wenlan-mcp', tool_profile: 'query-only', authentication: 'bearer', space: 'review' })
    : new Response(null, { status: 401 })) as typeof fetch;
  const device = (await enrollDevice(store, candidate, { fetch: fetcher }))!;
  const other = (await enrollDevice(store, candidate, { fetch: fetcher }))!;
  const permission = { subject: device.id, connectorId: device.id, space: 'review', generation: 0 };
  const first = { grantId: 'a'.repeat(16), clientId: 'client-a' };
  const second = { grantId: 'b'.repeat(16), clientId: 'client-b' };
  await replaceClientAuthorization(store, permission.subject, first.clientId, authorizationId);
  await replaceClientAuthorization(store, permission.subject, second.clientId, authorizationId);
  await claimAuthorizationGrant(store, first, permission, authorizationId);
  await claimAuthorizationGrant(store, second, permission, authorizationId);
  return { store, device, other, permission, first, second };
}

test('device management lists only its own grants without secrets or internal authority fields', async () => {
  const state = await managed();
  const { store, device, other } = state;
  const page = await listDeviceGrants(store, device.id, device.managementToken);
  assert.equal(page!.items.length, 2);
  assert.deepEqual(Object.keys(page!.items[0]).sort(), ['id', 'clientId', 'space', 'createdAt', 'expiresAt', 'status', 'cleanupPending',
    'clientName', 'redirectHost', 'knownClient', 'lastUsedAt', 'access'].sort());
  const serialized = JSON.stringify(page);
  for (const secret of [device.managementToken, 'b'.repeat(43), 'credentialHash', 'owner', 'connectorId']) assert(!serialized.includes(secret));
  assert.equal((await listDeviceGrants(store, other.id, other.managementToken))!.items.length, 0);
  assert.equal(await listDeviceGrants(store, device.id, other.managementToken), null);
});

test('listed grant ends are capped by consent and the device credential', async () => {
  const { store, device, first, second } = await managed();
  const listed = async () => (await listDeviceGrants(store, device.id, device.managementToken))!.items;
  const grantEnd = (await listed()).find(item => item.id === first.grantId)!.expiresAt;
  assert.equal(device.expiresAt - grantEnd < 60_000, true, 'a fresh grant ends with its 90-day device credential');
  const earlier = Date.now() + 5 * DAY_MS;
  const key = await clientKey(device.id, first.clientId);
  await store.transaction(tx => tx.put(key, { authorizationId, expiresAt: earlier } satisfies ClientAuthorization));
  assert.equal((await listed()).find(item => item.id === first.grantId)!.expiresAt, earlier);
  const record = (await store.transaction(tx => tx.get<DeviceRecord>(deviceKey(device.id))))!;
  const deviceEnd = Date.now() + 3 * DAY_MS;
  await store.transaction(tx => tx.put(deviceKey(device.id), { ...record, expiresAt: deviceEnd }));
  for (const item of await listed()) assert.equal(item.expiresAt, deviceEnd);
  assert.equal((await listed()).find(item => item.id === second.grantId)!.status, 'active');
});

test('per-grant revocation denies one client without disabling the device or its other grant', async () => {
  const { store, device, permission, first, second } = await managed();
  const result = await revokeDeviceGrant(store, device.id, device.managementToken, first.grantId, async (id, subject) => {
    assert.equal(id, first.grantId);
    assert.equal(subject, device.id);
    assert.equal(await authorizationGrantActive(store, first, permission), false, 'deny must commit before token cleanup');
  });
  assert.deepEqual(result, { revoked: true, cleanupPending: false });
  assert.equal(await authorizationGrantActive(store, second, permission), true);
  assert(await authenticateDevice(store, device.id, device.managementToken));
});

test('cross-device and unauthenticated revocation never call the token store', async () => {
  const { store, device, other, permission, first } = await managed();
  let cleanupCalls = 0;
  const cleanup = async () => { cleanupCalls++; };
  assert.equal(await revokeDeviceGrant(store, other.id, other.managementToken, first.grantId, cleanup), null);
  assert.equal(await revokeDeviceGrant(store, device.id, other.managementToken, first.grantId, cleanup), null);
  assert.equal(await revokeDeviceGrant(store, device.id, device.managementToken, '../private', cleanup), null);
  assert.equal(cleanupCalls, 0);
  assert.equal(await authorizationGrantActive(store, first, permission), true);
});

test('failed token cleanup remains denied and pending until an idempotent retry succeeds', async () => {
  const { store, device, permission, first } = await managed();
  const result = await revokeDeviceGrant(store, device.id, device.managementToken, first.grantId,
    async () => { throw new Error('PRIVATE_KV_ERROR'); });
  assert.deepEqual(result, { revoked: true, cleanupPending: true });
  assert.equal(await authorizationGrantActive(store, first, permission), false);
  const pending = (await listDeviceGrants(store, device.id, device.managementToken))!.items.find(item => item.id === first.grantId)!;
  assert.equal(pending.status, 'inactive');
  assert.equal(pending.cleanupPending, true);
  assert.deepEqual(await revokeDeviceGrant(store, device.id, device.managementToken, first.grantId, async () => {}),
    { revoked: true, cleanupPending: false });
  assert.equal((await listDeviceGrants(store, device.id, device.managementToken))!.items.find(item => item.id === first.grantId)!.cleanupPending, false);
});

test('rotation invalidates old management credentials while the new credential can clean up prior grants', async () => {
  const { store, device, first } = await managed();
  const rotated = (await rotateDeviceCredential(store, device.id, device.managementToken))!;
  assert.equal(await listDeviceGrants(store, device.id, device.managementToken), null);
  assert.equal(await revokeDeviceGrant(store, device.id, device.managementToken, first.grantId, async () => { assert.fail(); }), null);
  assert((await listDeviceGrants(store, device.id, rotated.managementToken))!.items.every(item => item.status === 'inactive'));
  assert.deepEqual(await revokeDeviceGrant(store, device.id, rotated.managementToken, first.grantId, async () => {}),
    { revoked: true, cleanupPending: false });
});

test('grant pages are bounded and cursors cannot traverse another owner prefix', async () => {
  const { store, device, permission } = await managed();
  for (let n = 0; n < 28; n++) {
    await replaceClientAuthorization(store, permission.subject, `client-${n}`, authorizationId);
    await claimAuthorizationGrant(store, { grantId: String(n).padStart(16, '0'), clientId: `client-${n}` }, permission, authorizationId);
  }
  const first = (await listDeviceGrants(store, device.id, device.managementToken))!;
  assert.equal(first.items.length, 25);
  assert(first.cursor);
  const second = (await listDeviceGrants(store, device.id, device.managementToken, first.cursor))!;
  assert.equal(second.items.length, 5);
  assert.equal(second.cursor, undefined);
  assert.equal(new Set([...first.items, ...second.items].map(item => item.id)).size, 30);
  assert.equal(await listDeviceGrants(store, device.id, device.managementToken, 'x:other-prefix'), null);
});

test('replacement consent invalidates old access and late code claims without affecting another client', async () => {
  const { store, device, permission, first, second } = await managed();
  const replacement = 'd'.repeat(64);
  await replaceClientAuthorization(store, permission.subject, first.clientId, replacement);
  assert.equal(await authorizationGrantActive(store, first, permission), false);
  assert.equal(await authorizationGrantActive(store, second, permission), true);
  assert.equal(await claimAuthorizationGrant(store, { ...first, grantId: 'e'.repeat(16) }, permission, authorizationId), false);
  const current = { ...first, grantId: 'f'.repeat(16) };
  assert.equal(await claimAuthorizationGrant(store, current, permission, replacement), true);
  assert.equal(await authorizationGrantActive(store, current, permission), true);
  const page = (await listDeviceGrants(store, device.id, device.managementToken))!;
  assert.equal(page.items.find(item => item.id === first.grantId)!.status, 'inactive');
  assert.equal(page.items.find(item => item.id === current.grantId)!.status, 'active');
});

test('a grant records and lists the access its scopes carry; receipts without it read as query', async () => {
  const { store, device, permission } = await managed();
  const access = async () => Object.fromEntries((await listDeviceGrants(store, device.id, device.managementToken))!.items
    .map(item => [item.clientId, item.access]));
  // managed() claimed both grants with no scopes: the search-only default.
  assert.deepEqual(await access(), { 'client-a': 'query', 'client-b': 'query' });
  const claim = async (n: string, scopes?: string[]) => {
    await replaceClientAuthorization(store, permission.subject, `client-${n}`, authorizationId);
    return claimAuthorizationGrant(store, { grantId: n.repeat(16), clientId: `client-${n}` },
      scopes ? { ...permission, scopes } : permission, authorizationId);
  };
  assert.equal(await claim('w', ['wenlan:query', 'wenlan:write']), true);
  assert.equal(await claim('q', ['wenlan:query']), true);
  assert.deepEqual(await access(), { 'client-a': 'query', 'client-b': 'query', 'client-w': 'write', 'client-q': 'query' });
  // A receipt written before the field existed has no access key at all.
  const key = `oauth-grant:${permission.subject}:${'q'.repeat(16)}`;
  const stored = (await store.transaction(tx => tx.get<Record<string, unknown>>(key)))!;
  assert.equal(stored.access, 'query');
  const { access: _dropped, ...legacy } = stored;
  await store.transaction(tx => tx.put(key, legacy));
  assert.equal((await access())['client-q'], 'query');
  // An unknown stored value never reads as write.
  await store.transaction(tx => tx.put(key, { ...legacy, access: 'admin' }));
  assert.equal((await access())['client-q'], 'query');
});
