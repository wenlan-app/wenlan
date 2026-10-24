// SPDX-License-Identifier: AGPL-3.0-only
import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { toast } from "sonner";
import {
  acceptNoteProposal,
  dismissNoteProposal,
  getNoteAiChanges,
  getNoteProposal,
  listNoteProposals,
  setNoteAiChanges,
  type NoteProposal,
  type RegisteredSource,
} from "../../../lib/tauri";
import { NoteAiChangesSection, NoteProposalNotice, VaultSuggestions, inPagesFolder, trimUnchanged, vaultKey } from "./VaultNotes";

vi.mock("../../../lib/tauri", () => ({
  acceptNoteProposal: vi.fn(),
  dismissNoteProposal: vi.fn(),
  getKnowledgePath: vi.fn(),
  getNoteAiChanges: vi.fn(),
  getNoteProposal: vi.fn(),
  listNoteProposals: vi.fn(),
  setNoteAiChanges: vi.fn(),
}));
vi.mock("sonner", () => ({ toast: vi.fn() }));

const vault: RegisteredSource = {
  id: "notes", source_type: "obsidian", path: "/Users/me/My notes", status: "Active",
  last_sync: 1, file_count: 3, memory_count: 3,
};
const proposal: NoteProposal = {
  id: "np_0123456789abcdef", source_id: "notes", path: "Work/plan.md",
  content: "Launch in October.\n", reason: "The planning call moved it.", agent: "claude-code",
  created_at: 1, base_hash: "x",
};

function wrap(children: ReactNode) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={client}>{children}</QueryClientProvider>);
}

beforeEach(() => {
  vi.resetAllMocks();
  const stored = new Map<string, "ask_first" | "never">();
  const read = (path: string) => {
    const value = stored.get(path);
    return value ? { path, value, from: "file" as const } : { path, value: "never" as const, from: "default" as const };
  };
  vi.mocked(getNoteAiChanges).mockImplementation(async (_id, path) => read(path));
  vi.mocked(setNoteAiChanges).mockImplementation(async (_id, path, value) => {
    if (value) stored.set(path, value); else stored.delete(path);
    return read(path);
  });
  vi.mocked(listNoteProposals).mockResolvedValue([]);
});

describe("vault note paths", () => {
  it("names the vault, a folder and a note by their keys", () => {
    expect(vaultKey("/Users/me/My notes", "/Users/me/My notes", false)).toBe("");
    expect(vaultKey("/Users/me/My notes/", "/Users/me/My notes/Work", true)).toBe("Work/");
    expect(vaultKey("/Users/me/My notes", "/Users/me/My notes/Work/plan.md", false)).toBe("Work/plan.md");
    expect(vaultKey("C:\\Notes", "C:\\Notes\\a.md", false)).toBe("a.md");
    expect(vaultKey("/Users/me/My notes", "/Users/me/My notes 2/a.md", false)).toBeNull();
  });

  it("knows Wenlan's own pages folder", () => {
    expect(inPagesFolder("/v/Wenlan/page.md", "/v/Wenlan")).toBe(true);
    expect(inPagesFolder("/v/Wenlan", "/v/Wenlan/")).toBe(true);
    expect(inPagesFolder("/v/Wenlanish/a.md", "/v/Wenlan")).toBe(false);
    expect(inPagesFolder("/v/a.md", undefined)).toBe(false);
  });

  it("shortens long unchanged text around a change", () => {
    const long = "word ".repeat(200);
    const trimmed = trimUnchanged([{ kind: "same", text: long }, { kind: "ins", text: " new" }, { kind: "same", text: long }]);
    expect(trimmed[0]!.text.startsWith(" … ")).toBe(true);
    expect(trimmed[0]!.text.length).toBeLessThan(200);
    expect(trimmed[2]!.text.endsWith(" … ")).toBe(true);
    expect(trimmed[1]).toEqual({ kind: "ins", text: " new" });
  });
});

