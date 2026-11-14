// SPDX-License-Identifier: Apache-2.0
import assert from 'node:assert/strict';
import test from 'node:test';
import { DatabaseSync } from 'node:sqlite';
import { oauthStore, OAUTH_MIGRATION_CUTOFF } from '../src/oauth-store.ts';
import { deleteProviderGrant } from '../src/cleanup-kv.ts';

function fixture(cap = 50_000, legacyValues: Record<string, string> = {}) {
  const db = new DatabaseSync(':memory:');
  const sql = { exec(query: string, ...bindings: any[]) {
    const rows = db.prepare(query).all(...bindings);
    return { toArray: () => rows, one: () => { assert.equal(rows.length, 1); return rows[0]; } };
  } } as unknown as SqlStorage;
  let now = OAUTH_MIGRATION_CUTOFF - 86400;
  const reads: string[] = [];
  const legacy = new Proxy({ get: async (key: string) => { reads.push(key); return legacyValues[key] ?? null; } }, {
    get(target, key) { if (key in target) return Reflect.get(target, key); return () => assert.fail(`legacy ${String(key)} forbidden`); },
  }) as unknown as KVNamespace;
  const store = oauthStore(sql, legacy, () => now, cap);
  return { db, sql, store, reads, legacy, now: () => now, advance: (seconds: number) => { now += seconds; } };
}

test('SQL OAuth text/JSON, metadata, ordered prefix paging and opaque cursors', async () => {
  const { store } = fixture();
  assert.equal(await store.get('missing'), null);
  await store.put('p:b', '{"n":2}', { metadata: { nested: ['ok'] } });
  await store.put('p:a', 'one'); await store.put('other', 'excluded'); await store.put('p:雪', 'unicode');
  assert.equal(await store.get('p:a'), 'one');
  assert.deepEqual(await store.get('p:b', { type: 'json' }), { n: 2 });
  assert.deepEqual(await store.get('p:b', 'json'), { n: 2 });
  const first = await store.list({ prefix: 'p:', limit: 1 });
  assert.deepEqual(first.keys, [{ name: 'p:a' }]); assert.equal(first.list_complete, false);
  const second = await store.list({ prefix: 'p:', limit: 1, cursor: first.cursor });
  assert.deepEqual(second.keys, [{ name: 'p:b', metadata: { nested: ['ok'] } }]);
  const last = await store.list({ prefix: 'p:', cursor: second.cursor });
  assert.deepEqual(last.keys, [{ name: 'p:雪' }]); assert.equal(last.list_complete, true); assert.equal(last.cursor, '');
  await assert.rejects(store.list({ cursor: 'garbage' }), /cursor/);
  await assert.rejects(store.list({ prefix: 'wrong', cursor: first.cursor }), /cursor/);
  for (const limit of [0, -1, 1001, 1.5]) await assert.rejects(store.list({ limit }), /limit/);
  await store.delete('p:b'); assert.equal(await store.get('p:b'), null);
});

test('absolute and relative expiration, overwrite and immediate expired invisibility', async () => {
  const f = fixture();
  await f.store.put('abs', 'a', { expiration: f.now() + 10 });
  await f.store.put('ttl', 't', { expirationTtl: 5 });
  assert.equal((await f.store.list({ prefix: 'ttl' })).keys[0].expiration, f.now() + 5);
  f.advance(5); assert.equal(await f.store.get('ttl'), null);
  assert.deepEqual((await f.store.list()).keys.map(k => k.name), ['abs']);
  f.advance(5); assert.equal(await f.store.get('abs'), null);
  await f.store.put('abs', 'new'); assert.equal(await f.store.get('abs'), 'new');
  assert.equal((await f.store.list()).keys[0].expiration, undefined);
});

test('capacity sweeps expired rows and refuses new values while allowing updates', async () => {
  const f = fixture(4); f.advance(2 * 86400); // no migration markers after cutoff
  await f.store.put('a', 'a'); await f.store.put('b', 'b'); await f.store.put('c', 'c');
  await f.store.put('expired', 'e', { expiration: f.now() - 1 });
  await f.store.put('d', 'd');
  await assert.rejects(f.store.put('e', 'e'), /capacity/);
  await f.store.put('a', 'updated'); assert.equal(await f.store.get('a'), 'updated');
  assert.equal(f.db.prepare('SELECT count(*) AS n FROM oauth_kv').get()!.n, 4);
});

