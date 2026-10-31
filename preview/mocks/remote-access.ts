// SPDX-License-Identifier: AGPL-3.0-only
// UI-only synthetic fixture. No network, disk credentials or production fallback.
//
// Scenarios, picked from the URL so a screenshot pass can address each state:
//   ?remoteScenario=  connecting | no-apps | apps | expiring | ended |
//                     error-offline | error-rate | error-other |
//                     disconnect-unconfirmed | shutdown-unconfirmed | delayed-grant
//   ?pair=            known | chatgpt | unknown | expired | toomany | offline | long
//                     (the request the approval dialog finds; preview/main.tsx
//                     also opens the dialog with a code when this is present;
//                     ?pairKind= picks the same request without opening it, for
//                     typing a code into "Have a code?" by hand)
//   ?remoteScope=     whole (the scenario's profile and apps share the whole library)
//   ?remoteSpaces=    none (a library with no Spaces yet)
import type { RemoteAccessProfile, RemoteAccessStatus, RemoteGrant, RemotePairing } from "../../src/lib/tauri";
import { emit } from "./tauri-stubs";

const DAY = 86_400_000;
const MINUTE = 60_000;

let revision = 0;
let profile: RemoteAccessProfile | null = null;
let status: RemoteAccessStatus = { status: "off" };
const grants: RemoteGrant[] = [];
const query = new URLSearchParams(window.location.search);
const spaces = query.get("remoteSpaces") === "none"
  ? []
  : [{ id: "review", name: "Review workspace" }, { id: "private", name: "Private library" }];
/** The scope a seeded scenario shares. */
const seededSpace = query.get("remoteScope") === "whole" || spaces.length === 0 ? "*" : spaces[0].name;
const relayUrl = "https://relay.wenlan.app/mcp";
const request: RemotePairing = {
  pairingId: "a".repeat(64), clientId: "synthetic-client-for-review-0123456789",
  resource: relayUrl, scopes: ["wenlan:query"], expiresAt: Date.now() + 10 * MINUTE,
};

const scenario = query.get("remoteScenario");
const pairKind = query.get("pairKind") ?? query.get("pair") ?? "known";

function enabledProfile(expiresInMs: number): RemoteAccessProfile {
  return { revision: String(++revision), space: seededSpace, enabled: true, disconnect_pending: false, credential_expires_at: Date.now() + expiresInMs };
}

function connectedStatus(): RemoteAccessStatus {
  return { status: "connected", tunnel_url: null, relay_url: relayUrl };
}

function grant(id: string, over: Partial<RemoteGrant>): RemoteGrant {
  return {
    id, clientId: `client-${id}-0123456789abcdef`, space: seededSpace,
    createdAt: Date.now() - 3 * DAY, expiresAt: Date.now() + 60 * DAY,
    status: "active", cleanupPending: false,
    // The relay's own name is attacker-controlled, so the screens never trust it.
    clientName: "Whatever the app called itself", redirectHost: "claude.ai", knownClient: true,
    lastUsedAt: null, endReason: null, access: "query",
    ...over,
  };
}

function seedApps() {
  grants.push(
    grant("g-claude", { redirectHost: "claude.ai", knownClient: true, lastUsedAt: Date.now() - 5 * MINUTE, access: "write" }),
    grant("g-chatgpt", { redirectHost: "chatgpt.com", knownClient: true, lastUsedAt: null, createdAt: Date.now() - 20 * MINUTE }),
    grant("g-unknown", { redirectHost: "notes.example.org", knownClient: false, lastUsedAt: Date.now() - 3 * DAY }),
    grant("g-old-claude", { redirectHost: "claude.com", knownClient: true, status: "inactive", endReason: "revoked", lastUsedAt: Date.now() - 9 * DAY }),
    grant("g-old-chatgpt", { redirectHost: "chat.openai.com", knownClient: true, status: "inactive", endReason: "reset", lastUsedAt: null }),
    grant("g-old-unknown", { redirectHost: "notes.example.org", knownClient: false, status: "inactive", endReason: "expired", lastUsedAt: Date.now() - 40 * DAY }),
    grant("g-old-replaced", { redirectHost: "claude.ai", knownClient: true, status: "inactive", endReason: "replaced", lastUsedAt: Date.now() - 11 * DAY }),
  );
}

