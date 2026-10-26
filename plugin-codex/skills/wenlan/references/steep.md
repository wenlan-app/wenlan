# $wenlan steep

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
raw_args="<the full argument string passed to $wenlan steep>"
# The resolver ships inside the installed plugin; the relative path only
# exists in a wenlan checkout.
resolver="$(find "$HOME/.codex/plugins/cache" -path '*/wenlan/*/bin/resolve-space.sh' 2>/dev/null | head -1)"
resolved="$("${resolver:-plugin-codex/bin/resolve-space.sh}" --cwd "$PWD" --args "$raw_args")"
space="$(printf '%s\n' "$resolved" | cut -f1)"
```

Call `steep` with `action: "list"`, and `space` when it is non-empty. The
answer has the pages waiting (`total`) and `new_total`: material no page uses
yet. If both are 0, say there is nothing to write or update and stop.

New pages come first (section 2), then the pages waiting (section 3).

## 2. New pages from new material

Skip this section when `new_total` is 0.

Grouping and organizing below are small work for a helper on a cheaper
model, so they do not spend your main model. Spawn a subagent on
`gpt-6-luna`; when subagents are off, run
`codex exec -m gpt-6-luna "<the task>"` and use what it prints.
Only when no helper on a cheaper model can start at all, do the grouping and
organizing yourself, for this run only and at most 8 pages, and say in the
report that no cheaper helper was available. You always write the pages
yourself.

1. Call `steep` with `action: "new"` (and `space`). Each item is one imported
   note, chat, file or saved memory, with `item_id`, `kind`, `title` and
   `snippet`. Leave out items whose `held` is true: another run has them.
2. Group the items by topic with the helper: hand it every item's `item_id`,
   `kind`, `title` and `snippet`, and the answer's `page_titles` (the pages
   that already exist), and ask it to reply with only JSON:
   `[{"topic": "...", "item_ids": ["..."]}]`. Each group is one subject a
   person would look up as one page. Material on one subject from different
   years or versions is one group. If the material belongs under an existing
   page, even one whose title words differ (a trip to Porto under "Lisbon
   and Porto"), the topic is that page's title, exactly as given. An item
   belongs to at most one group; items that fit no group are left out. A
   group may be a single item only when that item is long on its own (a
   whole note, a long chat).
3. Biggest groups first, at most 8 pages this run. Finish each group, from
   its claim to its page, before you claim the next, so the first page lands
   within a few minutes. For each group:
   1. Call `steep` with `action: "claim_new"`, its `item_ids` and its topic
      as `title`. The answer has the items' text numbered `[1]` to `[n]`, a
      `token` and a `note`. A refusal (too little text, already taken) means
      skip the group.
   2. When the answer has `organize`, the helper organizes the sources before
      you write, the way Wenlan's own model would: their type and tags, and
      the people, projects and tools they mention. Hand the helper the
      `token`, `organize.stages` and `organize.tasks`. Ask it to answer every
      task with its stage's `instructions` as the system prompt and the
      task's `prompt` as the message, each answer exactly what the
      instructions ask for; then to call `steep` itself with
      `action: "organize"`, the token and `answers` as
      `[{"n": 1, "answer": "..."}]`; and to reply with only one line: the
      counts that call returned. Never pass the
      answers on yourself. Only when you organize yourself (no helper could
      start) do you call `organize` with your own answers.
   3. Right before you write, call `steep` with `action: "links"` and the
      token. It returns `link_targets` and `existing_page` as they are now,
      counting the pages this run wrote since the claim.
   4. When it has `existing_page`, a page on this subject already exists: do
      not write a second one. Call `steep` with `action: "attach"` and the
      token. The sources go into that page, which then waits to be rewritten
      with all of them (section 3). Go on to the next group.
   5. Otherwise write the page yourself, with your main model. Never let the
      small model write it. Title: short, names the topic, in the sources'
      language. Open with one plain sentence that says what the page is
      about, then short paragraphs or lists. Use only what the sources say
      and cite every claim with its number, like `[2]`. Link each page in
      `link_targets` the text talks about once as `[[Title]]`, with the
      title exactly as given. Link no other titles.
   6. Call `steep` with `action: "write"`, the token, `title` and the body as
      `content`. A citation refusal: fix the sentences it points at and write
      again with the same token, once; still refused, `release` the token. If
      the sources turn out not to belong together, `release` the token.
   7. `created`: a new page. `merged`: a page on that topic already existed
      and took the sources. `attached`: the existing page took them.

   The helper for the next group may organize while you write a page, when
   your tool runs helpers in the background: after the page's organize line
   is back, claim the next group and start its helper in the background,
   then write the page.
   Otherwise take the groups one after another.

## 3. Pages that went out of date

Skip this section when `total` is 0 and nothing was attached in section 2.
When something was attached, call `list` again first: the pages that took
sources now wait here.

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
   `source_removed` is always rewritten, and so is a page you attached
   sources to in section 2, with no check.
   When `stale_reason` is `code_changed`, the claim's `code` shows the lines
   as they read now and `what_changed` says what the page should now say.
   An entry with `deleted: true` is a file that was deleted from the
   repository: its `excerpt` shows the lines as they were, and the rewrite
   drops or corrects what only they supported. If, having read the code, the
   page still agrees with it, call `still_accurate`; otherwise rewrite.
   Either way Wenlan then stops relying on the deleted file.
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
commit, the write and the attach are the only writes. Never release a token you did not
get from your own claim.

## 4. Report

A few short lines, no ids: the titles of the new pages written, for each
attach "added N sources to <title>", how many of
their sources were organized and how many of the people, projects and tools
found there earned a Wiki page (`confirmed`), whether you grouped and organized
them yourself because no cheaper helper could start, how many pages were updated, how many wait for the
user as revision cards, how many still hold, how many code changes were
checked and found harmless, and for each `needs_you` page its
title and the reason from its `note`. If more new material or more than 10
pages were left, say so and that `$wenlan steep` again continues.
