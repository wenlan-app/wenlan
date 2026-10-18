// SPDX-License-Identifier: Apache-2.0
import assert from 'node:assert/strict';
import test from 'node:test';
import { cleanClientName, clientIdentity, displayName, knownClientName, redirectHostOf } from '../src/client-identity.ts';

test('allowlisted redirect hosts are the only known clients', () => {
  for (const [uri, name] of [
    ['https://claude.ai/api/mcp/auth_callback', 'Claude'],
    ['https://claude.com/api/mcp/auth_callback', 'Claude'],
    ['https://chatgpt.com/connector_platform_oauth_redirect', 'ChatGPT'],
    ['https://chat.openai.com/aip/callback', 'ChatGPT'],
    ['https://agent.meta.ai/oauth/callback', 'Muse'],
    ['https://CLAUDE.AI/callback', 'Claude'],
  ] as const) {
    assert.equal(knownClientName(uri), name, uri);
    const identity = clientIdentity(uri, 'whatever');
    assert.equal(identity.knownClient, true, uri);
    assert.equal(displayName(identity), name, uri);
  }
});

test('look-alike, subdomain, non-https, port and userinfo redirects are unknown', () => {
  for (const uri of [
    'https://claude.ai.evil.example/callback',
    'https://evil-claude.ai/callback',
    'https://www.claude.ai/callback',
    'https://claude.ai.example/callback',
    'https://chatgpt.com.attacker.example/callback',
    'https://meta.ai/callback',
    'https://evil.agent.meta.ai/callback',
    'https://xn--claud-0ra.ai/callback',
    'http://claude.ai/callback',
    'https://claude.ai:8443/callback',
    'https://user@claude.ai/callback',
    'https://user:pass@chatgpt.com/callback',
    'claude.ai/callback',
    'not a url',
    '',
  ]) {
    assert.equal(knownClientName(uri), null, uri);
    assert.equal(clientIdentity(uri, 'Claude').knownClient, false, uri);
  }
  assert.equal(knownClientName(undefined), null);
  assert.equal(knownClientName(42), null);
});

test('a hostile DCR client_name never becomes the displayed name or trust', () => {
  const identity = clientIdentity('https://attacker.example/cb', 'Claude');
  assert.deepEqual(identity, { clientName: 'Claude', redirectHost: 'attacker.example', knownClient: false });
  assert.equal(displayName(identity), 'attacker.example');
  // A stored record that claims knownClient for an unlisted host still shows the host.
  assert.equal(displayName({ redirectHost: 'attacker.example', knownClient: true }), 'attacker.example');
  assert.equal(displayName({ redirectHost: '', knownClient: false }), null);
  assert.equal(displayName({}), null);
});

test('client names are trimmed, stripped of control and bidi characters and capped at 80', () => {
  assert.equal(cleanClientName('  My Client\u0000\u0007\n '), 'My Client');
  assert.equal(cleanClientName('Safe‮name⁦x​'), 'Safenamex');
  assert.equal(cleanClientName('x'.repeat(200)), 'x'.repeat(80));
  assert.equal([...cleanClientName('\u{1F600}'.repeat(100))!].length, 80);
  for (const empty of ['', '   ', '\u0000\u0001', null, undefined, 7, {}]) assert.equal(cleanClientName(empty), null);
});

test('redirect host is the lowercased parsed host, or empty', () => {
  assert.equal(redirectHostOf('https://Client.Example:8443/cb?x=1'), 'client.example');
  assert.equal(redirectHostOf('nonsense'), '');
  assert.equal(redirectHostOf(undefined), '');
});
