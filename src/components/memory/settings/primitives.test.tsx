// SPDX-License-Identifier: AGPL-3.0-only
import { useState } from "react";
import { describe, expect, it, beforeEach, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { i18n } from "../../../i18n";
import {
  Button,
  Field,
  Input,
  Select,
  SectionHeader,
  SettingRow,
  StatusChip,
  Tag,
  Toggle,
} from "./primitives";

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

describe("StatusChip", () => {
  it("renders the success token triplet for an up state", () => {
    render(<StatusChip state={{ kind: "up", detail: "Ollama 0.6.2" }} label="Ollama" />);
    const chip = screen.getByText(/Ollama/).closest("span[aria-live]") as HTMLElement;
    expect(chip.className).toContain("bg-[var(--mem-status-success-bg)]");
    expect(chip.className).toContain("text-[var(--mem-status-success-text)]");
    expect(chip.className).toContain("border-[var(--mem-status-success-border)]");
    expect(screen.getByText("Ollama · Ollama 0.6.2")).toBeInTheDocument();
  });

  it("renders the danger token triplet for a down state", () => {
    render(<StatusChip state={{ kind: "down", detail: "connection refused" }} label="Ollama" />);
    const chip = screen.getByText(/Ollama/).closest("span[aria-live]") as HTMLElement;
    expect(chip.className).toContain("bg-[var(--mem-status-danger-bg)]");
    expect(chip.className).toContain("text-[var(--mem-status-danger-text)]");
    expect(chip.className).toContain("border-[var(--mem-status-danger-border)]");
  });

  it("shows the not-verified i18n string for a stale state", () => {
    render(<StatusChip state={{ kind: "stale" }} label="Ollama" />);
    expect(screen.getByText("Ollama · Not verified yet")).toBeInTheDocument();
  });

  it("marks the container aria-live=polite and the dot aria-hidden", () => {
    const { container } = render(<StatusChip state={{ kind: "idle" }} label="Ollama" />);
    const chip = screen.getByText("Ollama").closest("span[aria-live]");
    expect(chip).toHaveAttribute("aria-live", "polite");
    const dot = container.querySelector('[aria-hidden="true"]');
    expect(dot).not.toBeNull();
  });
});

describe("Field", () => {
  it("wires the control's aria-describedby to the description id", () => {
    render(
      <Field label="Endpoint" htmlFor="endpoint" description="Where the daemon listens">
        <Input />
      </Field>,
    );
    const input = screen.getByLabelText("Endpoint");
    expect(input).toHaveAttribute("aria-describedby", "endpoint-desc");
    expect(screen.getByText("Where the daemon listens")).toHaveAttribute("id", "endpoint-desc");
  });

  it("renders error text outside the <label> element and wins over description", () => {
    render(
      <Field label="Endpoint" htmlFor="endpoint" description="helper" error="Invalid host">
        <Input />
      </Field>,
    );
    const input = screen.getByLabelText("Endpoint");
    expect(input).toHaveAttribute("aria-describedby", "endpoint-error");
    const errorEl = screen.getByRole("alert");
    expect(errorEl).toHaveTextContent("Invalid host");
    const label = screen.getByText("Endpoint");
    expect(label.tagName).toBe("LABEL");
    expect(label).not.toContainElement(errorEl);
    expect(screen.queryByText("helper")).not.toBeInTheDocument();
  });

  it("uses the shared eyebrow tracking token, not a hardcoded letter-spacing", () => {
    render(
      <Field label="Endpoint" htmlFor="endpoint">
        <Input />
      </Field>,
    );
    const label = screen.getByText("Endpoint");
    expect(label).toHaveStyle({ letterSpacing: "var(--mem-tracking-eyebrow)" });
  });
});

describe("Toggle", () => {
  function ControlledToggle() {
    const [enabled, setEnabled] = useState(false);
    return <Toggle enabled={enabled} onToggle={() => setEnabled((e) => !e)} />;
  }

  it("reflects enabled via aria-pressed and flips it on click", async () => {
    const user = userEvent.setup();
    render(<ControlledToggle />);
    const button = screen.getByRole("button");
    expect(button).toHaveAttribute("aria-pressed", "false");
    await user.click(button);
    expect(button).toHaveAttribute("aria-pressed", "true");
  });

  it.each([
    ["zh-Hant", "無法使用"],
    ["zh-Hans", "不可用"],
  ] as const)("localizes unavailable status in %s", async (language, label) => {
    await i18n.changeLanguage(language);
    render(<Toggle enabled={false} valueUnknown onToggle={() => {}} aria-label="Launch at login" />);
    expect(screen.getByRole("status", { name: "Launch at login" })).toHaveTextContent(label);
  });

  it.each([
    ["loading", "Loading"],
    ["unavailable", "Unavailable"],
  ] as const)("renders unknown %s as a named noninteractive status", (state, label) => {
    const onToggle = vi.fn();
    render(<Toggle enabled={false} valueUnknown unknownState={state} onToggle={onToggle}
      aria-label="Launch at login" aria-describedby="login-detail" />);

    const status = screen.getByRole("status", { name: "Launch at login" });
    expect(status).toHaveTextContent(label);
    expect(status).toHaveAttribute("aria-describedby", "login-detail");
    expect(status.tagName).toBe("SPAN");
    expect(status).not.toHaveAttribute("aria-pressed");
    expect(screen.queryByRole("button", { name: "Launch at login" })).not.toBeInTheDocument();
    expect(status.className).toContain("min-h-[26px]");
    expect(status.className).not.toContain("border");
    fireEvent.click(status);
    expect(onToggle).not.toHaveBeenCalled();
  });

  it("keeps the focused switch mounted and inert while saving", () => {
    const onToggle = vi.fn();
    const { rerender } = render(<Toggle enabled={false} valueUnknown unknownState="saving" onToggle={onToggle}
      aria-label="Share optional usage stats" aria-describedby="telemetry-detail" />);

    const toggle = screen.getByRole("button", { name: "Share optional usage stats" });
    const status = screen.getByRole("status", { name: "Saving" });
    toggle.focus();
    expect(toggle).toHaveAttribute("aria-pressed", "false");
    expect(toggle).toHaveAttribute("aria-disabled", "true");
    expect(toggle).toHaveAttribute("aria-busy", "true");
    expect(status).toHaveTextContent("Saving");
    expect(status).toHaveAttribute("aria-describedby", "telemetry-detail");
    expect(toggle).toHaveFocus();

    fireEvent.click(toggle);
    expect(onToggle).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Share optional usage stats" })).toBe(toggle);
    expect(toggle).toHaveFocus();

    rerender(<Toggle enabled onToggle={onToggle} aria-label="Share optional usage stats" />);
    expect(screen.getByRole("button", { name: "Share optional usage stats" })).toBe(toggle);
    expect(toggle).toHaveFocus();
    expect(toggle).toHaveAttribute("aria-pressed", "true");
  });
});

describe("Button", () => {
  it("disables the button when disabled is set", () => {
    render(<Button disabled>Save</Button>);
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
  });

  it("renders every variant", () => {
    const variants = ["primary", "secondary", "ghost", "danger"] as const;
    for (const variant of variants) {
      render(<Button variant={variant}>{variant}</Button>);
      expect(screen.getByRole("button", { name: variant })).toBeInTheDocument();
    }
  });

  it("wraps icon+text children in a flex span, so preflight's svg{display:block} can't force a line break", () => {
    render(
      <Button>
        <svg data-testid="btn-icon" />
        <span>Show config</span>
      </Button>,
    );
    const icon = screen.getByTestId("btn-icon");
    const wrapper = icon.parentElement as HTMLElement;
    expect(wrapper.className).toContain("inline-flex");
    expect(wrapper.className).toContain("items-center");
  });
});

describe("Select", () => {
  it("renders its options and fires onChange with the selected value", () => {
    const onChange = vi.fn();
    render(
      <Select aria-label="Language" value="en" onChange={onChange}>
        <option value="en">English</option>
        <option value="zh-Hans">简体中文</option>
      </Select>,
    );
    const select = screen.getByRole("combobox", { name: "Language" }) as HTMLSelectElement;
    expect(screen.getByRole("option", { name: "English" })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "简体中文" })).toBeInTheDocument();
    fireEvent.change(select, { target: { value: "zh-Hans" } });
    expect(onChange).toHaveBeenCalledTimes(1);
  });

  it("wires into Field like Input does: id + aria-describedby via cloneElement", () => {
    render(
      <Field label="Language" htmlFor="lang" description="Interface language">
        <Select value="en" onChange={() => {}}>
          <option value="en">English</option>
        </Select>
      </Field>,
    );
    const select = screen.getByLabelText("Language");
    expect(select).toHaveAttribute("id", "lang");
    expect(select).toHaveAttribute("aria-describedby", "lang-desc");
  });

  it("hides the chevron glyph from assistive tech", () => {
    const { container } = render(
      <Select aria-label="Language" value="en" onChange={() => {}}>
        <option value="en">English</option>
      </Select>,
    );
    const chevron = container.querySelector('svg')?.closest('[aria-hidden="true"]');
    expect(chevron).not.toBeNull();
  });
});

