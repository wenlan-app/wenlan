---
name: forget
description: >
  Forget a memory in Wenlan by ID: it leaves recall and pages, and it can
  be restored. Prefer `/capture` with `supersedes` for corrections. Invoked as
  `/wenlan forget <source_id>`.
argument-hint: "<source_id>"
allowed-tools: ["Bash", "mcp__plugin_wenlan_wenlan__forget", "mcp__plugin_wenlan_wenlan__recall"]
user-invocable: false
---

# /wenlan forget

Forget a memory by its `source_id`. Wenlan keeps it out of recall and pages,
and it can be restored.

## Safety

An AI tool's forget does not destroy the memory; only the user can, in the app.
A memory read from the user's own note is refused: propose the change to the
note instead. Prefer `/capture` with `supersedes` pointing at the old memory
when history should be preserved.

Always confirm with the user before calling forget, unless the same user message
already includes an exact id and an unambiguous delete instruction (for example
they pasted the ID and said "delete this").

If the id is missing, call `/recall` first to find candidates or ask the user
for the exact id. The confirmation reply must include the action and id:

```text
delete <id>
```

A bare id is not confirmation.

## Call

After confirmation, call:

```text
forget(memory_id="<source_id>")
```

Delete only one memory per tool call. For bulk cleanup, use `/wenlan curate captures`
so each item has its own decision.

## When to use

- User says "forget this", "delete that", "that's wrong, remove it".
- User explicitly identifies a memory by ID, or confirms `delete <id>` after
  recall found the matching memory.

## When NOT to use

- For corrections, prefer storing a new memory with `supersedes` pointing
  at the old one. That preserves history. Use `/capture` with the
  `supersedes` arg instead. If your agent's trust level is below full, that
  write may stage for human review instead of taking effect immediately.
- Bulk deletions: use `/wenlan curate captures` so each item has its own decision.
- Page rebuilds: use `/wenlan steep rebuild <page-id>`.
