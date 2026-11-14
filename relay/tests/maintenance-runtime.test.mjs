// SPDX-License-Identifier: Apache-2.0
import assert from 'node:assert/strict';
import test from 'node:test';
import { oauthKV } from './fixtures/oauth-storage.mjs';
import { createRequire } from 'node:module';
import { resolve, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';

if (!process.env.WENLAN_RELAY_TOOLCHAIN) throw new Error('WENLAN_RELAY_TOOLCHAIN required');
const requireTool = createRequire(join(resolve(process.env.WENLAN_RELAY_TOOLCHAIN), 'package.json'));
const { Miniflare } = requireTool('miniflare');
const { build } = requireTool('esbuild');
const origin = 'https://wenlan-relay.example';

test('actual authority alarm persists cursor, deletes the OAuth grant without listing KV, and stops when empty', { timeout: 60_000 }, async () => {
  const bundle = await build({ entryPoints: [fileURLToPath(new URL('./fixtures/maintenance-worker.ts', import.meta.url))],
    bundle: true, write: false, format: 'esm', platform: 'browser', target: 'es2022',
    external: ['cloudflare:workers'], loader: { '.png': 'binary' } });
  const dir = await mkdtemp(join(tmpdir(), 'wenlan-relay-cleanup-'));
  const options = { modules: true, script: bundle.outputFiles[0].text,
    compatibilityDate: '2026-09-08', host: '127.0.0.1', port: 0,
    bindings: { PUBLIC_ORIGIN: origin }, kvNamespaces: ['OAUTH_KV'],
    kvPersist: join(dir, 'kv'), durableObjectsPersist: join(dir, 'do'),
    durableObjects: { AUTHORITY: { className: 'MaintenanceFixture', useSQLite: true } },
    outboundService: () => new Response(null, { status: 502 }) };
  let runtime = new Miniflare(options);
  const fixture = async (operation, records) => {
    const namespace = await runtime.getDurableObjectNamespace('AUTHORITY');
    const response = await namespace.get(namespace.idFromName('wenlan-v1')).fetch(`${origin}/__fixture`, {
      method: 'POST', body: JSON.stringify({ operation, records }),
    });
    assert.equal(response.status, 200, await response.clone().text());
    return response.json();
  };
  const subject = 'd'.repeat(64);
  const id = 'g'.repeat(16);
  const receiptKey = `oauth-grant:${subject}:${id}`;
  const grantKey = `grant:${subject}:${id}`;
  const tokenPrefix = `token:${subject}:${id}:`;
  try {
    const initialKV = await oauthKV(runtime);
    // Synthetic provider records. Token validity after revocation is covered
    // by the OAuth tests; this checks what physical cleanup touches.
    await initialKV.put(grantKey, JSON.stringify({ id, userId: subject }));
    for (let n = 0; n < 25; n++) await initialKV.put(`${tokenPrefix}${String(n).padStart(3, '0')}`, '{}');
    const receipt = { id, subject, connectorId: subject, clientId: 'synthetic', space: 'review', generation: 0,
      owner: 'synthetic', active: false, cleanupPending: true, authorizationId: 'a'.repeat(64),
      createdAt: Date.now(), expiresAt: Date.now() + 60_000 };
    const seed = { [receiptKey]: receipt };
    for (let i = 0; i < 65; i++) seed[`pairing:${String(i).padStart(3, '0')}`] = { expiresAt: Date.now() - 1 };
    await fixture('seed', seed);
    await fixture('schedule');
    let first;
    for (let i = 0; i < 60; i++) {
      first = await fixture('inspect');
      if (first.records[receiptKey].cleanupFailures !== undefined) break;
      await new Promise(resolve => setTimeout(resolve, 25));
    }
    assert.equal(first.records[receiptKey].cleanupFailures, 0, 'native scheduled alarm did not finish its cleanup');
    assert(first.alarm > Date.now(), 'must reschedule without any rate buckets or new traffic');
    assert(first.records['maintenance:cursor'].after);
    assert(first.records['pairing:064'], 'one alarm cannot scan unbounded rows');
    // SQL cleanup removes both the grant and every token without KV lists.
    assert.equal(first.records[receiptKey].cleanupPending, false);
    assert.equal(first.records[receiptKey].active, false);
    assert.equal(await initialKV.get(grantKey), null);
    assert.equal((await initialKV.list({ prefix: tokenPrefix })).keys.length, 0);
    await runtime.dispose();
    runtime = new Miniflare(options);
    const restored = await fixture('inspect');
    assert.deepEqual(restored.records['maintenance:cursor'], first.records['maintenance:cursor']);
    assert.equal(restored.alarm, first.alarm);
    const second = await fixture('alarm');
    assert.notDeepEqual(second.records['maintenance:cursor'], first.records['maintenance:cursor']);
    for (let i = 0; i < 3; i++) await fixture('alarm');
    assert(!(await fixture('inspect')).records['pairing:064']);
    const final = await fixture('inspect');
    assert.equal(final.records[receiptKey].cleanupPending, false);
    assert.equal(final.records[receiptKey].active, false);
    const kv = await oauthKV(runtime);
    assert.equal(await kv.get(grantKey), null);
    assert(final.alarm, 'live replay tombstone still needs eventual retention cleanup');
    await fixture('clear');
    assert.equal((await fixture('alarm')).alarm, null);
    await fixture('oauth-expired');
    const swept = await fixture('alarm');
    assert.equal(swept.oauthRows, 44, 'one alarm deletes at most 256 expired OAuth rows');
    assert(swept.alarm, 'remaining OAuth expirations must keep maintenance scheduled');
    const drained = await fixture('alarm');
    assert.equal(drained.oauthRows, 0);
    assert.equal(drained.alarm, null);
    await fixture('oauth-full');
    const refused = await runtime.dispatchFetch(`${origin}/oauth/register`, {
      method: 'POST', headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ redirect_uris: ['https://client.example/callback'],
        grant_types: ['authorization_code', 'refresh_token'], response_types: ['code'], token_endpoint_auth_method: 'none' }),
    });
    assert.equal(refused.status, 503, 'OAuth capacity failures reach the sanitized HTTP handler');
    const errorBody = await refused.text();
    assert(!errorBody.includes('capacity') && !errorBody.includes('oauth_kv') && !errorBody.includes('SQL'));
    assert.equal((await fixture('inspect')).oauthRows, 50000);

  } finally {
    await runtime.dispose();
    await rm(dir, { recursive: true, force: true });
  }
});