describe("Tag — chip-never-lies invariant", () => {
  it("renders its label and defaults to the neutral tone", () => {
    render(<Tag>Beta</Tag>);
    const tag = screen.getByText("Beta");
    expect(tag.className).toContain("bg-transparent");
    expect(tag.className).toContain("text-[var(--mem-text-secondary)]");
    expect(tag.className).toContain("border-[var(--mem-border)]");
  });

  it("renders the accent tone on request", () => {
    render(<Tag tone="accent">New</Tag>);
    const tag = screen.getByText("New");
    expect(tag.className).toContain("bg-[var(--mem-indigo-bg)]");
    expect(tag.className).toContain("text-[var(--mem-accent-indigo)]");
    expect(tag.className).toContain("border-transparent");
  });

  it("the tone union has no probe vocabulary, and StatusChip's state cannot be faked with a string (compile-time only, see @ts-expect-error below; pnpm exec tsc -b fails if either stops being an error)", () => {
    // @ts-expect-error — Tag has no probe vocabulary
    <Tag tone="up">Configured</Tag>;
    // @ts-expect-error — StatusChip color cannot be faked with a string
    <StatusChip state="recommended" label="x" />;
    expect(true).toBe(true);
  });
});

describe("SectionHeader", () => {
  it("renders without an icon", () => {
    const { container } = render(<SectionHeader label="Profile" />);
    expect(screen.getByText("Profile")).toBeInTheDocument();
    expect(container.querySelector('[aria-hidden="true"]')).toBeNull();
  });

  it("still renders a given icon", () => {
    const { container } = render(
      <SectionHeader label="Profile" icon={<svg data-testid="icon" />} />,
    );
    expect(container.querySelector('[aria-hidden="true"]')).not.toBeNull();
    expect(screen.getByTestId("icon")).toBeInTheDocument();
  });

  it("renders an action slot, right-aligned via justify-between", () => {
    const { container } = render(
      <SectionHeader label="Diagnostics" action={<button>Refresh</button>} />,
    );
    expect(screen.getByRole("button", { name: "Refresh" })).toBeInTheDocument();
    expect(container.firstElementChild?.className).toContain("justify-between");
  });
});

