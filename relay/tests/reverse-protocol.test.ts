// SPDX-License-Identifier: Apache-2.0
import assert from 'node:assert/strict';
import test from 'node:test';
import {
  decodeBody,
  decodeFrame,
  encodeBody,
  encodeFrame,
  limits,
  type ReverseFrame,
} from '../src/reverse-protocol.ts';

const token = 't'.repeat(32);
const emptyHeaders = {};

function request(overrides: Partial<Extract<ReverseFrame, { type: 'request' }>> = {}) {
  return {
    v: 1 as const,
    type: 'request' as const,
    id: 'request-1',
    path: '/mcp' as const,
    method: 'POST' as const,
    headers: { ...emptyHeaders },
    body: encodeBody(new TextEncoder().encode('{"jsonrpc":"2.0"}')),
    ...overrides,
  };
}

function response(overrides: Partial<Extract<ReverseFrame, { type: 'response' }>> = {}) {
  return {
    v: 1 as const,
    type: 'response' as const,
    id: 'response-1',
    status: 200,
    headers: { 'content-type': 'application/json' },
    ...overrides,
  };
}

function chunk(overrides: Partial<Extract<ReverseFrame, { type: 'chunk' }>> = {}) {
  return {
    v: 1 as const,
    type: 'chunk' as const,
    id: 'chunk-1',
    seq: 0,
    body: encodeBody(new Uint8Array([0, 1, 2, 255])),
    ...overrides,
  };
}

function assertInvalid(value: unknown) {
  assert.throws(() => decodeFrame(value as string), error =>
    error instanceof Error && error.message === 'Invalid reverse frame');
}

test('body helpers round-trip empty, bounded, and multibyte bytes', () => {
  const multibyte = new TextEncoder().encode('繁體中文 and emoji: 🧠');
  assert.deepEqual(decodeBody(encodeBody(multibyte)), multibyte);
  assert.deepEqual(decodeBody(encodeBody(new Uint8Array())), new Uint8Array());

  const max = new Uint8Array(limits.maxRequestBodyBytes);
  assert.equal(decodeBody(encodeBody(max)).byteLength, limits.maxRequestBodyBytes);
  assert.throws(() => encodeBody(new Uint8Array(limits.maxRequestBodyBytes + 1)), /Invalid reverse frame/);
});

test('every frame variant round-trips', () => {
  const frames: ReverseFrame[] = [
    request({ headers: { authorization: `Bearer ${token}` } }),
    request({ id: 'info', path: '/connector-info', method: 'GET', headers: {}, body: '' }),
    request({ id: 'get', method: 'GET', headers: {}, body: '' }),
    request({ id: 'delete', method: 'DELETE', headers: {}, body: '' }),
    response({ headers: { 'content-type': 'text/event-stream', 'mcp-session-id': 'session-1' } }),
    response({ id: 'no-content', status: 204, headers: {} }),
    chunk(),
    { v: 1, type: 'end', id: 'end-1' },
    { v: 1, type: 'cancel', id: 'cancel-1' },
    { v: 1, type: 'credit', id: 'credit-1', seq: Number.MAX_SAFE_INTEGER },
  ];

  for (const frame of frames) {
    assert.deepEqual(decodeFrame(encodeFrame(frame)), frame);
  }
});

