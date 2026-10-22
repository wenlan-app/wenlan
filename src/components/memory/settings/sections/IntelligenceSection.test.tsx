// SPDX-License-Identifier: AGPL-3.0-only
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import "../../../../i18n";

const mocks = vi.hoisted(() => ({
  getApiKey: vi.fn(),
  setApiKey: vi.fn(),
  getModelChoice: vi.fn(),
  setModelChoice: vi.fn(),
  getExternalLlm: vi.fn(),
  setExternalLlm: vi.fn(),
  testExternalLlm: vi.fn(),
  listExternalModels: vi.fn(),
  getExternalLlmKeyConfigured: vi.fn(),
  getDaemonVersion: vi.fn(),
  getOnDeviceModel: vi.fn(),
  getSystemInfo: vi.fn(),
  downloadOnDeviceModel: vi.fn(),
  getResolvedRouting: vi.fn(),
  setSourcePin: vi.fn(),
  getBackgroundAiEnabled: vi.fn(),
  setBackgroundAiEnabled: vi.fn(),
  getLockMyPages: vi.fn(),
  setLockMyPages: vi.fn(),
}));
vi.mock("../../../../lib/tauri", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../../../lib/tauri")>();
  return { ...actual, ...mocks };
});

import IntelligenceSection from "./IntelligenceSection";

function renderSection(qc: QueryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })) {
  render(
    <QueryClientProvider client={qc}>
      <IntelligenceSection delay={0} />
    </QueryClientProvider>
  );
  return qc;
}

// A resolved-routing payload the app treats as PINNED mode. Callers override the
// per-job routes; the pool below has all three sources configured so source
// options render enabled.
function pinnedRouting(over: {
  everyday?: { source: string; model: string | null; mode: string; pin?: string | null };
  synthesis?: { source: string; model: string | null; mode: string; pin?: string | null };
} = {}) {
  return {
    everyday: over.everyday ?? { source: "anthropic", model: "claude-opus-4-6", mode: "pinned" },
    synthesis: over.synthesis ?? { source: "anthropic", model: "claude-sonnet-4-6", mode: "pinned" },
    pool: {
      anthropic: { configured: true, everyday_model: "claude-opus-4-6", synthesis_model: "claude-sonnet-4-6" },
      external: { endpoint: "https://api.openai.com/v1", model: "gpt-5.2" },
      on_device: { selected: "qwen3-4b-instruct-2507", loaded: true },
    },
  };
}