describe("Toggle — accessible name", () => {
  // The switch renders only a decorative knob, so its accessible name is its
  // ONLY description. Unnamed, a screen reader says "button, pressed" and never
  // says what was toggled. This was true of the hand-rolled role="switch" it
  // replaced, so the bug predates the primitive — pin it so it stays fixed.
  it("is reachable by name, not just by role", () => {
    render(<Toggle enabled={false} onToggle={() => {}} aria-label="Launch at login" />);
    expect(screen.getByRole("button", { name: "Launch at login" })).toBeInTheDocument();
  });

  it("SettingRow names its Toggle with the row title", () => {
    render(
      <SettingRow title="Launch at login" description="Start Wenlan on boot" enabled onToggle={() => {}} />,
    );
    const toggle = screen.getByRole("button", { name: "Launch at login" });
    expect(toggle).toHaveAttribute("aria-pressed", "true");
  });
});

describe("SettingRow", () => {
  it("toggle arm: renders title/description, flips via the Toggle, and wires error to aria-describedby (unchanged behavior)", async () => {
    function ControlledRow() {
      const [enabled, setEnabled] = useState(false);
      return (
        <SettingRow
          title="Run at login"
          description="Start Wenlan automatically"
          enabled={enabled}
          onToggle={() => setEnabled((e) => !e)}
          error="Could not save"
        />
      );
    }
    const user = userEvent.setup();
    render(<ControlledRow />);
    expect(screen.getByText("Run at login")).toBeInTheDocument();
    expect(screen.getByText("Start Wenlan automatically")).toBeInTheDocument();
    const toggle = screen.getByRole("button");
    expect(toggle).toHaveAttribute("aria-pressed", "false");
    expect(toggle).toHaveAttribute("aria-describedby", expect.stringContaining("error"));
    expect(toggle.getAttribute("aria-describedby")).toContain("description");
    await user.click(toggle);
    expect(toggle).toHaveAttribute("aria-pressed", "true");
  });

  it("passes unknownState through and describes the status with the row detail", () => {
    render(<SettingRow title="Launch at login" description="Start Wenlan on boot" enabled={false}
      valueUnknown unknownState="loading" onToggle={() => {}} />);
    const status = screen.getByRole("status", { name: "Launch at login" });
    expect(status).toHaveTextContent("Loading");
    expect(document.getElementById(status.getAttribute("aria-describedby")!)).toHaveTextContent("Start Wenlan on boot");
  });

  it("opens Settings help without activating the switch and dismisses outside or with Escape", async () => {
    const { container } = render(
      <SettingRow
        title="Telemetry"
        description=""
        details="Only aggregate counts and app version are sent."
        statusLine={<p role="status">Pending count remains visible</p>}
        warning="Usage stats are unavailable"
        error="Could not save this setting"
        enabled={false}
        onToggle={() => {}}
      />,
    );
    const toggle = screen.getByRole("button", { name: "Telemetry" });
    const disclosure = screen.getByRole("button", { name: "Details: Telemetry" });
    expect(disclosure.parentElement).toHaveClass("settings-help-anchor");
    expect(disclosure.parentElement?.querySelector(".sr-only")).toHaveTextContent(
      "Only aggregate counts and app version are sent.",
    );
    expect(disclosure).toHaveAttribute("aria-expanded", "false");
    expect(toggle).toHaveAttribute("aria-describedby", expect.stringContaining("details"));
    const detailDescriptionId = toggle.getAttribute("aria-describedby")!.split(" ")[0];
    expect(document.getElementById(detailDescriptionId!)).toHaveTextContent(
      "Only aggregate counts and app version are sent.",
    );
    expect(container.querySelector("p:empty")).toBeNull();
    expect(screen.getByRole("status")).toHaveTextContent("Pending count remains visible");
    expect(screen.getByText("Usage stats are unavailable")).toBeVisible();
    expect(screen.getByText("Could not save this setting")).toBeVisible();

    await userEvent.click(disclosure);
    expect(disclosure).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("tooltip")).toHaveTextContent("Only aggregate counts and app version are sent.");
    expect(toggle).toHaveAttribute("aria-pressed", "false");

    await userEvent.click(document.body);
    expect(disclosure).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();

    await userEvent.click(disclosure);
    await userEvent.keyboard("{Escape}");
    expect(disclosure).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();
    expect(disclosure).toHaveFocus();
  });

  it("control arm: renders the given control instead of a Toggle", () => {
    render(
      <SettingRow
        title="Language"
        description="Interface language"
        control={<div data-testid="custom-control">picker</div>}
      />,
    );
    expect(screen.getByText("Language")).toBeInTheDocument();
    expect(screen.getByTestId("custom-control")).toBeInTheDocument();
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });
});