test('request body and header boundaries are enforced', () => {
  const maxRequestBody = encodeBody(new Uint8Array(limits.maxRequestBodyBytes));
  assert.deepEqual(decodeFrame(encodeFrame(request({ body: maxRequestBody }))), request({ body: maxRequestBody }));

  for (const method of ['GET', 'DELETE'] as const) {
    const decoded = decodeFrame(encodeFrame(request({ method, body: '' })));
    assert.equal((decoded as Extract<ReverseFrame, { type: 'request' }>).method, method);
    assertInvalid(JSON.stringify(request({ method, body: encodeBody(new Uint8Array([1])) })));
  }

  const maxHeader = 'x'.repeat(limits.maxHeaderValueBytes);
  assert.deepEqual(
    decodeFrame(encodeFrame(request({ headers: { accept: maxHeader }, body: '' }))),
    request({ headers: { accept: maxHeader }, body: '' }),
  );
  assertInvalid(JSON.stringify(request({ headers: { accept: `${maxHeader}x` }, body: '' })));
  assertInvalid(JSON.stringify(request({ headers: { accept: 'line\nfeed' }, body: '' })));
  assertInvalid(JSON.stringify(request({ headers: { accept: 'carriage\rreturn' }, body: '' })));
  assertInvalid(JSON.stringify(request({ headers: { accept: 'nul\u0000byte' }, body: '' })));
  assertInvalid(JSON.stringify(request({ headers: { accept: 'backspace\u0008' }, body: '' })));
  assertInvalid(JSON.stringify(request({ headers: { accept: 'delete\u007f' }, body: '' })));
  assertInvalid(JSON.stringify(request({ headers: { accept: 'non-ascii: é' }, body: '' })));
  assert.deepEqual(
    decodeFrame(encodeFrame(request({ headers: { accept: '\t printable ~' }, body: '' }))),
    request({ headers: { accept: '\t printable ~' }, body: '' }),
  );

  assert.deepEqual(
    decodeFrame(encodeFrame(request({ headers: { authorization: `Bearer ${'a'.repeat(128)}` }, body: '' }))),
    request({ headers: { authorization: `Bearer ${'a'.repeat(128)}` }, body: '' }),
  );
  for (const authorization of [
    'Bearer short',
    `Bearer ${'a'.repeat(129)}`,
    `Bearer ${'a'.repeat(31)}!`,
    `bearer ${token}`,
    `Bearer ${token} `,
  ]) {
    assertInvalid(JSON.stringify(request({ headers: { authorization }, body: '' })));
  }
});

test('chunk bodies and sequence boundaries are enforced', () => {
  const maxChunkBody = encodeBody(new Uint8Array(limits.maxChunkBodyBytes));
  assert.deepEqual(
    decodeFrame(encodeFrame(chunk({ body: maxChunkBody }))),
    chunk({ body: maxChunkBody }),
  );
  assertInvalid(JSON.stringify(chunk({ body: '' })));
  assertInvalid(JSON.stringify(chunk({ body: encodeBody(new Uint8Array(limits.maxChunkBodyBytes + 1)) })));

  for (const seq of [0, Number.MAX_SAFE_INTEGER]) {
    assert.equal((decodeFrame(encodeFrame(chunk({ seq }))) as Extract<ReverseFrame, { type: 'chunk' }>).seq, seq);
    assert.equal((decodeFrame(encodeFrame({ v: 1, type: 'credit', id: 'credit', seq })) as Extract<ReverseFrame, { type: 'credit' }>).seq, seq);
  }
  for (const seq of [-1, 1.5, Number.MAX_SAFE_INTEGER + 1, Infinity, NaN]) {
    assertInvalid(JSON.stringify(chunk({ seq })));
    assertInvalid(JSON.stringify({ v: 1, type: 'credit', id: 'credit', seq }));
  }
});

test('wire messages use exact keys and reject invalid discriminators', () => {
  const valid = request({ body: '' });
  assert.deepEqual(decodeFrame(JSON.stringify(valid)), valid);

  for (const invalid of [
    { ...valid, v: 2 },
    { ...valid, extra: true },
    { ...valid, type: 'error' },
    { ...valid, id: '' },
    { ...valid, id: 'x'.repeat(65) },
    { ...valid, id: 'bad id' },
    { ...valid, id: 'https://backend.invalid' },
    { ...valid, path: 'https://backend.invalid/mcp' },
    { ...valid, path: '/mcp?url=https://backend.invalid' },
    { ...valid, method: 'PUT' },
    { ...valid, headers: { Cookie: 'private=1' } },
    { ...valid, headers: { cookie: 'private=1' } },
    { ...valid, headers: { host: 'backend.invalid' } },
    { ...valid, headers: { authorization: `Bearer ${token}`, cookie: 'private=1' } },
  ]) {
    assertInvalid(JSON.stringify(invalid));
  }

  for (const invalid of [
    { ...response(), extra: true },
    { ...response(), status: 199 },
    { ...response(), status: 600 },
    { ...response(), status: 200.5 },
    { ...response(), headers: { authorization: `Bearer ${token}` } },
    { ...response(), headers: { 'Content-Type': 'application/json' } },
    { ...response(), headers: { 'set-cookie': 'private=1' } },
    { ...response(), headers: { 'content-type': 'application/json\u000b' } },
    { ...response(), headers: { 'content-type': 'application/json é' } },
  ]) {
    assertInvalid(JSON.stringify(invalid));
  }

  for (const type of ['end', 'cancel'] as const) {
    assertInvalid(JSON.stringify({ v: 1, type, id: 'id', extra: false }));
  }
});