switch (scenario) {
  case "connecting":
    profile = enabledProfile(90 * DAY);
    status = { status: "starting" };
    break;
  case "no-apps":
    profile = enabledProfile(90 * DAY);
    status = connectedStatus();
    break;
  case "apps":
    profile = enabledProfile(60 * DAY);
    status = connectedStatus();
    seedApps();
    break;
  case "expiring":
    profile = enabledProfile(9 * DAY);
    status = connectedStatus();
    seedApps();
    break;
  case "ended":
    profile = enabledProfile(-2 * DAY);
    status = { status: "error", error: "Remote connection requires device authorization" };
    break;
  case "error-offline":
    profile = enabledProfile(60 * DAY);
    status = { status: "error", error: "Remote connection unavailable; retry later" };
    break;
  case "error-rate":
    profile = enabledProfile(60 * DAY);
    status = { status: "error", error: "Remote connection rate limited" };
    break;
  case "error-other":
    profile = enabledProfile(60 * DAY);
    status = { status: "error", error: "Unexpected remote connection response" };
    break;
  case "disconnect-unconfirmed":
    profile = enabledProfile(DAY);
    status = { status: "error", error: "Local transport stop requested; remote access settings are not confirmed (Remote access credentials could not be stored safely); server revoke failed: Remote connection unavailable; retry later; disconnect must be retried before app restart" };
    break;
  case "shutdown-unconfirmed":
    profile = { revision: String(++revision), space: seededSpace, enabled: false, disconnect_pending: false, credential_expires_at: null };
    status = { status: "error", error: "Local remote-access processes have not been confirmed stopped. New connections are blocked; retry Stop access." };
    break;
  // Starts enabled and connected with no apps, then a synthetic grant appears
  // after ~8s with no pairing UI interaction needed. The grant arrives without
  // any status event, so only grant polling can reveal it.
  case "delayed-grant":
    profile = enabledProfile(DAY);
    status = connectedStatus();
    window.setTimeout(() => {
      if (!profile?.enabled) return;
      grants.push(grant("g-delayed", { createdAt: Date.now(), expiresAt: Date.now() + DAY, clientName: null }));
    }, 8000);
    break;
  default:
    break;
}

/** What the relay would say about the request behind a code, for each scenario. */
function pairingFor(kind: string): RemotePairing {
  switch (kind) {
    case "expired": throw new Error("Remote connection rejected (HTTP 404)");
    case "toomany": throw new Error("Remote connection rate limited");
    case "offline": throw new Error("Remote connection unavailable; retry later");
    case "unknown":
      return { ...request, clientName: "Claude", redirectHost: "notes.example.org", knownClient: false };
    case "chatgpt":
      return { ...request, clientName: "ChatGPT", redirectHost: "chatgpt.com", knownClient: true };
    default:
      return { ...request, clientName: "Claude", redirectHost: "claude.ai", knownClient: true };
  }
}

function requireCurrent(expectedRevision: unknown, message = "Stale fixture revision") {
  if (!profile?.enabled || expectedRevision !== profile.revision) throw new Error(message);
}