test('default and maximum page size is 1000', async () => {
  const f = fixture(); f.advance(2 * 86400);
  for (let i = 0; i < 1001; i++) await f.store.put(String(i).padStart(4, '0'), 'x');
  const page = await f.store.list(); assert.equal(page.keys.length, 1000); assert.equal(page.list_complete, false);
  assert.equal((await f.store.list({ cursor: page.cursor })).keys.length, 1);
});

test('legacy read once, own expiration copied, missing and expired records never reread', async () => {
  const f = fixture(100, { grant: JSON.stringify({ expiresAt: OAUTH_MIGRATION_CUTOFF + 100 }), text: '1',
    expired: JSON.stringify({ expiresAt: 1 }), client: JSON.stringify({ registrationExpiresAt: OAUTH_MIGRATION_CUTOFF + 500 }) });
  for (const key of ['grant', 'text', 'expired', 'client', 'absent']) { await f.store.get(key); await f.store.get(key); }
  assert.deepEqual(f.reads, ['grant', 'text', 'expired', 'client', 'absent']);
  assert.equal(await f.store.get('expired'), null);
  const keys = (await f.store.list()).keys;
  assert.equal(keys.find(k => k.name === 'grant')!.expiration, OAUTH_MIGRATION_CUTOFF + 100);
  assert.equal(keys.find(k => k.name === 'client')!.expiration, OAUTH_MIGRATION_CUTOFF + 500);
  assert.equal(keys.find(k => k.name === 'text')!.expiration, f.now() + 30 * 86400);
  f.store.sweep(); await f.store.get('expired'); assert.equal(f.reads.filter(k => k === 'expired').length, 1);
  f.advance(2 * 86400); await f.store.get('new'); assert(!f.reads.includes('new'));
});

test('tombstones block resurrection and grant cleanup removes SQL tokens and unread legacy tokens', async () => {
  const grant = 'grant:s:g', prefix = 'token:s:g:';
  const f = fixture(100, { [grant]: '{}', [prefix + 'unread']: '{}' });
  await f.store.get(grant); await f.store.put(prefix + 'a', '{}'); await f.store.put('token:s:other:a', '{}');
  await deleteProviderGrant(f.store, 'g', 's');
  assert.equal(await f.store.get(grant), null); assert.equal(await f.store.get(prefix + 'unread'), null);
  assert.deepEqual((await f.store.list({ prefix })).keys, []); assert.equal(await f.store.get('token:s:other:a'), '{}');
  await f.store.delete('never-read'); assert.equal(await f.store.get('never-read'), null);
  assert.deepEqual(f.reads, [grant]);
});

test('concurrent legacy reads cannot undo a put or delete', async () => {
  for (const mutation of ['put', 'delete']) {
    const f = fixture(); let resolve!: (value: string) => void;
    const store = oauthStore(f.sql, { get: () => new Promise(r => { resolve = r; }) } as unknown as KVNamespace, f.now);
    const pending = store.get('key');
    if (mutation === 'put') await store.put('key', 'new'); else await store.delete('key');
    resolve('old'); assert.equal(await pending, mutation === 'put' ? 'new' : null);
  }
});

test('alarm sweeps at most 256 expired rows, reschedules and eventually stops', async () => {
  const f = fixture(); f.advance(2 * 86400);
  for (let i = 0; i < 300; i++) await f.store.put(String(i), 'x', { expiration: f.now() - 1 });
  assert.equal(f.store.sweep(), true);
  assert.equal(f.db.prepare('SELECT count(*) AS n FROM oauth_kv').get()!.n, 44);
  assert.equal(f.store.sweep(), false);
});

test('unsupported methods and read/value types fail explicitly', async () => {
  const { store } = fixture();
  assert.throws(() => store.getWithMetadata('a'), /Unsupported OAuth storage operation/);
  assert.throws(() => (store as any).unknown(), /Unsupported OAuth storage operation/);
  await assert.rejects(store.get('a', { type: 'stream' } as any), /Unsupported OAuth storage operation/);
  await assert.rejects(store.put('a', new ArrayBuffer(1) as any), /Unsupported OAuth storage operation/);
});

