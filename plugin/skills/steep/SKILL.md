---
name: steep
description: >
  Steep on demand: write pages from material no page uses yet (imported
  notes, chats, files, saved memories), then bring every page that went out
  of date up to date, using this AI tool instead of Wenlan's background
  model. A page goes out of date when a source it was written from changes
  or is removed, or when code it relies on changes. Use for "steep",
  "organize my notes into pages", "update my out of date pages", "沉澱",
  "整理成筆記", "把過期的筆記更新". Invoked as `/wenlan steep [space:<name>]`.
argument-hint: "[space:<name>]"
allowed-tools: ["Bash", "Agent", "mcp__plugin_wenlan_wenlan__steep", "mcp__plugin_wenlan_wenlan__read_page"]
user-invocable: false
---

# /wenlan steep

Steep is one job with two modes: Automatic (Wenlan's own model, in the
background) and On demand (this AI tool, now). Both use the same checks and
the same write, so a page steeped here ends up exactly as if Wenlan had done
it. Reply in the user's language. Say "steep" in English and 沉澱 in Chinese;
never call it upkeep or say steeping is off.

When the session start printed a `Wenlan session:` line, pass its value as
`session` on every `steep` commit and write.

## 1. List the work

Resolve the Space the same way the other workflows do:

```bash
raw_args="<the full argument string passed to /wenlan steep>"
resolved="$("${CLAUDE_PLUGIN_ROOT}/scripts/resolve-space.sh" --cwd "$PWD" --args "$raw_args")"
space="$(printf '%s\n' "$resolved" | cut -f1)"
```

Call `steep` with `action: "list"`, and `space` when it is non-empty. The
answer has the pages waiting (`total`) and `new_total`: material no page uses
yet. If both are 0, say there is nothing to write or update and stop.

New pages come first (section 2), then the pages waiting (section 3).

## 2. New pages from new material

Skip this section when `new_total` is 0.

1. Call `steep` with `action: "new"` (and `space`). Each item is one imported
   note, chat, file or saved memory, with `item_id`, `kind`, `title` and
   `snippet`. Leave out items whose `held` is true: another run has them.
2. Group the items by topic. This is a small judgment, so give it to a helper
   on a small model: in Claude Code, start an `Agent` with `model: "haiku"`,
   hand it every item's `item_id`, `kind`, `title` and `snippet`, and ask it
   to reply with only JSON: `[{"topic": "...", "item_ids": ["..."]}]`. Each
   group is one subject a person would look up as one page; an item belongs
   to at most one group; items that fit no group are left out. A group may be
   a single item only when that item is long on its own (a whole note, a long
   chat). Never group with your main model: when you cannot start a helper on
   a small model, write no new pages this run and say that Automatic mode
   will do it.
3. Biggest groups first, at most 8 pages this run. For each group:
   1. Call `steep` with `action: "claim_new"` and its `item_ids`. The answer
      has the items' text numbered `[1]` to `[n]`, a `token` and a `note`. A
      refusal (too little text, already taken) means skip the group.
   2. Write the page yourself, with your main model. Never let the small
      model write it. Title: short, names the topic, in the sources'
      language. Open with one plain sentence that says what the page is
      about, then short paragraphs or lists. Use only what the sources say
      and cite every claim with its number, like `[2]`.
   3. Call `steep` with `action: "write"`, the token, `title` and the body as
      `content`. A citation refusal: fix the sentences it points at and write
      again with the same token, once; still refused, `release` the token. If
      the sources turn out not to belong together, `release` the token.
   4. `created`: a new page. `merged`: a page on that topic already existed
      and took the sources.

## 3. Pages that went out of date

Skip this section when `total` is 0.

Work through at most 10 pages this run, newest first. Skip a page whose
`leased_by` is set: another run has it. Page the ones with `needs_you`; they
are reported at the end, not claimed.

For each page:

1. Call `steep` with `action: "claim"` and its `page_id`. The answer has the
   page, its surviving sources numbered `[1]` to `[n]`, a `token`, a `plan`
   and a `note` that says what to do. A refusal (another run took it, or it
   moved) means skip it and go on.
2. `plan: "needs_you"`: nothing to claim. Keep its `note` for the report.
   `plan: "judge"`: the code this page relies on changed and nobody has
   judged whether that matters. Give each entry of `code_checks` to a helper
   on a small model (in Claude Code, an `Agent` with `model: "haiku"`):
   `judge_instructions` first, then the check's `prompt`, and ask it to reply
   with only the JSON object those instructions describe. Never judge with
   your main model: when you cannot start a helper on a small model,
   `release` the token and leave the page for Wenlan's automatic mode. Call `steep` with `action: "judge"`, the token, and
   `answers` as `[{n, answer}]`, each reply passed back unchanged. If the
   answer says something was `stale`, claim the page again: it now waits for
   a rewrite. Do not rewrite a page on a judge claim.
3. Decide whether it needs a rewrite. Only when `stale_reason` is
   `source_updated`: compare the page with its sources. This is a small
   judgment, so give it to a helper on a small model: in Claude Code, start an
   `Agent` with `model: "haiku"`, hand it the page and the numbered sources,
   and ask one question: does every sentence of the page still match the
   sources, yes or no. When it says yes, call `steep` with
   `action: "still_accurate"` and the token. When it says no, or you cannot
   start a helper on a small model, rewrite. A page whose `stale_reason` is
   `source_removed` is always rewritten.
   When `stale_reason` is `code_changed`, the claim's `code` shows the lines
   as they read now and `what_changed` says what the page should now say.
   If, having read the code, the page still agrees with it, call
   `still_accurate`; otherwise rewrite.
4. Rewrite it yourself, with your main model. Never let the small model write
   the page. Use only the numbered sources, cite every claim with its
   number like `[2]`, keep the title, headings and language, and drop
   anything only a removed source said. Keep a `mermaid` diagram block the
   page has, as it is, unless what it draws changed: then fix those boxes
   and arrows (labels stay in double quotes, never a bare `[n]` in one), or
   drop the diagram when the parts it shows are gone. A reply `warnings`
   entry starting `diagram N line M` means that diagram will not draw: fix
   it in the page with `write_page`, or remove it. For `code_changed`,
   change only what the code contradicts; the code is not a numbered source,
   so a sentence only the code supports carries no `[n]`. When the plan is `review_card`,
   keep every line the claim's `note` lists as the user's own, word for word
   and in place; they need no `[n]`. Then call `steep` with
   `action: "commit"`, the token, and the new body as `content`.
5. Read the commit answer:
   - `written`: done.
   - `review_card`: the user wrote this page, so the rewrite waits for them
     as a revision card. The page itself has not changed.
   - A citation refusal: fix the sentences it points at and commit again with
     the same token, once. Still refused: `release` the token and move on.
   - "changed since you claimed it": claim the page again once and redo it.
     Otherwise move on.

Never call `write_page` on a claimed page or for a claim_new's items; the
commit and the write are the only writes. Never release a token you did not
get from your own claim.

## 4. Report

A few short lines, no ids: the titles of the new pages written, how many pages were updated, how many wait for the
user as revision cards, how many still hold, how many code changes were
checked and found harmless, and for each `needs_you` page its
title and the reason from its `note`. If more new material or more than 10
pages were left, say so and that `/wenlan steep` again continues.
