// SPDX-License-Identifier: Apache-2.0

const INVALID_MESSAGE = 'Invalid reverse frame';
const textEncoder = new TextEncoder();
const BASE64_RE = /^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/;
const ID_RE = /^[A-Za-z0-9_-]{1,64}$/;
const AUTHORIZATION_RE = /^Bearer [A-Za-z0-9_-]{32,128}$/;
const HEADER_VALUE_RE = /^[\t\x20-\x7e]*$/;
const REQUEST_HEADER_NAMES = new Set([
  'accept',
  'content-type',
  'mcp-session-id',
  'mcp-protocol-version',
  'last-event-id',
  'authorization',
  // Set by the relay for a write grant only; the desktop picks its MCP process by it.
  'x-wenlan-access',
]);
const RESPONSE_HEADER_NAMES = new Set(['content-type', 'mcp-session-id', 'mcp-protocol-version']);

export const limits = Object.freeze({
  maxWireBytes: 96 * 1024,
  maxRequestBodyBytes: 64 * 1024,
  maxChunkBodyBytes: 16 * 1024,
  maxIdBytes: 64,
  maxHeaderValueBytes: 2048,
});

type RequestMethod = 'GET' | 'POST' | 'DELETE';
type RequestPath = '/mcp' | '/connector-info';

export type RequestFrame = {
  v: 1;
  type: 'request';
  id: string;
  path: RequestPath;
  method: RequestMethod;
  headers: Record<string, string>;
  body: string;
};

export type ResponseFrame = {
  v: 1;
  type: 'response';
  id: string;
  status: number;
  headers: Record<string, string>;
};

export type ChunkFrame = {
  v: 1;
  type: 'chunk';
  id: string;
  seq: number;
  body: string;
};

export type EndFrame = {
  v: 1;
  type: 'end';
  id: string;
};

export type CancelFrame = {
  v: 1;
  type: 'cancel';
  id: string;
};

export type CreditFrame = {
  v: 1;
  type: 'credit';
  id: string;
  seq: number;
};

export type ReverseFrame =
  | RequestFrame
  | ResponseFrame
  | ChunkFrame
  | EndFrame
  | CancelFrame
  | CreditFrame;

function invalid(): never {
  throw new Error(INVALID_MESSAGE);
}

function isPlainRecord(value: unknown): value is Record<string, unknown> {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) return false;
  const prototype = Object.getPrototypeOf(value);
  return prototype === Object.prototype || prototype === null;
}

function exactRecord(value: unknown, keys: readonly string[]): Record<string, unknown> {
  if (!isPlainRecord(value)) invalid();
  const ownNames = Object.getOwnPropertyNames(value);
  if (ownNames.length !== keys.length || ownNames.some(name => !keys.includes(name))) invalid();
  if (Object.getOwnPropertySymbols(value).length !== 0) invalid();
  return value;
}

function exactString(value: unknown): string {
  if (typeof value !== 'string') invalid();
  return value;
}

function byteLength(value: string): number {
  return textEncoder.encode(value).byteLength;
}

function validateId(value: unknown): string {
  const id = exactString(value);
  if (!ID_RE.test(id) || byteLength(id) > limits.maxIdBytes) invalid();
  return id;
}

function validateSequence(value: unknown): number {
  if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < 0) invalid();
  return value;
}

function validateHeaders(value: unknown, allowedNames: ReadonlySet<string>, authorization: boolean): Record<string, string> {
  if (!isPlainRecord(value)) invalid();
  const ownNames = Object.getOwnPropertyNames(value);
  if (ownNames.some(name => !allowedNames.has(name)) || Object.getOwnPropertySymbols(value).length !== 0) invalid();
  const input = value;
  const headers: Record<string, string> = {};
  for (const name of ownNames) {
    if (!allowedNames.has(name)) invalid();
    const headerValue = exactString(input[name]);
    if (headerValue.length > limits.maxHeaderValueBytes || !HEADER_VALUE_RE.test(headerValue)) invalid();
    if (authorization && name === 'authorization' && !AUTHORIZATION_RE.test(headerValue)) invalid();
    headers[name] = headerValue;
  }
  return headers;
}

function bytesToBinary(bytes: Uint8Array): string {
  let binary = '';
  for (let offset = 0; offset < bytes.length; offset += 0x8000) {
    const end = Math.min(offset + 0x8000, bytes.length);
    let part = '';
    for (let index = offset; index < end; index += 1) part += String.fromCharCode(bytes[index]);
    binary += part;
  }
  return binary;
}

export function encodeBody(bytes: Uint8Array): string {
  try {
    if (!(bytes instanceof Uint8Array) || bytes.byteLength > limits.maxRequestBodyBytes) invalid();
    return btoa(bytesToBinary(bytes));
  } catch {
    invalid();
  }
}