describe("IntelligenceSection", () => {
  afterEach(() => {
    Object.values(mocks).forEach((m) => m.mockReset());
  });

  beforeEach(() => {
    mocks.getApiKey.mockResolvedValue(null);
    mocks.setApiKey.mockResolvedValue(undefined);
    mocks.getModelChoice.mockResolvedValue([null, null]);
    mocks.setModelChoice.mockResolvedValue(undefined);
    mocks.getExternalLlm.mockResolvedValue([null, null]);
    mocks.setExternalLlm.mockResolvedValue(undefined);
    mocks.testExternalLlm.mockResolvedValue({ response: "pong" });
    mocks.listExternalModels.mockResolvedValue([]);
    mocks.getExternalLlmKeyConfigured.mockResolvedValue(false);
    mocks.getDaemonVersion.mockResolvedValue("0.12.0");
    mocks.getOnDeviceModel.mockResolvedValue({ loaded: null, selected: null, models: [] });
    mocks.getSystemInfo.mockResolvedValue({
      total_ram_gb: 16,
      available_ram_gb: 10,
      has_metal: true,
      has_cuda: false,
      os: "macOS",
      arch: "arm64",
      recommended_builtin: "qwen3-4b-instruct-2507",
    });
    mocks.downloadOnDeviceModel.mockResolvedValue(undefined);
    // Default to LEGACY mode — the live 0.13.2 daemon has no routing endpoint.
    mocks.getResolvedRouting.mockResolvedValue(null);
    mocks.setSourcePin.mockResolvedValue(undefined);
    mocks.getLockMyPages.mockResolvedValue(false);
    mocks.setLockMyPages.mockResolvedValue(null);
    // Automatic by default, so the task rows under it render.
    mocks.getBackgroundAiEnabled.mockResolvedValue(true);
    mocks.setBackgroundAiEnabled.mockResolvedValue(undefined);
  });

  it("offers steeping as two modes, never as an on/off switch", async () => {
    mocks.getBackgroundAiEnabled.mockResolvedValue(false);
    renderSection();

    const group = await screen.findByRole("radiogroup", { name: "Steeping" });
    const onDemand = within(group).getByRole("radio", { name: "On demand" });
    await waitFor(() => expect(onDemand).toBeChecked());
    expect(within(group).getByRole("radio", { name: "Automatic" })).not.toBeChecked();
    expect(screen.queryByRole("button", { name: "Steeping" })).not.toBeInTheDocument();
    expect(screen.getByText(/run \/wenlan steep/)).toBeInTheDocument();
    // On demand needs no model, so the task rows stay out of the way.
    expect(screen.queryByText("Everyday work")).not.toBeInTheDocument();
  });

  it("lets a note-only install choose Automatic without choosing a model", async () => {
    mocks.getBackgroundAiEnabled.mockResolvedValue(false);
    mocks.setBackgroundAiEnabled.mockImplementation(async (enabled: boolean) => {
      mocks.getBackgroundAiEnabled.mockResolvedValue(enabled);
    });
    renderSection();

    const automatic = await screen.findByRole("radio", { name: "Automatic" });
    await waitFor(() => expect(automatic).toBeEnabled());
    await userEvent.click(automatic);
    expect(mocks.setBackgroundAiEnabled).toHaveBeenCalledWith(true);
    await waitFor(() => expect(screen.getByRole("radio", { name: "Automatic" })).toBeChecked());
    expect(mocks.setSourcePin).not.toHaveBeenCalled();
    expect(await screen.findByText("Everyday work")).toBeInTheDocument();
  });

  it("says where text goes in Automatic", async () => {
    mocks.getResolvedRouting.mockResolvedValue(pinnedRouting({
      everyday: { source: "on_device", model: "qwen3-4b-instruct-2507", mode: "pinned" },
      synthesis: { source: "anthropic", model: "claude-sonnet-4-6", mode: "pinned" },
    }));
    renderSection();
    expect(await screen.findByTestId("steep-trust")).toHaveTextContent("Synthesis sends its text to Anthropic. Everything else stays on this computer.");
  });

  it("counts a cloud vendor behind the external slot as leaving the computer", async () => {
    mocks.getResolvedRouting.mockResolvedValue(pinnedRouting({
      everyday: { source: "external", model: "gpt-5.2", mode: "pinned" },
      synthesis: { source: "on_device", model: "qwen3-4b-instruct-2507", mode: "pinned" },
    }));
    renderSection();
    expect(await screen.findByTestId("steep-trust")).toHaveTextContent(/^Everyday work sends its text to OpenAI/);
  });

  it("says nothing leaves when every job runs on this computer", async () => {
    mocks.getResolvedRouting.mockResolvedValue(pinnedRouting({
      everyday: { source: "on_device", model: "qwen3-4b-instruct-2507", mode: "pinned" },
      synthesis: { source: "on_device", model: "qwen3-4b-instruct-2507", mode: "pinned" },
    }));
    renderSection();
    expect(await screen.findByTestId("steep-trust")).toHaveTextContent("Everything stays on this computer.");
  });

  it("keeps ordinary Background AI readiness out of the settings copy", async () => {
    mocks.getResolvedRouting.mockResolvedValue(pinnedRouting());
    renderSection();

    await screen.findByRole("radio", { name: "Automatic" });
    expect(screen.queryByText("AI available")).not.toBeInTheDocument();
    expect(screen.queryByText("Background AI is off")).not.toBeInTheDocument();
    expect(screen.queryByText("Some tasks still need an AI service.")).not.toBeInTheDocument();
  });

  it("fills unset jobs when Automatic is chosen after providers were set up On demand", async () => {
    mocks.getBackgroundAiEnabled.mockResolvedValue(false);
    mocks.getResolvedRouting.mockResolvedValue(pinnedRouting({
      everyday: { source: "basic", model: null, mode: "unconfigured", pin: null },
      synthesis: { source: "none", model: null, mode: "unconfigured", pin: null },
    }));
    mocks.setBackgroundAiEnabled.mockImplementation(async (enabled: boolean) => {
      mocks.getBackgroundAiEnabled.mockResolvedValue(enabled);
    });
    const qc = renderSection();
    const invalidateSpy = vi.spyOn(qc, "invalidateQueries");
    const automatic = await screen.findByRole("radio", { name: "Automatic" });
    await waitFor(() => expect(automatic).toBeEnabled());

    await userEvent.click(automatic);

    await waitFor(() => expect(mocks.setSourcePin).toHaveBeenCalledWith("on_device", "anthropic", true));
    expect(invalidateSpy).toHaveBeenCalledWith({ queryKey: ["resolvedRouting"] });
    expect(invalidateSpy).toHaveBeenCalledWith({ queryKey: ["activity"] });
  });

  it("shows unavailable when the latest steeping read fails", async () => {
    mocks.getBackgroundAiEnabled
      .mockResolvedValueOnce(false)
      .mockRejectedValueOnce(new Error("settings unavailable"));
    const qc = renderSection();
    await waitFor(() => expect(screen.getByRole("radio", { name: "On demand" })).toBeChecked());

    await qc.invalidateQueries({ queryKey: ["backgroundAiEnabled"] });

    await waitFor(() => expect(screen.getByRole("radio", { name: "On demand" })).not.toBeChecked());
    expect(screen.getByRole("radio", { name: "On demand" })).toBeDisabled();
    expect(screen.getByRole("radio", { name: "Automatic" })).not.toBeChecked();
    expect(await screen.findByText("Could not save this setting. Try again.")).toBeInTheDocument();
  });

  it("shows independent Anthropic models without a task disclosure or model summary", async () => {
    mocks.getApiKey.mockResolvedValue("sk-ant-***configured");
    mocks.getModelChoice.mockResolvedValue(["claude-haiku-4-5-20251001", "claude-sonnet-4-6"]);
    renderSection();
    const everyday = await screen.findByLabelText("Choose everyday model");
    await waitFor(() => expect(everyday).toHaveValue("claude-haiku-4-5-20251001"));
    expect(screen.getByLabelText("Choose synthesis model")).toHaveValue("claude-sonnet-4-6");
    expect(screen.queryByRole("button", { name: "Everyday work" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Synthesis" })).not.toBeInTheDocument();
    expect(screen.queryByText("Anthropic · Haiku 4.5")).not.toBeInTheDocument();
  });

  it("legacy mode shows a service, not a duplicate on-device model, and leaves synthesis not set up", async () => {
    mocks.getOnDeviceModel.mockResolvedValue({ loaded: "qwen3-4b-instruct-2507", selected: "qwen3-4b-instruct-2507",
      models: [{ id: "qwen3-4b-instruct-2507", display_name: "Qwen3 4B Instruct", cached: true }] });
    renderSection();
    const everyday = (await screen.findByText("Everyday work")).closest<HTMLElement>(".settings-task-row")!;
    expect(await within(everyday).findByText("On-device")).toBeInTheDocument();
    expect(within(everyday).queryByText("Qwen3 4B Instruct")).not.toBeInTheDocument();
    const synthesis = screen.getByText("Synthesis").closest<HTMLElement>(".settings-task-row")!;
    expect(within(synthesis).getByText("Not set up")).toBeInTheDocument();
    expect(within(synthesis).getByRole("button", { name: "Set up" })).toBeInTheDocument();
  });

  it("legacy service is read-only but independent model changes stay functional", async () => {
    mocks.getApiKey.mockResolvedValue("sk-ant-***configured");
    mocks.getModelChoice.mockResolvedValue(["claude-opus-4-6", null]);
    renderSection();
    const select = await screen.findByLabelText("Choose everyday model");
    await waitFor(() => expect(select).toHaveValue("claude-opus-4-6"));
    expect(screen.queryByLabelText("AI service for Everyday work")).not.toBeInTheDocument();
    await userEvent.selectOptions(select, "claude-haiku-4-5-20251001");
    expect(mocks.setModelChoice).toHaveBeenCalledWith("claude-haiku-4-5-20251001", null);
    expect(mocks.setSourcePin).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button", { name: "Details: Everyday work" }));
    expect(await screen.findByRole("tooltip")).toHaveTextContent("Changing the source needs the updated Wenlan runtime.");
  });

  it("uses authoritative routing and changes the source directly, without expanding", async () => {
    mocks.getApiKey.mockResolvedValue("sk-ant-***configured");
    mocks.getResolvedRouting.mockResolvedValue(pinnedRouting({
      everyday: { source: "external", model: "gpt-5.2", mode: "pinned", pin: "external" },
    }));
    const qc = renderSection();
    const invalidate = vi.spyOn(qc, "invalidateQueries");
    const select = await screen.findByLabelText("AI service for Everyday work");
    expect(select).toHaveValue("external");
    const row = select.closest<HTMLElement>(".settings-task-row")!;
    expect(within(row).queryByLabelText("Choose everyday model")).not.toBeInTheDocument();
    expect(within(row).queryByText("gpt-5.2")).not.toBeInTheDocument();
    await userEvent.selectOptions(select, "on_device");
    expect(mocks.setSourcePin).toHaveBeenCalledWith("on_device", null);
    await waitFor(() => expect(invalidate).toHaveBeenCalledWith({ queryKey: ["activity"] }));
  });

  it("keeps the configured preference selected when it differs from the unavailable route", async () => {
    const routing = pinnedRouting({
      everyday: { source: "on_device", model: "qwen3-4b-instruct-2507", mode: "pinned", pin: "on_device" },
      synthesis: { source: "external", model: "gpt-5.2", mode: "pinned_unavailable", pin: "anthropic" },
    });
    routing.pool.anthropic.configured = false;
    mocks.getResolvedRouting.mockResolvedValue(routing);
    renderSection();
    const select = await screen.findByLabelText("AI service for Synthesis");
    expect(select).toHaveValue("anthropic");
    expect(within(select).getByRole("option", { name: "Anthropic" })).toBeDisabled();
    const row = select.closest<HTMLElement>(".settings-task-row")!;
    expect(within(row).getByRole("status")).toHaveTextContent("Anthropic is unavailable. This work is paused.");
    expect(within(row).queryByText("OpenAI · gpt-5.2")).not.toBeInTheDocument();
    await userEvent.selectOptions(select, "external");
    expect(mocks.setSourcePin).toHaveBeenCalledWith(null, "external");
  });

  it("does not pretend an unloaded pinned model is enabled", async () => {
    const routing = pinnedRouting({
      synthesis: { source: "on_device", model: null, mode: "pinned_unavailable", pin: "on_device" },
    });
    routing.pool.on_device.loaded = false;
    mocks.getResolvedRouting.mockResolvedValue(routing);
    renderSection();
    const select = await screen.findByLabelText("AI service for Synthesis");
    expect(select).toHaveValue("on_device");
    const row = select.closest<HTMLElement>(".settings-task-row")!;
    expect(within(row).getByRole("status")).toHaveTextContent("On-device is unavailable. This work is paused.");
    expect(within(row).queryByText("Enabled")).not.toBeInTheDocument();
  });

  it("shows an unavailable state when older routing omitted the pin", async () => {
    mocks.getResolvedRouting.mockResolvedValue(pinnedRouting({
      synthesis: { source: "external", model: "gpt-5.2", mode: "pinned_unavailable", pin: null },
    }));
    renderSection();
    const select = await screen.findByLabelText("AI service for Synthesis");
    expect(select).toHaveValue("");
    expect(within(select).getByRole("option", { selected: true })).toHaveTextContent("Unavailable");
    expect(select.closest(".settings-task-row")).toHaveTextContent("The chosen AI is unavailable. This work is paused.");
  });

  it("no providers: Basic and Not set up each appear once, with a working setup link", async () => {
    const scroll = vi.fn();
    const previousScroll = Element.prototype.scrollIntoView;
    Element.prototype.scrollIntoView = scroll;
    try {
      renderSection();
      const everyday = (await screen.findByText("Everyday work")).closest<HTMLElement>(".settings-task-row")!;
      const synthesis = screen.getByText("Synthesis").closest<HTMLElement>(".settings-task-row")!;
      await within(synthesis).findByText("Not set up");
      expect(within(everyday).getAllByText("Basic")).toHaveLength(1);
      expect(within(synthesis).getAllByText("Not set up")).toHaveLength(1);
      await userEvent.click(within(everyday).getByRole("button", { name: "Set up" }));
      expect(screen.getByRole("tab", { name: "On-device" })).toHaveAttribute("aria-selected", "true");
      expect(scroll).toHaveBeenCalledWith({ behavior: "smooth", block: "start" });
      expect(mocks.setSourcePin).not.toHaveBeenCalled();
    } finally { Element.prototype.scrollIntoView = previousScroll; }
  });

  it("unconfigured source selections are visible with unavailable options disabled", async () => {
    mocks.getResolvedRouting.mockResolvedValue({
      everyday: { source: "basic", model: null, mode: "unconfigured", pin: null },
      synthesis: { source: "none", model: null, mode: "unconfigured", pin: null },
      pool: { anthropic: { configured: false, everyday_model: null, synthesis_model: null }, external: null, on_device: null },
    });
    renderSection();
    const select = await screen.findByLabelText("AI service for Synthesis");
    const options = within(select).getAllByRole("option") as HTMLOptionElement[];
    expect(select).toHaveValue("");
    expect(options.filter((option) => !option.disabled)).toHaveLength(0);
    expect(within(select).getByRole("option", { selected: true })).toHaveTextContent("Not set up");
  });

  it("synthesis can directly select On-device without opening an editing mode", async () => {
    mocks.getResolvedRouting.mockResolvedValue(pinnedRouting());
    renderSection();
    const select = await screen.findByLabelText("AI service for Synthesis");
    expect(within(select).getByRole("option", { name: "On-device" })).toBeEnabled();
    await userEvent.selectOptions(select, "on_device");
    expect(mocks.setSourcePin).toHaveBeenCalledWith(null, "on_device");
  });

  it("shared model appears only in AI services and switching stays there", async () => {
    mocks.getResolvedRouting.mockResolvedValue(pinnedRouting({
      everyday: { source: "on_device", model: "qwen3-4b-instruct-2507", mode: "pinned", pin: "on_device" },
      synthesis: { source: "on_device", model: "qwen3-4b-instruct-2507", mode: "pinned", pin: "on_device" },
    }));
    mocks.getOnDeviceModel.mockResolvedValue({ loaded: "qwen3-4b-instruct-2507", selected: "qwen3-4b-instruct-2507", models: [
      { id: "qwen3-4b-instruct-2507", display_name: "Qwen3 4B", param_count: "4B", ram_required_gb: 6, file_size_gb: 2.7, cached: true },
      { id: "qwen3-8b", display_name: "Qwen3 8B", param_count: "8B", ram_required_gb: 10, file_size_gb: 5.2, cached: true },
    ] });
    renderSection();
    await screen.findByLabelText("AI service for Everyday work");
    const everyday = screen.getByText("Everyday work").closest<HTMLElement>(".settings-task-row")!;
    const synthesis = screen.getByText("Synthesis").closest<HTMLElement>(".settings-task-row")!;
    expect(within(everyday).queryByText(/Qwen/)).not.toBeInTheDocument();
    expect(within(synthesis).queryByText(/Qwen/)).not.toBeInTheDocument();
    expect(screen.queryByText("Shared model")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Choose on-device model")).not.toBeInTheDocument();
    expect(mocks.downloadOnDeviceModel).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("tab", { name: "On-device" }));
    expect(screen.getAllByText("Qwen3 4B", { exact: true })).toHaveLength(1);
    const enable = await within(screen.getByRole("tabpanel")).findByRole("button", { name: "Enable" });
    await userEvent.click(enable);
    await waitFor(() => expect(mocks.downloadOnDeviceModel).toHaveBeenCalledWith("qwen3-8b"));
  });

  it("a failed direct service change reports the error and keeps the saved selection", async () => {
    mocks.getResolvedRouting.mockResolvedValue(pinnedRouting());
    mocks.setSourcePin.mockRejectedValue(new Error("daemon offline"));
    renderSection();
    const select = await screen.findByLabelText("AI service for Everyday work");
    await userEvent.selectOptions(select, "on_device");
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not change the AI service. Try again.");
    expect(select).toHaveValue("anthropic");
    expect(select).toBeEnabled();
  });


  it("service tabs preserve drafts and do not change task assignments", async () => {
    renderSection();
    const panel = await screen.findByRole("tabpanel");
    expect(await screen.findByRole("combobox", { name: "Provider" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Anthropic" })).not.toBeInTheDocument();
    const input = within(panel).getByPlaceholderText("sk-ant-api03-...");
    await userEvent.type(input, "unsaved-draft");
    await userEvent.click(screen.getByRole("tab", { name: "Local server" }));
    expect(screen.getByRole("tab", { name: "Local server" })).toHaveAttribute("aria-selected", "true");
    await userEvent.click(screen.getByRole("tab", { name: "Cloud" }));
    expect(input).toHaveValue("unsaved-draft");
    expect(mocks.setApiKey).not.toHaveBeenCalled();
    expect(mocks.setExternalLlm).not.toHaveBeenCalled();
    expect(mocks.setSourcePin).not.toHaveBeenCalled();
  });

  it("keyboard navigation switches service views without changing assignments", async () => {
    renderSection();
    const cloud = await screen.findByRole("tab", { name: "Cloud" });
    cloud.focus();
    await userEvent.keyboard("{ArrowRight}");
    expect(screen.getByRole("tab", { name: "On-device" })).toHaveAttribute("aria-selected", "true");
    await userEvent.keyboard("{End}");
    expect(screen.getByRole("tab", { name: "Local server" })).toHaveAttribute("aria-selected", "true");
    expect(mocks.setSourcePin).not.toHaveBeenCalled();
  });

  const deviceModels = [
    { id: "qwen3-4b-instruct-2507", display_name: "Qwen3 4B", param_count: "4B", ram_required_gb: 6, file_size_gb: 2.7, cached: true },
    { id: "qwen3.5-9b", display_name: "Qwen3.5 9B", param_count: "9B", ram_required_gb: 12, file_size_gb: 5.2, cached: true },
    { id: "qwen3-14b", display_name: "Qwen3 14B", param_count: "14B", ram_required_gb: 20, file_size_gb: 9, cached: false },
  ];

  it("separates downloaded and enabled models, then refreshes actual activation state", async () => {
    mocks.getApiKey.mockResolvedValue("configured");
    mocks.getOnDeviceModel.mockResolvedValue({ loaded: "qwen3-4b-instruct-2507", selected: "qwen3-4b-instruct-2507", models: deviceModels });
    mocks.downloadOnDeviceModel.mockImplementation(async (id: string) => {
      mocks.getOnDeviceModel.mockResolvedValue({ loaded: id, selected: id, models: deviceModels });
    });
    renderSection();
    await userEvent.click(screen.getByRole("tab", { name: "On-device" }));
    const panel = screen.getByRole("tabpanel");
    const enable = await within(panel).findByRole("button", { name: "Enable" });
    expect(within(panel).getByText("Enabled", { exact: true })).toBeVisible();
    expect(within(panel).getByText("Downloaded", { exact: true })).toBeVisible();
    expect(within(panel).getByRole("button", { name: "Download & enable" })).toBeDisabled();
    await userEvent.click(enable);
    await waitFor(() => expect(mocks.downloadOnDeviceModel).toHaveBeenCalledWith("qwen3.5-9b"));
    await waitFor(() => expect(within(panel).getByText("Qwen3.5 9B").closest(".settings-row")).toHaveTextContent("Enabled"));
    expect(mocks.setSourcePin).not.toHaveBeenCalled();
  });

  it("reports a model activation error without showing it as enabled", async () => {
    mocks.getOnDeviceModel.mockResolvedValue({ loaded: null, selected: null, models: [deviceModels[0]] });
    mocks.downloadOnDeviceModel.mockRejectedValue(new Error("model load failed"));
    renderSection();
    await userEvent.click(screen.getByRole("tab", { name: "On-device" }));
    const panel = screen.getByRole("tabpanel");
    await userEvent.click(await within(panel).findByRole("button", { name: "Enable" }));
    expect(await within(panel).findByRole("alert")).toHaveTextContent("model load failed");
    expect(within(panel).queryByText("Enabled", { exact: true })).not.toBeInTheDocument();
  });

  it("does not offer a downloaded but unloaded device model as an available AI service", async () => {
    const routing = pinnedRouting();
    routing.pool.on_device.loaded = false;
    mocks.getResolvedRouting.mockResolvedValue(routing);
    renderSection();
    const select = await screen.findByLabelText("AI service for Synthesis");
    expect(within(select).getByRole("option", { name: "On-device (set up in AI services)" })).toBeDisabled();
  });

  it("Synthesis Beta help explains plugin/MCP even without a model", async () => {
    renderSection();
    await userEvent.click(await screen.findByRole("button", { name: "Details: Beta" }));
    const tooltip = await screen.findByRole("tooltip");
    expect(tooltip).toHaveTextContent("Background synthesis is in Beta");
    expect(tooltip).toHaveTextContent("Wenlan's plugin or MCP with Claude or Codex");
    await userEvent.keyboard("{Escape}");
    expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Details: Beta" })).toHaveFocus();
  });

  describe("Lock pages I've written", () => {
    it("switching it on locks the user's pages and says how many in one line", async () => {
      mocks.getBackgroundAiEnabled.mockResolvedValue(false);
      mocks.setLockMyPages.mockResolvedValue(12);
      renderSection();
      const row = await screen.findByTestId("lock-my-pages");
      expect(within(row).getByText("AI changes to pages you wrote or edited wait for your review. Pages AI wrote keep updating.")).toBeInTheDocument();
      const toggle = await within(row).findByRole("button", { name: "Lock pages I've written" });
      expect(toggle).toHaveAttribute("aria-pressed", "false");
      await userEvent.click(toggle);
      expect(mocks.setLockMyPages).toHaveBeenCalledWith(true);
      expect(await within(row).findByText("Locked 12 pages.")).toBeInTheDocument();
      expect(within(row).getByRole("button", { name: "Lock pages I've written" })).toHaveAttribute("aria-pressed", "true");
    });

    it("switching it off unlocks nothing and shows no count", async () => {
      mocks.getBackgroundAiEnabled.mockResolvedValue(false);
      mocks.getLockMyPages.mockResolvedValue(true);
      renderSection();
      const row = await screen.findByTestId("lock-my-pages");
      const toggle = await within(row).findByRole("button", { name: "Lock pages I've written" });
      await waitFor(() => expect(toggle).toHaveAttribute("aria-pressed", "true"));
      await userEvent.click(toggle);
      expect(mocks.setLockMyPages).toHaveBeenCalledWith(false);
      await waitFor(() => expect(within(row).getByRole("button", { name: "Lock pages I've written" })).toHaveAttribute("aria-pressed", "false"));
      expect(within(row).queryByText(/Locked/)).toBeNull();
    });

    it("a failed save says so and keeps the switch where it was", async () => {
      mocks.getBackgroundAiEnabled.mockResolvedValue(false);
      mocks.setLockMyPages.mockRejectedValue(new Error("daemon down"));
      renderSection();
      const row = await screen.findByTestId("lock-my-pages");
      await userEvent.click(await within(row).findByRole("button", { name: "Lock pages I've written" }));
      expect(await within(row).findByText("Could not save this setting. Try again.")).toBeInTheDocument();
      expect(within(row).getByRole("button", { name: "Lock pages I've written" })).toHaveAttribute("aria-pressed", "false");
    });
  });
});
