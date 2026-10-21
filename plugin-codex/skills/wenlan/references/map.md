# $wenlan map

Read this repository and keep a small set of orientation notes in its Space.
Each key sentence is tied to the code lines it was read from (a code basis), so
Wenlan can tell when the code moved on. The notes are written by the AI. A
rerun refreshes only the notes whose cited code changed. Reply in the user's
language.

## 1. Resolve repository and Space

The skill accepts one optional inline token of the form `space:<name>`. The
bundled resolver strips it and prints the resolved Space line itself:

```bash
raw_args="<the full argument string passed to $wenlan map>"
# The resolver ships inside the installed plugin; the relative path only
# exists in a wenlan checkout.
resolver="$(find "$HOME/.codex/plugins/cache" -path '*/wenlan/*/bin/resolve-space.sh' 2>/dev/null | head -1)"
resolved="$("${resolver:-plugin-codex/bin/resolve-space.sh}" --cwd "$PWD" --args "$raw_args")"
space="$(printf '%s\n' "$resolved" | cut -f1)"
repo_root="$(git -C "$PWD" rev-parse --show-toplevel)"
```

If the `git rev-parse` fails, say this folder is not a git repository and stop.
If `space` is empty, say this folder is not matched to a Space and that
`$wenlan map space:<name>` names one, then stop. Do not ask a question.

## 2. The six notes

Title each note in the user's language, taken from how they typed the request.

| # | Topic | What it answers | English title | 繁體中文 title |
|---|---|---|---|---|
| 1 | What this is | One paragraph and where the entry points are | `What Wenlan is` | `文瀾是什麼` |
| 2 | How to run it | Install, build, start | `How to run it` | `怎麼跑起來` |
| 3 | Main flow | The one or two most important paths, from entry to effect | `Main flow` | `主流程` |
| 4 | Module map | Directories and what each one owns | `Module map` | `模組地圖` |
| 5 | How tests run | The layers, and which are slow | `How tests run` | `測試怎麼跑` |
| 6 | Known pitfalls | From git log, comments and existing docs | `Known pitfalls` | `已知的坑` |

Title 1 names the project: the table shows the Wenlan repository, so use the
repository's own name for another one (`What <project> is`, `<project>是什麼`).

## 3. Decide per note: write, refresh, or leave

Call `list_pages` with `space="<space>"` once. For each of the six topics, find its note
by title in the user's language: an exact title match against the list, never
fuzzy.

- Missing: write it (step 4).
- Present, and listed with a `code_basis` entry in state `pending` or
  `unavailable` whose `repo_root` is this repository: refresh it (step 5).
- Present with nothing outstanding: leave it, and say so in the report.
  `list_pages` rechecks the cited files first and lists only code bases that
  are not `ok`, so a note with none listed is current.

Do not call `read_page` on a note you are leaving as is.

## 4. Writing a note

Read the repository with Read, Glob, Grep and `git ls-files`. Never read a file
`git check-ignore` reports, a `.env` or other secrets, or anything outside
`repo_root`. Do not run build, test or install commands, and write nothing into
the repository.

Compose a short note (a screen or less) in the user's language. Every sentence
that states something read from code gets one `code_basis` entry
`{repo_root, path, line_start, line_end}` with the exact lines it rests on. Keep
3 to 8 entries per note, and never more than 100, the most one write accepts.

```
write_page(space="<space>", title="<note title>", content="<note>",
           code_basis=[{repo_root, path, line_start, line_end}, ...])
```

Whenever a tool offers a `space` field, pass the resolved Space in it; a write without it goes to the connection's default Space, which may not be this repository's. When the connection is locked to one Space the field is absent, and nothing needs passing.

Pass no `source_memory_ids`. If the reply says `code_basis is not available
over remote connections`, say map needs local Wenlan and stop. A `warnings`
entry saying a code basis was not recorded is reported, not hidden.

## 5. Refreshing a note

Call `read_page` for the note's `version` and full `code_basis` (only for notes you are refreshing). For each entry
in state `pending`, read those lines of the file now (the entry gives the path
and lines) and decide whether what the note says still holds.

- All entries still hold: call `page_code_basis` with `action: "confirm"` for
  each entry. No rewrite.
- Something changed: rewrite only the sentences that changed. Then call
  `write_page` with `page_id`, `expected_version` set to the `version` you
  read, the edited `content`, and a `code_basis` list that names every entry
  the rewrite relies on at its current lines. Naming a span again re-anchors
  it as `ok`; a span that moved replaces the stale entry it overlaps on the
  same file, so nothing is left `pending`.
- `unavailable` entry (the file or repository is gone): drop the sentence or
  re-point it, and call `page_code_basis` with `action: "remove"` for the old
  entry.

A `gated` result means the user edited that note: report it as staged for
review, not refreshed. A refusal saying the page moved means someone edited it
after your read: read it again and redo the edit once.

## 6. Report

One line per topic with the note title: written, refreshed, confirmed, staged
for review, or left as is. Then one line on seeing drift later: `/brief` names
how many notes rely on code that changed, and `$wenlan map` rechecks them.

## Safety

- Reads and writes of notes just happen; no approval is needed.
- Map never deletes a note and never edits the repository.
- Map runs no commands in the repository beyond `git rev-parse`,
  `git ls-files`, `git check-ignore` and `git log`.