function decodeCanonicalBase64(base64: string): Uint8Array {
  const maxEncodedBytes = Math.ceil(limits.maxRequestBodyBytes / 3) * 4;
  if (base64.length > maxEncodedBytes) invalid();
  if (!BASE64_RE.test(base64)) invalid();

  const binary = atob(base64);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) bytes[index] = binary.charCodeAt(index);
  if (encodeBody(bytes) !== base64) invalid();
  return bytes;
}

export function decodeBody(base64: string): Uint8Array {
  try {
    return decodeCanonicalBase64(exactString(base64));
  } catch {
    invalid();
  }
}

function validateRequest(value: unknown): RequestFrame {
  const input = exactRecord(value, ['v', 'type', 'id', 'path', 'method', 'headers', 'body']);
  if (input.v !== 1 || input.type !== 'request') invalid();
  const id = validateId(input.id);
  const path = exactString(input.path);
  if (path !== '/mcp' && path !== '/connector-info') invalid();
  const method = exactString(input.method);
  if (method !== 'GET' && method !== 'POST' && method !== 'DELETE') invalid();
  if (path === '/connector-info' && method !== 'GET') invalid();
  const headers = validateHeaders(input.headers, REQUEST_HEADER_NAMES, true);
  const body = exactString(input.body);
  if ((method === 'GET' || method === 'DELETE') && body !== '') invalid();
  decodeBody(body);
  return { v: 1, type: 'request', id, path, method, headers, body };
}

function validateResponse(value: unknown): ResponseFrame {
  const input = exactRecord(value, ['v', 'type', 'id', 'status', 'headers']);
  if (input.v !== 1 || input.type !== 'response') invalid();
  const id = validateId(input.id);
  if (typeof input.status !== 'number' || !Number.isInteger(input.status) || input.status < 200 || input.status > 599) invalid();
  const headers = validateHeaders(input.headers, RESPONSE_HEADER_NAMES, false);
  return { v: 1, type: 'response', id, status: input.status, headers };
}

function validateChunk(value: unknown): ChunkFrame {
  const input = exactRecord(value, ['v', 'type', 'id', 'seq', 'body']);
  if (input.v !== 1 || input.type !== 'chunk') invalid();
  const id = validateId(input.id);
  const seq = validateSequence(input.seq);
  const body = exactString(input.body);
  if (body.length > Math.ceil(limits.maxChunkBodyBytes / 3) * 4) invalid();
  const decodedBody = decodeBody(body);
  if (decodedBody.byteLength === 0 || decodedBody.byteLength > limits.maxChunkBodyBytes) invalid();
  return { v: 1, type: 'chunk', id, seq, body };
}

function validateEnd(value: unknown): EndFrame {
  const input = exactRecord(value, ['v', 'type', 'id']);
  if (input.v !== 1 || input.type !== 'end') invalid();
  return { v: 1, type: 'end', id: validateId(input.id) };
}

function validateCancel(value: unknown): CancelFrame {
  const input = exactRecord(value, ['v', 'type', 'id']);
  if (input.v !== 1 || input.type !== 'cancel') invalid();
  return { v: 1, type: 'cancel', id: validateId(input.id) };
}

function validateCredit(value: unknown): CreditFrame {
  const input = exactRecord(value, ['v', 'type', 'id', 'seq']);
  if (input.v !== 1 || input.type !== 'credit') invalid();
  return { v: 1, type: 'credit', id: validateId(input.id), seq: validateSequence(input.seq) };
}

function validateFrame(value: unknown): ReverseFrame {
  if (!isPlainRecord(value)) invalid();
  switch (value.type) {
    case 'request': return validateRequest(value);
    case 'response': return validateResponse(value);
    case 'chunk': return validateChunk(value);
    case 'end': return validateEnd(value);
    case 'cancel': return validateCancel(value);
    case 'credit': return validateCredit(value);
    default: invalid();
  }
}

export function decodeFrame(text: string): ReverseFrame {
  try {
    if (typeof text !== 'string' || text.length > limits.maxWireBytes || byteLength(text) > limits.maxWireBytes) invalid();
    return validateFrame(JSON.parse(text) as unknown);
  } catch {
    invalid();
  }
}

export function encodeFrame(frame: ReverseFrame): string {
  try {
    const validated = validateFrame(frame);
    const text = JSON.stringify(validated);
    if (typeof text !== 'string' || text.length > limits.maxWireBytes || byteLength(text) > limits.maxWireBytes) invalid();
    return text;
  } catch {
    invalid();
  }
}
