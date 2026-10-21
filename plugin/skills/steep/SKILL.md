---
name: steep
description: >
  Steep on demand: bring every note that went out of date up to date, using
  this AI tool instead of Wenlan's background model. A note goes out of date
  when a source it was written from changes or is removed. Use for "steep",
  "update my out of date notes", "沉澱", "把過期的筆記更新". Invoked as
  `/wenlan steep [space:<name>]`.
argument-hint: "[space:<name>]"
allowed-tools: ["Bash", "Agent", "mcp__plugin_wenlan_wenlan__steep", "mcp__plugin_wenlan_wenlan__read_page"]
user-invocable: false
---

# /wenlan steep

Steep is one job with two modes: Automatic (Wenlan's own model, in the
background) and On demand (this AI tool, now). Both use the same checks and
the same write, so a note steeped here ends up exactly as if Wenlan had done
it. Reply in the user's language. Say "steep" in English and 沉澱 in Chinese;
never call it upkeep or say steeping is off.

## 1. List the work

Resolve the Space the same way the other workflows do:

```bash
raw_args="<the full argument string passed to /wenlan steep>"
resolved="$("${CLAUDE_PLUGIN_ROOT}/scripts/resolve-space.sh" --cwd "$PWD" --args "$raw_args")"
space="$(printf '%s\n' "$resolved" | cut -f1)"
```

Call `steep` with `action: "list"`, and `space` when it is non-empty. If `total` is 0, say nothing is waiting and stop.

Work through at most 10 notes this run, newest first. Skip a note whose
`leased_by` is set: another run has it. Note the ones with `needs_you`; they
are reported at the end, not claimed.

## 2. One note at a time

For each note:

1. Call `steep` with `action: "claim"` and its `page_id`. The answer has the
   note, its surviving sources numbered `[1]` to `[n]`, a `token`, a `plan`
   and a `note` that says what to do. A refusal (another run took it, or it
   moved) means skip it and go on.
2. `plan: "needs_you"`: nothing to claim. Keep its `note` for the report.
3. Decide whether it needs a rewrite. Only when `stale_reason` is
   `source_updated`: compare the note with its sources. This is a small
   judgment, so give it to a helper on a small model: in Claude Code, start an
   `Agent` with `model: "haiku"`, hand it the note and the numbered sources,
   and ask one question: does every sentence of the note still match the
   sources, yes or no. When it says yes, call `steep` with
   `action: "still_accurate"` and the token. When it says no, or you cannot
   start a helper on a small model, rewrite. A note whose `stale_reason` is
   `source_removed` is always rewritten.
4. Rewrite it yourself, with your main model. Never let the small model write
   the note. Use only the numbered sources, cite every claim with its
   number like `[2]`, keep the title, headings and language, and drop
   anything only a removed source said. Then call `steep` with
   `action: "commit"`, the token, and the new body as `content`.
5. Read the commit answer:
   - `written`: done.
   - `review_card`: the user wrote this note, so the rewrite waits for them
     as a revision card. The note itself has not changed.
   - A citation refusal: fix the sentences it points at and commit again with
     the same token, once. Still refused: `release` the token and move on.
   - "changed since you claimed it": claim the note again once and redo it.
     Otherwise move on.

Never call `write_page` on a claimed note; the commit is the only write.
Never release a token you did not get from your own claim.

## 3. Report

A few short lines, no ids: how many notes were updated, how many wait for the
user as revision cards, how many still hold, and for each `needs_you` note its
title and the reason from its `note`. If more than 10 were waiting, say how
many are left and that `/wenlan steep` again continues.
