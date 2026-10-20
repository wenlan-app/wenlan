// SPDX-License-Identifier: Apache-2.0
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { decodeFrame, encodeFrame } from '../src/reverse-protocol.ts';

test('TypeScript consumes the same valid and invalid corpus as the Rust desktop codec', () => {
  const corpus = JSON.parse(readFileSync(new URL('./fixtures/reverse-protocol-corpus.json', import.meta.url), 'utf8'));
  assert.equal(corpus.valid.length, 15);
  assert.equal(corpus.invalid.length, 28);
  for (const value of corpus.valid) {
    const frame = decodeFrame(JSON.stringify(value));
    assert.deepEqual(frame, value);
    assert.deepEqual(JSON.parse(encodeFrame(frame)), value);
  }
  for (const value of corpus.invalid) {
    assert.throws(() => decodeFrame(JSON.stringify(value)), { message: 'Invalid reverse frame' });
  }
});