test('simultaneous legacy misses share one read and persisted markers survive recreation', async () => {
  const f = fixture(); let complete!: (value: string) => void; let reads = 0;
  const store = oauthStore(f.sql, { get() { reads++; return new Promise(r => { complete = r; }); } } as unknown as KVNamespace, f.now);
  const a = store.get('key'), b = store.get('key');
  assert.equal(reads, 1); complete('value'); assert.deepEqual(await Promise.all([a, b]), ['value', 'value']);
  await store.delete('key');
  const restored = oauthStore(f.sql, f.legacy, f.now);
  assert.equal(await restored.get('key'), null); assert.deepEqual(f.reads, []);
});

test('migration markers count toward capacity and no legacy read occurs at cutoff', async () => {
  const f = fixture(2, { legacy: 'old' });
  await f.store.put('a', 'value');
  await f.store.delete('unread');
  await assert.rejects(f.store.put('b', 'new'), /capacity/);
  await f.store.put('a', 'updated');
  f.advance(86400);
  assert.equal(await f.store.get('legacy'), null); assert.deepEqual(f.reads, []);
  await f.store.put('b', 'new'); // admission reclaims migration markers after cutoff
  assert.equal(await f.store.get('b'), 'new');
});

function assertCount(f: ReturnType<typeof fixture>, expected?: number) {
  const actual = Number(f.db.prepare('SELECT (SELECT count(*) FROM oauth_kv) + (SELECT count(*) FROM oauth_migrated) AS n').get()!.n);
  assert.equal(f.db.prepare('SELECT rows FROM oauth_meta WHERE id = 1').get()!.rows, actual);
  if (expected !== undefined) assert.equal(actual, expected);
}

test('counter tracks inserts, overwrites, deletions, prefix deletions, expiry and marker cleanup', async () => {
  const f = fixture();
  assertCount(f, 0);
  await f.store.put('a', 'one'); assertCount(f, 1);
  await f.store.put('a', 'two'); assertCount(f, 1);
  await f.store.put('p:one', '1'); await f.store.put('p:two', '2'); assertCount(f, 3);
  await f.store.delete('a'); assertCount(f, 2);
  await f.store.delete('absent'); assertCount(f, 3);
  await f.store.delete('absent'); assertCount(f, 3);
  await f.store.deletePrefix('p:'); assertCount(f, 2);
  await f.store.put('expired', 'x', { expirationTtl: 1 }); assertCount(f, 3);
  f.advance(1); f.store.sweep(); assertCount(f, 2);
  f.advance(86400); f.store.sweep(); assertCount(f, 0);
  await f.store.delete('absent'); await f.store.deletePrefix('p:'); assertCount(f, 0);
});

test('meta initializes once from existing rows and survives store recreation', async () => {
  const f = fixture();
  await f.store.put('a', 'x'); await f.store.delete('absent');
  f.db.exec('DROP TABLE oauth_meta');
  oauthStore(f.sql, f.legacy, f.now); assertCount(f, 2);
  const statements: string[] = [];
  const sql = { exec(query: string, ...args: any[]) { statements.push(query); return f.sql.exec(query, ...args); } } as SqlStorage;
  const restored = oauthStore(sql, f.legacy, f.now);
  await restored.put('b', 'x'); await restored.delete('a'); await restored.get('missing');
  assertCount(f, 2);
  assert(!statements.some(query => /count\(\*\)/i.test(query)), 'steady-state operations never recount tables');
});

test('misses write no SQL rows, expire in ten minutes, and evict beyond 1000 entries', async () => {
  const f = fixture();
  const before = f.db.prepare('SELECT total_changes() AS n').get()!.n;
  for (let i = 0; i < 1001; i++) assert.equal(await f.store.get(`missing:${i}`), null);
  assert.equal(f.db.prepare('SELECT total_changes() AS n').get()!.n, before);
  assertCount(f, 0);
  await f.store.get('missing:1000'); assert.equal(f.reads.length, 1001);
  await f.store.get('missing:0'); assert.equal(f.reads.length, 1002);
  f.advance(600); await f.store.get('missing:1000'); assert.equal(f.reads.length, 1003);
});

