# $wenlan curate

Power-user audit lever. Most users do not need $wenlan curate in daily flow:

- **Pending revisions** surface in `/brief` automatically (top 3; the person accepts them in the Wenlan app).
- **Pending captures from this session** surface in `/handoff`'s preview block (top 3).
- **Orphan wikilinks** can be inspected while steep processes a requested knowledge arrangement; they are not pending revisions.

Use $wenlan curate only when you want the deep walk those skills intentionally do not force.

**Only the person accepts a card, in the Wenlan app.** A pending revision or a
review proposal is a card. You never accept one: the daemon refuses an accept
from any AI tool, and so does the `wenlan` CLI. Show the cards, tell
the person they wait in the app, and dismiss or reject one only when the person
says so.

**Captures are meant to decay.** A capture lands pending (not yet confirmed) and, if nothing
ever conflicts with it, it quietly fades — that is by design, not a backlog you owe the
system. The only items that *need* a human are genuine conflicts (a contradiction,
or a richer re-capture worth merging); those surface as pending revisions. `$wenlan curate
captures` is an **opt-in audit** for when you *want* to seal a batch (e.g. after a
bulk import), not a daily chore to zero out.

## Safety protocol

Perform no mutation until the user replies. Ambiguous replies do not mutate.
Cancellation, no reply, or unclear syntax is a no-op.

Render at most four items at a time. Number every item. Ask for explicit action
syntax:

```text
1 accept   (captures only; a revision or proposal waits for the person in the app)
2 dismiss
3 skip
4 edit: <replacement text>
```

Do not rely on hidden pagination state across turns. After applying a batch,
re-list before showing another batch.

## `$wenlan curate revisions` (and bare `$wenlan curate`) → via the `wenlan` CLI

Revisions go through the **CLI**, not MCP, because revision actions must key on
the exact `revision_source_id`. Resolve the binary once, then list:

```
W="$(command -v wenlan || echo "$HOME/.wenlan/bin/wenlan")"
"$W" --format json curate
```

That prints a JSON array; each element is ONE logical revision (the CLI already
groups the per-chunk rows and joins their text, so you never see a mid-sentence
fragment):

- `revision_source_id` — the staged revision; the **action key** for dismiss.
  Keying on this (not the target) means the *named* revision is acted on even when
  several compete for one memory.
- `target_source_id` — the memory this revision would replace (context only).
- `content` — the full revised text.
- `source_agent` — who proposed it (or `null` = daemon).
- `original` — the current text the revision would replace (`null` if unavailable).
- `diff` — a card-ready, length-bounded `OLD:` / `NEW:` preview of what the
  revision would replace (two labeled lines, original then revision, each clipped).
  `null` when `original` is unavailable.

**Read the JSON directly.** Parse the array yourself; do not pipe it through
`jq`/`python`/`awk` to reshape first. That extra round-trip is slow (each retry
is a model turn) and failure-prone (quoting, sandboxed temp paths).

**Show the diff so the user isn't deciding blind.** Print `diff` verbatim under
each item. If `diff` is `null`, print `content` and say the original is
unavailable. Don't reformat the diff; it's already length-bounded.

Render up to four revisions, numbered:

```text
Pending revisions:

1. revision_source_id: <rev_id>
   target: <target_source_id>
   proposed: "<content>"
   diff:
   <diff or "(original unavailable)">

Reply with: 1 dismiss, 1 leave
```

- `leave` → nothing: it keeps waiting in the Wenlan app, where the person
  accepts it.
- `dismiss` → it was NOT an edit: unstage the false revision link and keep BOTH
  memories as independent rows. (Non-destructive — nothing is deleted. To delete a
  genuinely unwanted capture, use `$wenlan curate captures` → reject.)

Apply only explicit actions, in ONE Bash call (leave runs nothing):

```
"$W" curate dismiss <revision_source_id>     # for each Dismiss
```

If the array is empty, say so in one line ("Nothing needs you — no pending
conflicts.") and stop. Captures are meant to decay; mention `$wenlan curate captures`
only as an opt-in deep audit the user can run if they *want* to, never as a backlog.

The list now carries the `original` and a ready-made `diff` alongside the revised
`content`, so the user can see what changed without a separate lookup.

## `$wenlan curate captures` (opt-in audit, MCP)

The captures path is the rare opt-in audit, so it stays on MCP. `list_pending`
lists every pending capture (pass `space` only if the user named one, e.g.
"curate work captures"; otherwise leave it off for all pending). Each memory has a
`source_id`. Render up to four, numbered, and ask for `accept`, `reject`, `skip`,
or `edit: <replacement text>`. Per item:

- `accept` → `confirm_memory(memory_id=<source_id>)`
- `reject` → `forget(memory_id=<source_id>)`
- `skip` → nothing
- `edit: <replacement>` → `capture(content=<replacement>, supersedes=<source_id>)`,
  then `forget(memory_id=<source_id>)`. If the capture response says
  `gated: true`, the edit was staged for review instead of applied — skip the
  `forget` call (the original is still the live version) and tell the user
  their correction is waiting on approval in the review queue.

Reject and edit delete the original pending capture, so require the numbered
item and action verb in the same reply.

## `$wenlan curate refinements` (explicit proposal audit, MCP)

Use `$wenlan curate refinements` only when the user explicitly asks to inspect or
review the daemon proposal/refinement queue. Never poll the refinement queue
ambiently.

List first with
`mcp__wenlan__list_refinements(limit=50)` and show at most four
items. Include each proposal id, action, confidence, and bounded payload
summary. The action set includes `vocab_promote`, so vocabulary promotion
candidates stay discoverable.

Only the person accepts a proposal, in the Wenlan app. Perform no mutation
until the user gives an unambiguous item-level reject decision. Use native
numbered item decisions only:

- `leave` → nothing: the person accepts it in the Wenlan app.
- `reject` → `mcp__wenlan__reject_refinement(id="<proposal-id>")`.

Skip or cancel is a no-op. A generic accept does not apply
`lint_repair_review`; route that action through `$wenlan lint repair` instead. Apply
only the explicit item decisions, then re-list. Re-list after every mutation
batch.

## When to use

- After a bulk import (ChatGPT, Obsidian dump) when you want to audit every
  auto-classification before sealing.
- When `/brief` shows ">3 pending revisions" and you want to clear the full
  queue, not just the top 3.
- When the user explicitly asks to inspect or review daemon refinement
  proposals.

## When NOT to use

- Daily session work. `/brief` handles the surface that matters today.
- Specific factual lookup: use `/recall`.
