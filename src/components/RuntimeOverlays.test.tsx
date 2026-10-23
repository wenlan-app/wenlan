import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

vi.mock("./onboarding/MilestoneToaster", () => ({
  MilestoneToaster: () => <div>milestone runtime</div>,
}));
vi.mock("./UpdaterDialog", () => ({
  default: () => <div>updater runtime</div>,
}));
vi.mock("./DaemonVersionWatcher", () => ({
  default: () => <div>daemon version watcher</div>,
}));

import { RuntimeOverlays } from "./RuntimeOverlays";

describe("RuntimeOverlays", () => {
  it("does not mount production daemon or updater consumers in Review", () => {
    render(<RuntimeOverlays review />);

    expect(screen.queryByText("milestone runtime")).not.toBeInTheDocument();
    expect(screen.queryByText("updater runtime")).not.toBeInTheDocument();
  });

  it("preserves the production overlays outside Review", () => {
    render(<RuntimeOverlays review={false} />);

    expect(screen.getByText("milestone runtime")).toBeInTheDocument();
    expect(screen.getByText("updater runtime")).toBeInTheDocument();
  });

  it("keeps the milestone toaster off non-main branches", () => {
    render(<RuntimeOverlays review={false} variant="updater-only" />);

    expect(screen.queryByText("milestone runtime")).not.toBeInTheDocument();
    expect(screen.getByText("updater runtime")).toBeInTheDocument();
  });

  it("mounts the daemon version watcher next to the updater outside Review", () => {
    render(<RuntimeOverlays review={false} />);

    expect(screen.getByText("daemon version watcher")).toBeInTheDocument();
  });
});