// Round 5, D5. `could_not_measure` had no chip kind of its own, so it was
// rendered as `down` — the same red, the same tone, as a MEASURED negative.
// The label kept the words apart; colour is read first and said "bad" about a
// state nobody observed. This kind exists so the two are not the same picture.
describe("StatusChip unknown state", () => {
  const chipFor = (label: string) =>
    screen.getByText(new RegExp(label)).closest("span[aria-live]") as HTMLElement;

  it("renders the warning token triplet, not the danger one", () => {
    render(<StatusChip state={{ kind: "unknown" }} label="Last stop" />);
    const chip = chipFor("Last stop");
    expect(chip.className).toContain("bg-[var(--mem-status-warning-bg)]");
    expect(chip.className).toContain("text-[var(--mem-status-warning-text)]");
    expect(chip.className).toContain("border-[var(--mem-status-warning-border)]");
    // The whole point: not the colour a measured negative gets.
    expect(chip.className).not.toContain("var(--mem-status-danger-bg)");
    expect(chip.className).not.toContain("var(--mem-status-success-bg)");
  });

  it("carries its own detail rather than borrowing the stale-state wording", () => {
    render(<StatusChip state={{ kind: "unknown", detail: "pid identity was lost" }} label="Last stop" />);
    expect(screen.getByText("Last stop · pid identity was lost")).toBeInTheDocument();
    expect(screen.queryByText(/Not verified yet/)).toBeNull();
  });
});