describe("NoteAiChangesSection", () => {
  it("starts a note at Never and lets the person switch it to Ask me first", async () => {
    wrap(<NoteAiChangesSection source={vault} path="/Users/me/My notes/Work/plan.md" kind="note" />);
    const never = await screen.findByRole("radio", { name: /Never/ });
    expect(never).toBeChecked();
    expect(screen.getByText("Your notes start at Never.")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("radio", { name: /Ask me first/ }));
    await waitFor(() => expect(setNoteAiChanges).toHaveBeenCalledWith("notes", "Work/plan.md", "ask_first"));
    expect(await screen.findByRole("radio", { name: /Ask me first/ })).toBeChecked();
    fireEvent.click(await screen.findByRole("button", { name: "Use the folder's setting" }));
    await waitFor(() => expect(setNoteAiChanges).toHaveBeenLastCalledWith("notes", "Work/plan.md", null));
  });

  it("explains the vault default and that PDFs never change", async () => {
    wrap(<NoteAiChangesSection source={vault} path="/Users/me/My notes" kind="vault" />);
    expect(await screen.findByText(/To let AI help keep them current, switch to Ask me first/)).toBeInTheDocument();
    expect(screen.getByText(/PDFs are always Never. A folder or a note can have its own setting/)).toBeInTheDocument();
    expect(getNoteAiChanges).toHaveBeenCalledWith("notes", "");
  });

  it("shows a PDF as Never with nothing to choose", async () => {
    vi.mocked(getNoteAiChanges).mockResolvedValue({ path: "paper.pdf", value: "never", from: "pdf" });
    wrap(<NoteAiChangesSection source={vault} path="/Users/me/My notes/paper.pdf" kind="note" />);
    expect(await screen.findByText("PDFs are always Never.")).toBeInTheDocument();
    expect(screen.queryByRole("radio")).toBeNull();
  });
});

describe("NoteProposalNotice", () => {
  beforeEach(() => {
    vi.mocked(listNoteProposals).mockResolvedValue([proposal]);
    vi.mocked(getNoteProposal).mockResolvedValue({ proposal, current: "Launch in September.\n" });
  });

  it("shows what the AI wants to change and writes only on Accept", async () => {
    vi.mocked(acceptNoteProposal).mockResolvedValue({ file: "/Users/me/My notes/Work/plan.md" });
    const onAccepted = vi.fn();
    wrap(<NoteProposalNotice source={vault} path="/Users/me/My notes/Work/plan.md" onAccepted={onAccepted} />);
    expect(await screen.findByText("AI wants to change this note")).toBeInTheDocument();
    expect(screen.getByText("The planning call moved it.")).toBeInTheDocument();
    expect(screen.getByText("Suggested by claude-code")).toBeInTheDocument();
    expect((await screen.findByText(/September/)).tagName).toBe("DEL");
    expect(screen.getByText(/October/).tagName).toBe("INS");
    expect(screen.getByText(/^Red is removed, green is added\./)).toBeInTheDocument();
    expect(acceptNoteProposal).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Accept" }));
    await waitFor(() => expect(acceptNoteProposal).toHaveBeenCalledWith(proposal.id));
    await waitFor(() => expect(onAccepted).toHaveBeenCalled());
    expect(toast).toHaveBeenCalledWith("Changed. Your note has the new text.");
  });

  it("explains only green when the change only adds text", async () => {
    vi.mocked(getNoteProposal).mockResolvedValue({ proposal, current: "Launch in\n" });
    wrap(<NoteProposalNotice source={vault} path="/Users/me/My notes/Work/plan.md" />);
    expect(await screen.findByText(/^Green is added\./)).toBeInTheDocument();
    expect(screen.queryByText(/Red is removed/)).toBeNull();
  });

  it("says so when the note changed after the suggestion", async () => {
    vi.mocked(acceptNoteProposal).mockRejectedValue("Conflict: note_changed_since_proposal");
    wrap(<NoteProposalNotice source={vault} path="/Users/me/My notes/Work/plan.md" />);
    fireEvent.click(await screen.findByRole("button", { name: "Accept" }));
    await waitFor(() => expect(toast).toHaveBeenCalledWith(expect.stringContaining("so the suggestion was dropped")));
  });

  it("declines without touching the note, and shows nothing for other notes", async () => {
    vi.mocked(dismissNoteProposal).mockResolvedValue();
    wrap(<NoteProposalNotice source={vault} path="/Users/me/My notes/Work/plan.md" />);
    fireEvent.click(await screen.findByRole("button", { name: "Decline" }));
    await waitFor(() => expect(dismissNoteProposal).toHaveBeenCalledWith(proposal.id));
    expect(acceptNoteProposal).not.toHaveBeenCalled();
  });

  it("stays hidden for a note with no suggestion", async () => {
    wrap(<NoteProposalNotice source={vault} path="/Users/me/My notes/Other.md" />);
    await waitFor(() => expect(listNoteProposals).toHaveBeenCalled());
    expect(screen.queryByText("AI wants to change this note")).toBeNull();
  });
});

describe("VaultSuggestions", () => {
  it("lists waiting suggestions and opens one", async () => {
    vi.mocked(listNoteProposals).mockResolvedValue([proposal]);
    const onOpen = vi.fn();
    wrap(<VaultSuggestions source={vault} onOpen={onOpen} />);
    fireEvent.click(await screen.findByRole("button", { name: /plan\.md/ }));
    expect(onOpen).toHaveBeenCalledWith(proposal);
  });
});
