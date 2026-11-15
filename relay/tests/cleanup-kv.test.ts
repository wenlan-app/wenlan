// SPDX-License-Identifier: Apache-2.0
import assert from 'node:assert/strict';
import test from 'node:test';
import { deleteProviderGrant } from '../src/cleanup-kv.ts';

test('grant cleanup retains delete-only compatibility with a legacy KV-shaped test double', async () => {
  const calls: string[] = [];
  const kv = new Proxy({}, {
    get: (_target, property) => async (key?: string) => { calls.push(`${String(property)}:${key ?? ''}`); },
  }) as unknown as KVNamespace;
  await deleteProviderGrant(kv, 'g'.repeat(16), 'd'.repeat(64));
  assert.deepEqual(calls, [`delete:grant:${'d'.repeat(64)}:${'g'.repeat(16)}`]);
});

test('grant cleanup refuses ids that could address another key', async () => {
  const kv = { delete: async () => assert.fail('must not delete') } as unknown as KVNamespace;
  for (const [grantId, subject] of [['', 's'], ['g', ''], ['g:x', 's'], ['g', 's:x']]) {
    await assert.rejects(deleteProviderGrant(kv, grantId, subject), /Invalid OAuth grant/);
  }
});
