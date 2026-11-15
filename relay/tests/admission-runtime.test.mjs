// SPDX-License-Identifier: Apache-2.0
import assert from 'node:assert/strict';
import test from 'node:test';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import { join, resolve } from 'node:path';

const toolchain = process.env.WENLAN_RELAY_TOOLCHAIN;
if (!toolchain) throw new Error('Set WENLAN_RELAY_TOOLCHAIN to an installed Wrangler project directory');
const requireTool = createRequire(join(resolve(toolchain), 'package.json'));
const { Miniflare } = requireTool('miniflare');
const { build } = requireTool('esbuild');
const origin = 'https://wenlan-relay.example';
const fixturePath = '/__admission-fixture';
const dayMs = 86_400_000;
// Freeze bucket boundaries and keep automatic alarms outside this bounded test.
const fixtureTime = Date.now() + 10 * 60_000;

// This wrapper is bundled only for this test. Its fixture route holds the
// production Durable Object state and seeds the production rate table directly;
// the production entry does not import it. Never deploy this fixture bundle.
const fixtureSource = `
import { RelayAuthority } from './src/worker.ts';
export { default } from './src/worker.ts';
Date.now = () => ${fixtureTime};

export class AdmissionFixture extends RelayAuthority {
  fixtureState;
  constructor(state, env) {
    super(state, env);
    this.fixtureState = state;
  }
  async fetch(request) {
    if (new URL(request.url).pathname !== '${fixturePath}') return super.fetch(request);
    const body = await request.json();
    if (body.operation === 'seed') {
      const buckets = Array.isArray(body.buckets) ? body.buckets : [];
      const fillerCount = Number(body.fillerCount ?? 0);
      const expiredFillers = Number(body.expiredFillers ?? 0);
      if (!Number.isSafeInteger(fillerCount) || fillerCount < 0 || fillerCount > 4096
        || !Number.isSafeInteger(expiredFillers) || expiredFillers < 0 || expiredFillers > fillerCount
        || buckets.length > 4 || fillerCount + buckets.length > 4096
        || buckets.some(bucket => !bucket || typeof bucket.key !== 'string'
          || !Number.isSafeInteger(bucket.count) || bucket.count < 0
          || !Number.isSafeInteger(bucket.expires))) return new Response(null, { status: 400 });
      this.fixtureState.storage.transactionSync(() => {
        this.fixtureState.storage.sql.exec('DELETE FROM rate_window');
        const now = Date.now();
        for (let i = 0; i < fillerCount; i++) this.fixtureState.storage.sql.exec(
          'INSERT INTO rate_window (key,count,expires) VALUES (?,1,?)',
          (i < expiredFillers ? 'expired:' : 'future:') + String(i).padStart(4, '0'),
          i < expiredFillers ? now - 1 : now + 3_600_000);
        for (const bucket of buckets) this.fixtureState.storage.sql.exec(
          'INSERT INTO rate_window (key,count,expires) VALUES (?,?,?)',
          bucket.key, bucket.count, bucket.expires);
      });
      return Response.json({ seeded: true });
    }
    if (body.operation === 'inspect') {
      const keys = (Array.isArray(body.keys) ? body.keys : [])
        .filter(key => typeof key === 'string').slice(0, 16);
      const summary = this.fixtureState.storage.sql.exec(
        'SELECT COUNT(*) AS rowCount, COUNT(CASE WHEN expires <= ? THEN 1 END) AS expiredCount FROM rate_window',
        Date.now()).one();
      const buckets = {};
      for (const key of keys) {
        const row = this.fixtureState.storage.sql.exec(
          'SELECT count, expires FROM rate_window WHERE key = ?', key).toArray()[0];
        if (row) buckets[key] = row;
      }
      return Response.json({ rows: summary.rowCount, expired: summary.expiredCount, buckets });
    }
    return new Response(null, { status: 400 });
  }
}
`;

async function hashPeer(ip) {
  const digest = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(ip.slice(0, 128)));
  return Array.from(new Uint8Array(digest), byte => byte.toString(16).padStart(2, '0')).join('');
}

async function bucketKeys(path, ip, timestamp = fixtureTime) {
  const peer = await hashPeer(ip);
  const minute = Math.floor(timestamp / 60_000);
  const hour = Math.floor(timestamp / 3_600_000);
  const day = Math.floor(timestamp / dayMs);
  return [
    `global:${minute}`,
    `peer:${peer}:${minute}`,
    `${path}:${peer}:${hour}`,
    `path-global:${path}:day:${day}`,
  ];
}