test('fresh puts and overwrites create no markers and fill the full value capacity', async () => {
  const f = fixture(3);
  for (const key of ['a', 'b', 'c']) { await f.store.put(key, 'x'); await f.store.put(key, 'y'); }
  assert.equal(f.db.prepare('SELECT count(*) AS n FROM oauth_migrated').get()!.n, 0);
  assert.equal(f.db.prepare('SELECT sum(legacy) AS n FROM oauth_kv').get()!.n, 0);
  assertCount(f, 3);
  await assert.rejects(f.store.put('d', 'x'), /capacity/);
  await f.store.delete('a'); assertCount(f, 2);
});

test('prefix ranges include the exact key and ASCII suffixes but exclude adjacent prefixes', async () => {
  const f = fixture();
  const keys = ['token:s:g', 'token:s:g:', 'token:s:g:!', 'token:s:g:~', 'token:s:g0:a', 'token:s:h:a'];
  for (const key of keys) await f.store.put(key, 'x');
  assert.deepEqual((await f.store.list({ prefix: 'token:s:g:' })).keys.map(k => k.name), keys.slice(1, 4));
  await f.store.deletePrefix('token:s:g:');
  assert.deepEqual((await f.store.list()).keys.map(k => k.name), [keys[0], ...keys.slice(4)]);
  assertCount(f, 4);
});

test('legacy flag survives overwrite; delete, prefix delete and sweep prevent resurrection after recreation', async () => {
  for (const operation of ['delete', 'prefix', 'generic-prefix', 'sweep', 'admission']) {
    const key = operation === 'generic-prefix' ? 'client:old' : 'token:s:g:old';
    const f = fixture(3, { [key]: 'old' });
    assert.equal(await f.store.get(key), 'old');
    await f.store.put(key, 'new', { expirationTtl: 1 });
    assert.equal(f.db.prepare('SELECT legacy FROM oauth_kv WHERE key = ?').get(key)!.legacy, 1);
    if (operation === 'delete') await f.store.delete(key);
    else if (operation.includes('prefix')) await f.store.deletePrefix(operation === 'prefix' ? 'token:s:g:' : 'client:');
    else {
      f.advance(1);
      if (operation === 'sweep') f.store.sweep();
      else {
        await f.store.put('a', 'x'); await f.store.put('b', 'x');
        await assert.rejects(f.store.put('c', 'x'), /capacity/);
      }
    }
    assertCount(f);
    const restored = oauthStore(f.sql, f.legacy, f.now);
    assert.equal(await restored.get(key), null, operation);
    assert.deepEqual(f.reads, [key], operation);
  }
});

test('a put followed by deletion also cancels an in-flight legacy read without fresh-key markers', async () => {
  for (const prefix of [false, true]) {
    const f = fixture(); let complete!: (value: string) => void;
    const store = oauthStore(f.sql, { get: () => new Promise(r => { complete = r; }) } as unknown as KVNamespace, f.now);
    const pending = store.get('token:s:g:id');
    await store.put('token:s:g:id', 'new');
    if (prefix) await store.deletePrefix('token:s:g:'); else await store.delete('token:s:g:id');
    complete('old'); assert.equal(await pending, null);
    assertCount(f, prefix ? 1 : 0);
  }
});


test('bounded sweeps replace legacy values with counted markers even at capacity', async () => {
  const legacy = Object.fromEntries(Array.from({ length: 300 }, (_, i) => [`old:${i}`, '{"expiresAt":1}']));
  const f = fixture(300, legacy);
  for (const key of Object.keys(legacy)) await f.store.get(key);
  assertCount(f, 300);
  assert.equal(f.store.sweep(), true);
  assert.equal(f.db.prepare('SELECT count(*) AS n FROM oauth_kv').get()!.n, 44);
  assert.equal(f.db.prepare('SELECT count(*) AS n FROM oauth_migrated').get()!.n, 256);
  assertCount(f, 300);
  f.advance(86400);
  assert.equal(f.store.sweep(), false); assertCount(f, 0);
  assert.equal(f.store.sweep(), false);
});

test('unsafe legacy expirations remain rejected without inserting SQL rows', async () => {
  const f = fixture(100, { bad: '{"expiresAt":1e100}' });
  await assert.rejects(f.store.get('bad'), /Invalid OAuth expiration/);
  assertCount(f, 0);
});
