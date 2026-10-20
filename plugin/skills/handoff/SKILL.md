---
name: handoff
description: >
  End a work session. Stores durable captures, writes a narrative session log,
  and automatically applies typed item-level deltas to the current Space Brief.
  Invoked as `/handoff`.
allowed-tools: ["Bash", "mcp__plugin_wenlan_wenlan__update_brief", "mcp__plugin_wenlan_wenlan__list_pages", "mcp__plugin_wenlan_wenlan__read_page", "mcp__plugin_wenlan_wenlan__write_page", "mcp__plugin_wenlan_wenlan__page_basis"]
---

# /handoff

Close the session with three separate artifacts:

1. A typed update to the daemon-owned Space Brief.
2. Durable MCP captures in that Space.
3. A chronological session log in `~/.wenlan/sessions/`.

It also brings the wiki pages this session used up to date (step 7).

The daemon Brief is current-work authority. Its
`~/.wenlan/sessions/_status/<space>.md` projection is a one-way human receipt.
Never read, edit, or overwrite that receipt as authority.

## 1. Resolve repository and Space

```bash
handoff_arg="<the Space name passed to /handoff, empty when none>"
resolved="$("$CLAUDE_PLUGIN_ROOT/scripts/resolve-space.sh" --cwd "$PWD" --arg "$handoff_arg" --new-repo-fallback 2>/dev/null)"
space="$(printf '%s\n' "$resolved" | cut -f1)"
source_layer="$(printf '%s\n' "$resolved" | cut -f2)"
```

If the user passed a Space name (`/wenlan:handoff <space>`), pass it as
`--arg <space>` to resolve-space.sh.

Print `space` and `source_layer`. Explicit pins, defaults, and mappings still
win. `cwd-repo-new` is the approved first-handoff fallback: use the canonical
repository basename, which the user can override through normal Space config.
Do not invent any other Space name.

Outside a Git repository, do not derive a new Space from the directory
basename. If resolution still leaves `space` empty, skip the Brief read and
typed update, do not issue Space-scoped captures, and continue with the
unscoped session log and any unscoped durable captures. Outside a Git
repository, a pin, explicit argument, default, or mapping still resolves a
Space, and the Brief update and captures still run.

## 2. Read the Brief before composing deltas

```bash
W="$(command -v wenlan || echo "$HOME/.wenlan/bin/wenlan")"
brief_before=""
daemon_down=0
if [ -n "$space" ]; then
  "$W" --format json status >/dev/null 2>&1 || daemon_down=1
  if [ "$source_layer" = "cwd-repo-new" ]; then
    space_probe_status=0
    space_probe="$("$W" --format json spaces show "$space" 2>&1)" || space_probe_status=$?
    if [ "$space_probe_status" -eq 0 ]; then
      daemon_down=0
      brief_before="$("$W" --format json --space "$space" brief)"
      source_layer="cwd-repo"
    elif [ "$space_probe" = "Error: space '$space' not found" ]; then
      : # absent Space: the typed update creates it
    elif [ "$daemon_down" = 1 ]; then
      brief_before=""
      echo "Wenlan is not running; this handoff will be saved locally and sync when it starts"
    else
      printf "%s\n" "$space_probe" >&2
      exit "$space_probe_status"
    fi
  else
    brief_status=0
    brief_output="$("$W" --format json --space "$space" brief 2>&1)" || brief_status=$?
    if [ "$brief_status" -eq 0 ]; then
      daemon_down=0
      brief_before="$brief_output"
    elif [ "$daemon_down" = 1 ]; then
      brief_before=""
      echo "Wenlan is not running; this handoff will be saved locally and sync when it starts"
    else
      printf "%s\n" "$brief_output" >&2
      exit "$brief_status"
    fi
  fi
fi
```

Read the Brief before composing deltas. This read is mandatory before any Brief
delta is authored for a registered Space. Retain the Brief version and every
item's exact ID, version, state, text, added date, and gate. Use
`last_handoff_at` for the pending-capture window.

`brief_not_created` is valid and write-free. Use summary
`expected_version: 0`. For `cwd-repo-new`, prove the Space is absent with
`spaces show` before composing deltas. Accept only the exact CLI error
`Error: space '<name>' not found` as first-handoff absence; any other probe
failure stops the handoff. An absent Space cannot have a Brief or existing
items, so use `expected_version: 0` and author no existing-item mutations. The
typed update may then create the Space and Brief.

## 3. Preview pending captures and gather evidence

Best-effort; skip silently when `daemon_down=1` or the command fails:

```bash
"$W" --format json --space "$space" memories --pending -l 50
```

Filter by `created_at >= last_handoff_at`, or 12 hours ago when absent. Show
at most three when any match, then continue automatically; `/wenlan curate captures`
remains opt-in.

For a repository, inspect a bounded recent log, short status, diff stat, and
worktree list. Combine that evidence with the conversation. Draft atomic
captures only for durable decisions, lessons, gotchas, corrections,
preferences, and facts. Skip transient or git-recoverable state.

## 4. Write the session log

Write `~/.wenlan/sessions/<YYYY-MM-DD-HHmm>-<slug>.md` with Accomplished,
Decisions, Lessons & Gotchas, Open Threads, Captures stored, Pages touched, and
Git summary. Pages touched lists each page this session used by title with
the planned action; step 7 fills in the outcome.
This is narrative history, not current-work authority. It is a plain file and
must never depend on the daemon, so write it before the Brief update.

