// SPDX-License-Identifier: Apache-2.0

/** Display identity for an OAuth client. The DCR `client_name` is chosen by
 * whoever registered the client, so it never decides trust or the displayed
 * name. Only an exact, allowlisted redirect host marks a client as known.
 */
export interface ClientIdentity {
  clientName: string | null;
  redirectHost: string;
  knownClient: boolean;
}

const KNOWN_CLIENTS: ReadonlyMap<string, string> = new Map([
  ['claude.ai', 'Claude'],
  ['claude.com', 'Claude'],
  ['chatgpt.com', 'ChatGPT'],
  ['chat.openai.com', 'ChatGPT'],
  // Meta's Muse custom connectors return to this host.
  ['agent.meta.ai', 'Muse'],
]);

/** Trimmed, control characters removed, at most 80 characters, or null. */
export function cleanClientName(value: unknown): string | null {
  if (typeof value !== 'string') return null;
  // eslint-disable-next-line no-control-regex
  const cleaned = value.replace(/[\u0000-\u001f\u007f-\u009f​-‏‪-‮⁦-⁩]/g, '').trim();
  return cleaned ? [...cleaned].slice(0, 80).join('').trim() : null;
}

/** Lowercased host of a redirect URI, or '' when it cannot be parsed. */
export function redirectHostOf(redirectUri: unknown): string {
  if (typeof redirectUri !== 'string') return '';
  try { return new URL(redirectUri).hostname.toLowerCase(); } catch { return ''; }
}

/** Known only for an https redirect with no port or credentials whose host is
 * exactly on the allowlist. Subdomains and look-alike hosts are unknown.
 */
export function knownClientName(redirectUri: unknown): string | null {
  if (typeof redirectUri !== 'string') return null;
  let url: URL;
  try { url = new URL(redirectUri); } catch { return null; }
  if (url.protocol !== 'https:' || url.port || url.username || url.password) return null;
  return KNOWN_CLIENTS.get(url.hostname.toLowerCase()) ?? null;
}

export function clientIdentity(redirectUri: unknown, clientName: unknown): ClientIdentity {
  return { clientName: cleanClientName(clientName), redirectHost: redirectHostOf(redirectUri),
    knownClient: knownClientName(redirectUri) !== null };
}

/** The name shown to people: the allowlisted product name, otherwise the
 * redirect host. Never the self-declared DCR name.
 */
export function displayName(identity: { redirectHost?: string | null; knownClient?: boolean }): string | null {
  const host = identity.redirectHost ?? '';
  if (identity.knownClient === true) {
    const name = KNOWN_CLIENTS.get(host);
    if (name) return name;
  }
  return host || null;
}