test('production admission limits and bounded rate table run in Miniflare', { timeout: 120_000 }, async t => {
  const production = await build({
    entryPoints: [fileURLToPath(new URL('../src/worker.ts', import.meta.url))],
    bundle: true, write: false, format: 'esm', platform: 'browser', target: 'es2022',
    external: ['cloudflare:workers'], loader: { '.png': 'binary' },
  });
  assert(!production.outputFiles[0].text.includes(fixturePath));
  assert(!production.outputFiles[0].text.includes('AdmissionFixture'));
  const bundle = await build({
    stdin: { contents: fixtureSource, resolveDir: fileURLToPath(new URL('../', import.meta.url)), sourcefile: 'admission-fixture.ts' },
    bundle: true, write: false, format: 'esm', platform: 'browser', target: 'es2022',
    external: ['cloudflare:workers'], loader: { '.png': 'binary' },
  });
  const runtime = new Miniflare({
    modules: true, script: bundle.outputFiles[0].text,
    compatibilityDate: '2026-09-08', host: '127.0.0.1', port: 0,
    bindings: { PUBLIC_ORIGIN: origin }, kvNamespaces: ['OAUTH_KV'],
    durableObjects: { AUTHORITY: { className: 'AdmissionFixture', useSQLite: true } },
    outboundService: () => new Response('Synthetic outbound disabled', { status: 503 }),
  });
  const request = (path, init) => runtime.dispatchFetch(`${origin}${path}`, { ...init, credentials: 'omit' });
  const attempt = async (path, ip) => {
    const response = await request(path, {
      method: 'POST', headers: { 'content-type': 'application/json', 'cf-connecting-ip': ip }, body: '{}',
    });
    const body = await response.text();
    return { status: response.status, retryAfter: response.headers.get('retry-after'), body };
  };
  const get = async (path, ip) => {
    const response = await request(path, { method: 'GET', headers: { 'cf-connecting-ip': ip } });
    const body = await response.arrayBuffer();
    return { status: response.status, contentType: response.headers.get('content-type'), body };
  };
  const namespace = await runtime.getDurableObjectNamespace('AUTHORITY');
  const authority = namespace.get(namespace.idFromName('wenlan-v1'));
  const fixture = async (operation, values = {}) => {
    const response = await authority.fetch(`${origin}${fixturePath}`, {
      method: 'POST', headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ operation, ...values }),
    });
    assert.equal(response.status, 200, `${operation}: ${await response.clone().text()}`);
    return response.json();
  };

  try {
    await t.test('cross-peer device attempts share a daily cap while another GET still works', async () => {
      const peers = Array.from({ length: 40 }, (_, index) => `198.51.100.${index + 1}`);
      const attempts = await Promise.all(peers.flatMap(ip => Array.from({ length: 3 }, () => attempt('/devices', ip))));
      assert.equal(attempts.filter(result => result.status === 400).length, 100);
      const rejected = attempts.filter(result => result.status === 429);
      assert.equal(rejected.length, 20);
      assert(rejected.every(result => Number(result.retryAfter) > 0 && Number(result.retryAfter) <= 86_400));
      const dayKey = (await bucketKeys('/devices', peers[0]))[3];
      const state = await fixture('inspect', { keys: [dayKey] });
      assert.equal(state.buckets[dayKey].count, 100);

      const css = await get('/pairing.css', '198.51.100.250');
      assert.equal(css.status, 200);
      assert.match(css.contentType, /^text\/css/);
      assert(css.body.byteLength > 0);
      const getDevices = await get('/devices', '198.51.100.251');
      assert.equal(getDevices.status, 404, 'GET /devices must not consume the POST daily budget');
      const afterGet = await fixture('inspect', { keys: [dayKey] });
      assert.equal(afterGet.buckets[dayKey].count, 100);
    });

    await t.test('cross-peer DCR attempts share its daily cap', async () => {
      const peers = Array.from({ length: 25 }, (_, index) => `203.0.113.${index + 1}`);
      // Exercise the same 250 attempts across peers without opening 250 local
      // TCP connections at once (Miniflare can reset that burst on macOS).
      // Last-slot concurrency is exercised independently below.
      const attempts = (await Promise.all(peers.map(async ip => {
        const results = [];
        for (let n = 0; n < 10; n++) results.push(await attempt('/oauth/register', ip));
        return results;
      }))).flat();
      assert.equal(attempts.filter(result => result.status === 400).length, 250);
      const next = await attempt('/oauth/register', '203.0.113.250');
      assert.equal(next.status, 429);
      assert.equal(next.retryAfter, String(Math.max(1,
        Math.ceil(((Math.floor(fixtureTime / dayMs) + 1) * dayMs - fixtureTime) / 1000))));
      const dayKey = (await bucketKeys('/oauth/register', peers[0]))[3];
      const state = await fixture('inspect', { keys: [dayKey] });
      assert.equal(state.buckets[dayKey].count, 250);
    });

    await t.test('a full table rejects new buckets without growth but admits existing buckets', async () => {
      await fixture('seed', { fillerCount: 4096 });
      const newPeer = '192.0.2.1';
      const rejected = await attempt('/devices', newPeer);
      assert.equal(rejected.status, 429);
      assert.equal(rejected.retryAfter, '60');
      const rejectedKeys = await bucketKeys('/devices', newPeer);
      const rejectedState = await fixture('inspect', { keys: rejectedKeys });
      assert.equal(rejectedState.rows, 4096);
      assert.deepEqual(rejectedState.buckets, {});

      const existingPeer = '192.0.2.2';
      const existingKeys = await bucketKeys('/devices', existingPeer);
      const now = fixtureTime;
      await fixture('seed', {
        fillerCount: 4092,
        buckets: existingKeys.map(key => ({ key, count: 1, expires: now + 3_600_000 })),
      });
      const admitted = await attempt('/devices', existingPeer);
      assert.equal(admitted.status, 400, admitted.body);
      const existingState = await fixture('inspect', { keys: existingKeys });
      assert.equal(existingState.rows, 4096);
      for (const key of existingKeys) assert.equal(existingState.buckets[key].count, 2);

      const otherPeer = '192.0.2.3';
      const otherKeys = await bucketKeys('/devices', otherPeer);
      const capRejected = await attempt('/devices', otherPeer);
      assert.equal(capRejected.status, 429);
      assert.equal(capRejected.retryAfter, '60');
      const afterOther = await fixture('inspect', { keys: [...otherKeys, existingKeys[0], existingKeys[3]] });
      assert.equal(afterOther.rows, 4096);
      assert.equal(afterOther.buckets[existingKeys[0]].count, 2);
      assert.equal(afterOther.buckets[existingKeys[3]].count, 2);
      assert.equal(afterOther.buckets[otherKeys[1]], undefined);
      assert.equal(afterOther.buckets[otherKeys[2]], undefined);
    });

    await t.test('concurrent new peers cannot overfill the last available slots', async () => {
      await fixture('seed', { fillerCount: 4092 });
      const attempts = await Promise.all(Array.from({ length: 12 }, (_, index) =>
        attempt('/devices', `192.0.2.${index + 10}`)));
      assert.equal(attempts.filter(result => result.status === 400).length, 1);
      assert.equal(attempts.filter(result => result.status === 429).length, 11);
      assert.equal((await fixture('inspect')).rows, 4096);
    });

    await t.test('recounts relevant keys removed by reclamation before admitting', async () => {
      const peer = '198.18.0.2';
      const keys = await bucketKeys('/devices', peer);
      await fixture('seed', {
        fillerCount: 4093,
        buckets: keys.slice(0, 3).map((key, index) => ({
          key, count: 1, expires: index === 0 ? fixtureTime - 1 : fixtureTime + 3_600_000,
        })),
      });
      const rejected = await attempt('/devices', peer);
      assert.equal(rejected.status, 429);
      assert.equal(rejected.retryAfter, '60');
      const state = await fixture('inspect', { keys });
      assert.equal(state.rows, 4095);
      assert.equal(state.buckets[keys[0]], undefined);
      assert.equal(state.buckets[keys[3]], undefined);
      assert.equal(state.buckets[keys[1]].count, 1);
      assert.equal(state.buckets[keys[2]].count, 1);
    });

    await t.test('reclaims at most 256 expired rows before admitting missing buckets', async () => {
      await fixture('seed', { fillerCount: 4096, expiredFillers: 300 });
      const peer = '198.18.0.1';
      const keys = await bucketKeys('/devices', peer);
      const admitted = await attempt('/devices', peer);
      assert.equal(admitted.status, 400, admitted.body);
      const state = await fixture('inspect', { keys });
      assert.equal(state.rows, 3844);
      assert.equal(state.expired, 44, 'reclamation must stop after one 256-row batch');
      for (const key of keys) assert.equal(state.buckets[key].count, 1);
    });
  } finally {
    await runtime.dispose();
  }
});
