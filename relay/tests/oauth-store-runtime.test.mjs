// SPDX-License-Identifier: Apache-2.0
import assert from 'node:assert/strict';
import test from 'node:test';
import { createRequire } from 'node:module';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

if (!process.env.WENLAN_RELAY_TOOLCHAIN) throw new Error('WENLAN_RELAY_TOOLCHAIN required');
const requireTool = createRequire(join(resolve(process.env.WENLAN_RELAY_TOOLCHAIN), 'package.json'));
const { Miniflare } = requireTool('miniflare');
const { build } = requireTool('esbuild');

test('OAuth indexed operations read few SQL rows with 5000 stored keys', async () => {
  const bundle = await build({ entryPoints: [fileURLToPath(new URL('./fixtures/oauth-cost-worker.ts', import.meta.url))],
    bundle: true, write: false, format: 'esm', platform: 'browser', target: 'es2022', external: ['cloudflare:workers'] });
  const runtime = new Miniflare({ modules: true, script: bundle.outputFiles[0].text,
    compatibilityDate: '2026-09-08', host: '127.0.0.1', port: 0,
    durableObjects: { STORE: { className: 'OAuthCostFixture', useSQLite: true } },
    outboundService: () => new Response(null, { status: 502 }) });
  try {
    const ns = await runtime.getDurableObjectNamespace('STORE');
    const response = await ns.get(ns.idFromName('cost')).fetch('https://fixture.example/');
    assert.equal(response.status, 200, await response.clone().text());
    const { count, actual, ...reads } = await response.json();
    assert.equal(count, 5001); assert.equal(count, actual);
    for (const [operation, n] of Object.entries(reads)) assert(n < 50, `${operation} read ${n} SQL rows`);
    console.log('OAuth SQL rowsRead with 5000 keys:', reads);
  } finally { await runtime.dispose(); }
});
