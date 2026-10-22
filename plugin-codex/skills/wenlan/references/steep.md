# $wenlan steep

Steep is one job with two modes: Automatic (Wenlan's own model, in the
background) and On demand (this AI tool, now). Both use the same checks and
the same write, so a page steeped here ends up exactly as if Wenlan had done
it. Reply in the user's language. Say "steep" in English and 沉澱 in Chinese;
never call it upkeep or say steeping is off.

When the session start printed a `Wenlan session:` line, pass its value as
`session` on every `steep` commit.

## 1. List the work

Resolve the Space the same way the other workflows do:

```bash
raw_args="<the full argument string passed to $wenlan steep>"
# The resolver ships inside the installed plugin; the relative path only
# exists in a wenlan checkout.
resolver="$(find "$HOME/.codex/plugins/cache" -path '*/wenlan/*/bin/resolve-space.sh' 2>/dev/null | head -1)"
resolved="$("${resolver:-plugin-codex/bin/resolve-space.sh}" --cwd "$PWD" --args "$raw_args")"
space="$(printf '%s\n' "$resolved" | cut -f1)"
```

Call `steep` with `action: "list"`, and `space` when it is non-empty. If `total` is 0, say nothing is waiting and stop.

Work through at most 10 pages this run, newest first. Skip a page whose
`leased_by` is set: another run has it. Page the ones with `needs_you`; they
are reported at the end, not claimed.

## 2. One page at a time

For each page:

1. Call `steep` with `action: "claim"` and its `page_id`. The answer has the
   page, its surviving sources numbered `[1]` to `[n]`, a `token`, a `plan`
   and a `note` that says what to do. A refusal (another run took it, or it
   moved) means skip it and go on.
2. `plan: "needs_you"`: nothing to claim. Keep its `note` for the report.
   `plan: "judge"`: the code this page relies on changed and nobody has
   judged whether that matters. Give each entry of `code_checks` to a helper
   on a small model when you can start one (a subagent on a smaller model):
   `judge_instructions` first, then the check's `prompt`, and ask it to reply
   with only the JSON object those instructions describe. Never judge with
   your main model: when you cannot start a helper on a small model,
   `release` the token and leave the page for Wenlan's automatic mode. Call `steep` with `action: "judge"`, the token, and
   `answers` as `[{n, answer}]`, each reply passed back unchanged. If the
   answer says something was `stale`, claim the page again: it now waits for
   a rewrite. Do not rewrite a page on a judge claim.
3. Decide whether it needs a rewrite. Only when `stale_reason` is
   `source_updated`: compare the page with its sources. This is a small
   judgment, so give it to a helper on a small model when you can start one
   (a subagent on a smaller model), hand it the page and the numbered sources,
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
   anything only a removed source said. For `code_changed`, change only what
   the code contradicts; the code is not a numbered source, so a sentence
   only the code supports carries no `[n]`. Then call `steep` with
   `action: "commit"`, the token, and the new body as `content`.
5. Read the commit answer:
   - `written`: done.
   - `review_card`: the user wrote this page, so the rewrite waits for them
     as a revision card. The page itself has not changed.
   - A citation refusal: fix the sentences it points at and commit again with
     the same token, once. Still refused: `release` the token and move on.
   - "changed since you claimed it": claim the page again once and redo it.
     Otherwise move on.

Never call `write_page` on a claimed page; the commit is the only write.
Never release a token you did not get from your own claim.

## 3. Report

A few short lines, no ids: how many pages were updated, how many wait for the
user as revision cards, how many still hold, how many code changes were
checked and found harmless, and for each `needs_you` page its
title and the reason from its `note`. If more than 10 were waiting, say how
many are left and that `$wenlan steep` again continues.