test('a backend cannot forge the internal upstream marker', () => {
  const forged = response({ headers: { 'content-type': 'application/json', 'x-wenlan-upstream-unavailable': '1' } });
  assertInvalid(JSON.stringify(forged));
  assert.throws(() => encodeFrame(forged as ReverseFrame), /Invalid reverse frame/);
  assert.deepEqual(decodeFrame(encodeFrame(response())), response());
});

test('invalid base64 is rejected instead of normalized', () => {
  for (const body of [' ', 'YQ', 'YQ==\n', 'YQ== ', 'YQ-=', 'YQ_=', '!!!!', 'YWI===']) {
    assertInvalid(JSON.stringify(request({ body })));
    assert.throws(() => decodeBody(body), /Invalid reverse frame/);
  }
});

test('wire byte limit is checked before JSON parsing', () => {
  const oversized = '{'.padEnd(limits.maxWireBytes + 1, 'x');
  assertInvalid(oversized);
  assertInvalid(`{"v":1,"type":"request","id":"id","path":"/mcp","method":"POST","headers":{},"body":"${'A'.repeat(limits.maxWireBytes)}"}`);
});

test('malformed inputs always use the generic error without echoing input', () => {
  const secret = 'PRIVATE_WIRE_PAYLOAD_123';
  for (const input of [
    secret,
    `{"v":1,"type":"request","id":"${secret}!","path":"/mcp","method":"POST","headers":{},"body":""}`,
    `{"v":1,"type":"request","id":"id","path":"https://${secret}","method":"POST","headers":{},"body":""}`,
  ]) {
    try {
      decodeFrame(input);
      assert.fail('expected invalid frame');
    } catch (error) {
      assert(error instanceof Error);
      assert.equal(error.message, 'Invalid reverse frame');
      assert.equal(error.message.includes(secret), false);
      assert.equal(String(error).includes(secret), false);
    }
  }
});

test('encodeFrame validates runtime objects and wire bounds', () => {
  const invalidFrames: unknown[] = [
    null,
    [],
    'frame',
    { v: 1, type: 'request', id: 'id', path: '/mcp', method: 'POST', headers: {}, body: '', extra: 1 },
    { v: 1, type: 'request', id: 'id', path: '/mcp', method: 'POST', headers: {}, body: 'YQ' },
    { v: 1, type: 'chunk', id: 'id', seq: 0, body: '' },
    { v: 1, type: 'response', id: 'id', status: 200, headers: {}, body: '' },
  ];
  for (const frame of invalidFrames) {
    assert.throws(() => encodeFrame(frame as ReverseFrame), error =>
      error instanceof Error && error.message === 'Invalid reverse frame');
  }

  const tooLarge = request({
    headers: {
      accept: '"'.repeat(limits.maxHeaderValueBytes),
      'content-type': 'x'.repeat(limits.maxHeaderValueBytes),
      'mcp-session-id': 'x'.repeat(limits.maxHeaderValueBytes),
      'mcp-protocol-version': 'x'.repeat(limits.maxHeaderValueBytes),
      'last-event-id': 'x'.repeat(limits.maxHeaderValueBytes),
      authorization: `Bearer ${'x'.repeat(128)}`,
    },
    body: encodeBody(new Uint8Array(limits.maxRequestBodyBytes)),
  });
  assert.throws(() => encodeFrame(tooLarge), /Invalid reverse frame/);
});

test('request frames may carry x-wenlan-access, response frames may not', () => {
  const marked = request({ headers: { 'x-wenlan-access': 'write' } });
  assert.deepEqual(decodeFrame(encodeFrame(marked)), marked);
  assertInvalid(JSON.stringify(response({ headers: { 'x-wenlan-access': 'write' } })));
  // Other names stay refused, including near-misses.
  for (const name of ['x-wenlan-access-extra', 'x-wenlan-space', 'x-wenlan-acces']) {
    assertInvalid(JSON.stringify(request({ headers: { [name]: 'write' } })));
  }
});
