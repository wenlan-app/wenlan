---
name: wenlan-handoff
description: "End a work session. Automatically applies typed item-level deltas to the current Space Brief. It stores no memories, writes no session log and edits no pages. Invoked as `/wenlan-handoff`."
---

# /wenlan-handoff

Close the session with one typed update to the daemon-owned Space Brief.

A handoff changes the Brief only. It writes no pages: pages follow a
memory change by themselves, and Wenlan checks the pages a session read at
night. It stores no memories: durable decisions, lessons and preferences are
captured with `/wenlan-capture` while the work happens, not at handoff. It writes no
session log either; older logs in the data folder's `sessions/` stay readable.

The daemon Brief is current-work authority. Its `sessions/_status/<space>.md`
projection in the Wenlan data folder is a one-way human receipt.
Never read, edit, or overwrite that receipt as authority.

## 1. Resolve repository and Space

```bash
handoff_arg="<the Space name passed to /wenlan-handoff, empty when none>"
resolved="$("{{WENLAN_SKILLS_DIR}}/wenlan/scripts/resolve-space.sh" --cwd "$PWD" --arg "$handoff_arg" --new-repo-fallback 2>/dev/null)"
space="$(printf '%s\n' "$resolved" | cut -f1)"
source_layer="$(printf '%s\n' "$resolved" | cut -f2)"
```

If the user passed a Space name (`/wenlan-handoff <space>`), pass it as
`--arg <space>` to resolve-space.sh.

Print `space` and `source_layer`. Explicit pins, defaults, and mappings still
win. `cwd-repo-new` is the approved first-handoff fallback: use the canonical
repository basename, which the user can override through normal Space config.
Do not invent any other Space name.

Outside a Git repository, do not derive a new Space from the directory
basename. If resolution still leaves `space` empty, skip the Brief read and the
typed update, and report that no Space resolved. Outside
a Git repository, a pin, explicit argument, default, or mapping still resolves
a Space, and the Brief update still runs.

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
item's exact ID, version, state, text, added date, and gate.

`brief_not_created` is valid and write-free. Use summary
`expected_version: 0`. For `cwd-repo-new`, prove the Space is absent with
`spaces show` before composing deltas. Accept only the exact CLI error
`Error: space '<name>' not found` as first-handoff absence; any other probe
failure stops the handoff. An absent Space cannot have a Brief or existing
items, so use `expected_version: 0` and author no existing-item mutations. The
typed update may then create the Space and Brief.

## 3. Gather evidence

For a repository, inspect a bounded recent log, short status, diff stat, and
worktree list. Combine that evidence with the conversation. Use it only to
compose the Brief update in step 4; a handoff drafts no captures.

## 4. Build and apply one typed Brief update

Compare the session outcome with `brief_before` and write one
`BriefUpdateRequest` JSON file:

```json
{
  "space": "<resolved Space>",
  "caller_id": "<this AI tool's name in lowercase, such as cursor>",
  "operation_id": "<unique id retained for retries of this handoff>",
  "summary": {
    "text": "<concise last-session summary>",
    "expected_version": 0
  },
  "mutations": []
}
```

Use the existing Brief version instead of `0` when present. When
`daemon_down=1`, use `expected_version: 0` — the summary applies only when
version 0 is still current — and author only `add` mutations;
do not author `edit`, `move`, `set_gate`, or `complete` without a Brief snapshot.
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
the Wenlan `handoff` tool with the same `space`,
`operation_id`, `summary`, and `mutations`; it is the same typed request over
MCP and returns the same receipt. When `daemon_down=1`, use the CLI, which
writes the stable handoff file into the project folder:

```bash
"$W" --format json --space "$space" handoff --file "$update_file"
```

Do not ask approval for this normal handoff update. Interpret `applied`,
`conflicts`, `projection_path`, and `warnings` independently. A `"status":
"saved_in_project"` result is durable but not applied: record the project path
and report it as saved for later import. Non-overlapping changes may commit while a stale
same-item delta conflicts. Re-read before any safe mechanical reconciliation;
never guess.

## 5. Report

Report applied/conflicted/queued Brief deltas, Brief version or outbox path,
and projection path or warning. When anything queued, add: `N handoff note(s)
saved locally — they sync when Wenlan starts (`wenlan outbox status`).`
