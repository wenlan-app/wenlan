// SPDX-License-Identifier: AGPL-3.0-only
import { beforeEach, describe, expect, it, vi } from "vitest";

const emit = vi.hoisted(() => vi.fn());
vi.mock("../../src/web/tauri-stubs", () => ({ emit }));

beforeEach(() => {
  vi.resetModules();
  emit.mockReset();
  window.history.replaceState({}, "", "/");
});

describe("remote reconnect fixture contract", () => {
  it("preserves the profile revision and grants with no off event", async () => {
    const { invokeRemoteFixture: invoke } = await import("./remote-access");
    const configured = await invoke("configure_remote_access", { scope: { kind: "space", name: "Review workspace" } }) as { revision: string };
    await invoke("toggle_remote_access", { enabled: true, expectedRevision: configured.revision });
    const before = await invoke("get_remote_access_profile") as { revision: string };
    await invoke("approve_remote_pairing", { expectedRevision: before.revision });
    const grants = await invoke("list_remote_grants");
    emit.mockClear();
    await expect(invoke("reconnect_remote_access", { expectedRevision: before.revision })).resolves.toMatchObject({ status: "connected" });
    expect(await invoke("get_remote_access_profile")).toEqual(before);
    expect(await invoke("list_remote_grants")).toEqual(grants);
    expect(emit.mock.calls).toEqual([["remote-access-status", expect.objectContaining({ status: "connected" })]]);
    await expect(invoke("reconnect_remote_access", { expectedRevision: "stale" })).rejects.toThrow("Stale fixture revision");
    expect(await invoke("list_remote_grants")).toEqual(grants);
    await invoke("toggle_remote_access", { enabled: false });
    expect(await invoke("list_remote_grants")).toEqual({ items: [], cursor: null });
    const disabled = await invoke("get_remote_access_profile") as { revision: string };
    await expect(invoke("reconnect_remote_access", { expectedRevision: disabled.revision })).rejects.toThrow("Stale fixture revision");
  });
});

describe("whole-library fixture contract", () => {
  it("saves the whole library as its own choice and refuses a Space named like it", async () => {
    const { invokeRemoteFixture: invoke } = await import("./remote-access");
    await expect(invoke("configure_remote_access", { scope: { kind: "space", name: "*" } })).rejects.toThrow("Unknown Space");
    const configured = await invoke("configure_remote_access", { scope: { kind: "wholeLibrary" } }) as { revision: string; space: string };
    expect(configured.space).toBe("*");
    await invoke("toggle_remote_access", { enabled: true, expectedRevision: configured.revision });
    const on = await invoke("get_remote_access_profile") as { revision: string };
    await expect(invoke("reconnect_remote_access", { expectedRevision: on.revision })).resolves.toMatchObject({ status: "connected" });
  });

  it("seeds a whole-library scenario and a library with no Spaces", async () => {
    window.history.replaceState({}, "", "/?remoteScenario=apps&remoteScope=whole");
    let fixture = await import("./remote-access");
    expect(await fixture.invokeRemoteFixture("get_remote_access_profile")).toMatchObject({ space: "*" });
    vi.resetModules();
    window.history.replaceState({}, "", "/?remoteSpaces=none");
    fixture = await import("./remote-access");
    expect(await fixture.invokeRemoteFixture("list_spaces")).toEqual([]);
  });
});

describe("pairing request fixture contract", () => {
  async function turnedOn(search = "") {
    window.history.replaceState({}, "", search ? `/?${search}` : "/");
    const { invokeRemoteFixture: invoke } = await import("./remote-access");
    const configured = await invoke("configure_remote_access", { scope: { kind: "space", name: "Review workspace" } }) as { revision: string };
    await invoke("toggle_remote_access", { enabled: true, expectedRevision: configured.revision });
    const current = await invoke("get_remote_access_profile") as { revision: string };
    return { invoke, revision: current.revision };
  }

  it("finds the request by short code and names the app by its host", async () => {
    const { invoke, revision } = await turnedOn();
    await expect(invoke("lookup_remote_pairing", { expectedRevision: revision, code: "K7MQ-4WXZ" }))
      .resolves.toMatchObject({ redirectHost: "claude.ai", knownClient: true });
    await expect(invoke("lookup_remote_pairing", { expectedRevision: "stale", code: "K7MQ-4WXZ" }))
      .rejects.toThrow("Stale fixture revision");
  });

  it.each([
    ["unknown", { redirectHost: "notes.example.org", knownClient: false }],
    ["chatgpt", { redirectHost: "chatgpt.com", knownClient: true }],
  ])("serves the %s request", async (pair, expected) => {
    const { invoke, revision } = await turnedOn(`pair=${pair}`);
    await expect(invoke("lookup_remote_pairing", { expectedRevision: revision, code: "K7MQ-4WXZ" }))
      .resolves.toMatchObject(expected);
  });

  it.each([
    ["expired", "Remote connection rejected (HTTP 404)"],
    ["toomany", "Remote connection rate limited"],
    ["offline", "Remote connection unavailable; retry later"],
  ])("fails the %s request the way the relay does", async (pair, message) => {
    const { invoke, revision } = await turnedOn(`pair=${pair}`);
    await expect(invoke("lookup_remote_pairing", { expectedRevision: revision, code: "K7MQ-4WXZ" }))
      .rejects.toThrow(message);
  });

  it("allows with the looked-up app, denies without adding one", async () => {
    const { invoke, revision } = await turnedOn("pair=unknown");
    const found = await invoke("lookup_remote_pairing", { expectedRevision: revision, code: "K7MQ-4WXZ" });
    await invoke("deny_remote_pairing", { expectedRevision: revision, pairingId: "a".repeat(64) });
    expect(await invoke("list_remote_grants")).toEqual({ items: [], cursor: null });
    await invoke("approve_remote_pairing", { expectedRevision: revision, inspected: found });
    const listed = await invoke("list_remote_grants") as { items: { redirectHost: string; knownClient: boolean; status: string }[] };
    expect(listed.items).toHaveLength(1);
    expect(listed.items[0]).toMatchObject({ redirectHost: "notes.example.org", knownClient: false, status: "active" });
  });

  it("renews with a fresh 90 days and ends the apps connected before", async () => {
    // The scenario starts already on, with apps and nine days left.
    window.history.replaceState({}, "", "/?remoteScenario=expiring");
    const { invokeRemoteFixture: invoke } = await import("./remote-access");
    const before = await invoke("get_remote_access_profile") as { revision: string; credential_expires_at: number };
    expect(before.credential_expires_at - Date.now()).toBeLessThan(10 * 86_400_000);
    await expect(invoke("renew_remote_access", { expectedRevision: "stale" })).rejects.toThrow("Stale fixture revision");
    await invoke("renew_remote_access", { expectedRevision: before.revision });
    const after = await invoke("get_remote_access_profile") as { revision: string; credential_expires_at: number };
    expect(after.revision).not.toBe(before.revision);
    expect(after.credential_expires_at - Date.now()).toBeGreaterThan(89 * 86_400_000);
    const listed = await invoke("list_remote_grants") as { items: { status: string; endReason?: string }[] };
    expect(listed.items.filter((item) => item.status === "active")).toEqual([]);
  });

  it("never has a notice or a link waiting", async () => {
    const { invoke } = await turnedOn();
    await expect(invoke("take_remote_access_notice")).resolves.toBeNull();
    await expect(invoke("take_remote_pairing_link")).resolves.toBeNull();
  });
});
