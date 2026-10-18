// SPDX-License-Identifier: AGPL-3.0-only
/**
 * Who an AI app says it is, as far as Wenlan is willing to repeat it.
 *
 * The app's own name is chosen by whoever registered it and can say anything,
 * so it is never shown. What Wenlan shows comes from the host the app asked to
 * be sent back to, and only a short list of hosts earns a friendly name. The
 * relay reports `knownClient` as well, but this list decides here too: an
 * older or compromised relay answer cannot promote an unknown host.
 */

type KnownClientName = "Claude" | "ChatGPT" | "Muse";

const KNOWN_HOSTS: Record<string, KnownClientName> = {
  "claude.ai": "Claude",
  "claude.com": "Claude",
  "chatgpt.com": "ChatGPT",
  "chat.openai.com": "ChatGPT",
  // Meta's Muse custom connectors return to this host.
  "agent.meta.ai": "Muse",
};

export type ClientIdentity =
  | { kind: "known"; name: KnownClientName }
  /** `host` is null when the relay did not say (an older relay). */
  | { kind: "unknown"; host: string | null };

export function clientIdentity(source: {
  knownClient?: boolean | null;
  redirectHost?: string | null;
}): ClientIdentity {
  const host = typeof source.redirectHost === "string" && source.redirectHost.trim()
    ? source.redirectHost.trim().toLowerCase() : null;
  const name = host ? KNOWN_HOSTS[host] : undefined;
  if (source.knownClient === true && name) return { kind: "known", name };
  return { kind: "unknown", host };
}
