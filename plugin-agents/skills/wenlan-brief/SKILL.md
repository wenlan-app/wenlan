---
name: wenlan-brief
description: "Read the current Space-owned project Brief from Wenlan. With an optional topic, appends separately labeled related context from the same Space. Invoked as `/wenlan-brief [topic]` when resuming work or asking to catch up."
---

# /wenlan-brief

Read one Space's current project snapshot. The daemon Brief is authoritative;
`~/.wenlan/sessions/_status/*.md` is only a one-way human-readable receipt and must
never be read as product state.

## 1. Resolve the Space

Run the bundled resolver once. It prints the resolved Space line itself; an
empty `space` means unscoped, so omit `space`.

```bash
raw_args="<the full argument string passed to /wenlan-brief>"
resolved="$("{{WENLAN_SKILLS_DIR}}/wenlan/scripts/resolve-space.sh" --cwd "$PWD" --args "$raw_args")"
space="$(printf '%s\n' "$resolved" | cut -f1)"
source_layer="$(printf '%s\n' "$resolved" | cut -f2)"
topic_arg="$(printf '%s\n' "$resolved" | cut -f3-)"
```

## 2. Read the Brief

Call:

```text
brief(
  topic="<topic_arg only when the user supplied one>",
  space="<resolved only when non-empty>"
)
```

Do not invent or infer a topic when the argument is absent. No topic means the
complete Brief alone. A topic means the same complete Brief plus a separate
`Related Context` section scoped to that Space.

Brief reads never create state. If the state is `brief_not_created`, explain
that the first `/wenlan-handoff` update will create it. If the state is
`space_not_resolved`, ask for a Space only when the working directory and
configuration cannot resolve one safely.

Render:

1. Last-session summary.
2. Active items.
3. Backlog items.
4. Related Context, only when returned.

Keep stable item IDs and versions available for later `/wenlan-handoff` reconciliation,
but do not clutter the normal user-facing list with them.

## 3. Pending revisions

After the Brief, call:

```text
list_pending_revisions(limit=10)
```

If empty, say nothing. If the call fails, emit one warning and keep the Brief.
If non-empty, show at most the top three with `target_source_id`,
`revision_content`, and source agent. Never auto-action them.

- accept:
  `accept_revision(target_source_id="<id>")`
- dismiss:
  `dismiss_revision(target_source_id="<id>")`
- skip: no call

If more than three exist, point to `/wenlan curate revisions`.

## Boundary

Use `/wenlan-brief` to resume a Space or answer "catch me up." It is not a mandatory
every-session boot step. Use `/wenlan-recall` for a specific fact, `/wenlan-capture` for a
durable memory, and `/wenlan-handoff` to close a work session.