export async function invokeRemoteFixture(command: string, args?: Record<string, unknown>): Promise<unknown> {
  switch (command) {
    case "list_spaces": return spaces;
    case "get_remote_access_status": return { ...status };
    case "get_remote_access_profile": return profile && { ...profile };
    // No wenlan://pair link reaches a browser, and the fixture never stops on its own.
    case "take_remote_pairing_link": case "take_remote_access_notice": return null;
    case "reconnect_remote_access": {
      if (!profile?.enabled || profile.disconnect_pending || args?.expectedRevision !== profile.revision) throw new Error("Stale fixture revision");
      const profileSpace = profile.space;
      if (profileSpace !== "*" && !spaces.some((space) => space.name === profileSpace)) throw new Error("Unknown Space");
      status = connectedStatus();
      await emit("remote-access-status", { ...status });
      return { ...status };
    }
    case "configure_remote_access": {
      if (profile?.enabled || (args?.expectedRevision ?? null) !== (profile?.revision ?? null)) throw new Error("Stale fixture revision");
      const scope = args?.scope as { kind?: string; name?: string } | undefined;
      let saved: string;
      if (scope?.kind === "wholeLibrary") saved = "*";
      else if (scope?.kind === "space" && scope.name?.trim() !== "*" && spaces.some((space) => space.name === scope.name)) saved = scope.name!;
      else throw new Error("Unknown Space");
      profile = { revision: String(++revision), space: saved, enabled: false, disconnect_pending: false, credential_expires_at: null };
      return { ...profile };
    }
    case "toggle_remote_access": {
      if (args?.enabled) {
        if (!profile || profile.revision !== args.expectedRevision) throw new Error("Stale fixture revision");
        profile = { ...profile, enabled: true, revision: String(++revision), credential_expires_at: Date.now() + 90 * DAY };
        status = connectedStatus();
      } else {
        if (profile) profile = { ...profile, enabled: false, credential_expires_at: null, revision: String(++revision) };
        grants.length = 0;
        status = { status: "off" };
      }
      await emit("remote-access-status", { ...status });
      return { ...status };
    }
    case "renew_remote_access": {
      requireCurrent(args?.expectedRevision);
      // A fresh 90 days, and the apps connected before must connect again.
      for (const item of grants) if (item.status === "active") { item.status = "inactive"; item.endReason = "reset"; }
      profile = { ...profile!, revision: String(++revision), credential_expires_at: Date.now() + 90 * DAY };
      status = connectedStatus();
      await emit("remote-access-status", { ...status });
      return { ...status };
    }
    case "inspect_remote_pairing": {
      if (args?.pairingId !== request.pairingId || args.expectedRevision !== profile?.revision) throw new Error("Unknown fixture pairing");
      return pairingFor(pairKind);
    }
    case "lookup_remote_pairing": {
      requireCurrent(args?.expectedRevision);
      return pairingFor(pairKind);
    }
    case "deny_remote_pairing": {
      requireCurrent(args?.expectedRevision);
      if (args?.pairingId !== request.pairingId) throw new Error("Unknown fixture pairing");
      return null;
    }
    case "approve_remote_pairing": {
      if (!profile?.enabled || args?.expectedRevision !== profile.revision) throw new Error("Stale fixture consent");
      const inspected = (args?.inspected as RemotePairing | undefined) ?? pairingFor(pairKind);
      grants.push(grant("g1", {
        clientId: request.clientId, space: profile.space, createdAt: Date.now(), expiresAt: Date.now() + DAY,
        clientName: inspected.clientName ?? null, redirectHost: inspected.redirectHost ?? null, knownClient: inspected.knownClient ?? false,
        access: args?.access === "write" ? "write" : "query",
      }));
      return null;
    }
    case "list_remote_grants": return { items: grants.map((item) => ({ ...item })), cursor: null };
    case "revoke_remote_grant": {
      const found = grants.find((item) => item.id === args?.grantId);
      if (!found) throw new Error("Unknown fixture grant");
      found.status = "inactive";
      found.endReason = "revoked";
      return { revoked: true, cleanupPending: false };
    }
    case "test_remote_mcp_connection": return { ok: true, latency_ms: 42, error: null };
    default: throw new Error(`Unsupported remote UI fixture command: ${command}`);
  }
}