## 5. Build and apply one typed Brief update

Compare the session outcome with `brief_before` and write one
`BriefUpdateRequest` JSON file:

```json
{
  "space": "<resolved Space>",
  "caller_id": "claude-code",
  "operation_id": "<unique id retained for retries of this handoff>",
  "summary": {
    "text": "<concise last-session summary>",
    "expected_version": 0
  },
  "mutations": []
}
```

Use the existing Brief version instead of `0` when present. When
`daemon_down=1`, use `expected_version: 0` — the daemon reconciles the
summary version when it replays the queued file — and author only `add`
mutations; do not author `edit`, `move`, `set_gate`, or `complete` without a
Brief snapshot.
If `space` is empty, skip this typed update entirely.

- `add`: genuinely new open work, in `active` or `backlog`, with an optional
  gate.
- `edit`, `move`, `set_gate`, and `complete`: use the exact existing item ID.
- Every delta for one existing item uses the same version from the pre-handoff Brief snapshot.
  Do not chain versions generated by earlier deltas in the same request.
- `complete` removes the item; there is no Done state.
- Never fuzzy-match. Leave an ambiguous item unchanged.
- Never auto-demote untouched Active work.
- Do not add an unchanged duplicate.

Apply exactly once. When the daemon is up (`daemon_down=0`), call
`mcp__plugin_wenlan_wenlan__update_brief` with the same `space`,
`operation_id`, `summary`, and `mutations`; it is the same typed request over
MCP and returns the same receipt. When `daemon_down=1`, use the CLI, which
queues the file in the outbox for replay:

```bash
"$W" --format json --space "$space" brief update --file "$update_file"
```

Do not ask approval for this normal handoff update. Interpret `applied`,
`conflicts`, `projection_path`, and `warnings` independently. A `"status":
"queued"` result is success-queued: record the outbox path and report it as
queued, not applied. Non-overlapping changes may commit while a stale
same-item delta conflicts. Re-read before any safe mechanical reconciliation;
never guess.

Apply the Brief update before Space-scoped captures when this fallback is new.
That creates the basename Space through the typed handoff path without making a
read or a capture create state. If this first update fails, stop Space-scoped
captures and report the exact failure. A queued result is not a failure.

## 6. Store durable captures

For each drafted durable item, run one command:

```bash
"$W" --format json --space "$space" capture -t <type> "<content>"
```

Use one atomic item per call. A `"status": "queued"` result counts as
queued, not failed — record the path. Do not ask about ordinary captures.
Pause only for a contradiction, critical incident, irreversible production
action, or genuine durability ambiguity.

## 7. Update the pages this session used

Skip this step when `daemon_down=1` or `space` is empty.

List the wiki pages this session read, quoted, or changed. Recall them from the
conversation: pages opened with `read_page`, `/wenlan pages`, or `recall` results the
work relied on. Confirm each title against
`mcp__plugin_wenlan_wenlan__list_pages` for the Space; drop any title that does
not match a listed page exactly. Never fuzzy-match.

For each confirmed page, decide whether this session changed what the page
should say: a decision that replaces one it records, a fact it lacks, or a
statement the session proved wrong. Leave the page alone when nothing it says
changed. Restyling is not a reason to write.

To update a page:

1. Call `mcp__plugin_wenlan_wenlan__read_page` and keep `page.version`,
   `page.source_memory_ids` (it may be empty) and `basis` (absent when the page
   relies on no other note).
2. Look at `basis` before editing. For each entry whose `state` is `pending`,
   call `mcp__plugin_wenlan_wenlan__page_basis` with `action: "diff"` to see the
   text this page relied on next to the basis note's text now. If the change
   does not touch what this page says, call it with `action: "confirm"` and
   `version` set to the entry's `to_version`; no rewrite is needed. If it does,
   rewrite the page in steps 3 and 4. A basis that is `archived`, `deleted` or
   `unavailable` cannot be confirmed: leave it and note it in the session log.
3. Edit the current body: keep everything still true, change only what this
   session changed. Do not rewrite from memory of an older read.
4. Call `mcp__plugin_wenlan_wenlan__write_page` with `page_id`, the edited
   `content`, `expected_version` set to that `page.version`, and
   `source_memory_ids` set to the page's existing source ids plus the ids of
   any step 6 captures the new text relies on. Also pass `basis` as
   `[{page_id, version}]` for every note this edit relied on, using the
   `page.version` of each note's own `read_page` (read it first if you have not).
   Memory ids go in `source_memory_ids`; a note's page id goes only in `basis`.

A `basis:` line in the reply that shows an entry still `pending` means that
note changed after you read it: go back to step 2 for it.

A refusal saying the page moved means someone edited it after your read:
read it again and redo the edit once. A `gated` result means the page is
locked for review; report it as staged for review, not as updated. Do not ask
approval for ordinary page updates.

Afterwards, record each outcome (updated, staged for review, unchanged, or
skipped because the daemon was down) under Pages touched in the session log.

## 8. Report

Report capture counts (stored vs queued), session-log path, pages updated or
staged for review,
applied/conflicted/queued Brief deltas, Brief version or outbox path, and
projection path or warning. When anything queued, add: `N handoff note(s)
saved locally — they sync when Wenlan starts (`wenlan outbox status`).`
