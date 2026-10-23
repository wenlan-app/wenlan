---
name: wiki
description: >
  Read this repository and keep a wiki about it in its Space, sized to the
  repository: an overview that lists every page, then one page per part or
  flow a newcomer needs, each key sentence tied to the code lines it was read
  from, so Wenlan can tell when the code moved on. Writes the pages that are
  missing and leaves the rest alone; a page whose code changed is brought up
  to date by steep, not here. Use for "build a
  wiki for this repo", "get to know this repository", "幫這個倉庫建 wiki",
  "認識這個倉庫". Invoked as `/wenlan wiki [space:<name>]`.
argument-hint: "[space:<name>]"
allowed-tools: ["Bash", "Read", "Glob", "Grep", "mcp__plugin_wenlan_wenlan__list_pages", "mcp__plugin_wenlan_wenlan__read_page", "mcp__plugin_wenlan_wenlan__write_page", "mcp__plugin_wenlan_wenlan__page_code_basis", "mcp__plugin_wenlan_wenlan__recall"]
user-invocable: false
---

# /wenlan wiki

Read this repository and keep a wiki about it in its Space. How many pages it
has depends on the repository: a small tool may need five, a large monorepo
twenty or more. The overview page lists them all.
Each key sentence is tied to the code lines it was read from (a code basis), so
Wenlan can tell when the code moved on. The pages are written by the AI. A
rerun writes only the pages that are missing. A page whose cited code changed
is brought up to date by steep (`/wenlan steep`, or Wenlan's automatic mode),
not here: wiki writes new pages, steep keeps them current. Reply in the user's
language.

## 1. Resolve repository and Space

The skill accepts one optional inline token of the form `space:<name>`. The
bundled resolver strips it and prints the resolved Space line itself:

```bash
raw_args="<the full argument string passed to /wenlan wiki>"
resolved="$("${CLAUDE_PLUGIN_ROOT}/scripts/resolve-space.sh" --cwd "$PWD" --args "$raw_args")"
space="$(printf '%s\n' "$resolved" | cut -f1)"
repo_root="$(git -C "$PWD" rev-parse --show-toplevel)"
```

If the resolver cannot run (a background run may allow only read-only
commands), take the Space from the `space:<name>` token instead.

If the `git rev-parse` fails, say this folder is not a git repository and stop.
If `space` is empty, say this folder is not matched to a Space and that
`/wenlan wiki space:<name>` names one, then stop. Do not ask a question.

## 2. Plan the pages

Call `list_pages` with `space="<space>"` once.

Find the overview page by exact title (below). If it exists, call `read_page`
on it: its `## Pages` section (`## 頁面` in Chinese) is the plan. Then look
over the repository once more for an important part or flow the list misses,
such as a new package or a new entry point, and add it to the plan.

If there is no overview, plan from the repository. Look before you plan:
`git ls-files` (count and top-level layout), the README and docs folder, the
build manifests (`package.json`, `Cargo.toml`, `pyproject.toml`, `go.mod` and
the like), the entry points, CI config, and `git log --oneline -50`.

Plan the smallest set of pages that lets a newcomer work in this repository.
Organise by parts and flows, not by copying the directory tree.

Always:

| Page | What it answers | English title | 繁體中文 title |
|---|---|---|---|
| Overview | What the project is, where the entry points are, and the list of pages | `What <project> is` | `<project>是什麼` |
| How to run it | Install, build, start | `How to run it` | `怎麼跑起來` |
| Known pitfalls | From git log, comments and existing docs | `Known pitfalls` | `已知的坑` |

As the repository needs (each only when there is real code behind it):

- **Architecture or module map**: how the parts fit and what each one owns
  (`Module map`, `模組地圖`).
- **One page per major part**: a package, crate, service or directory that
  owns real behaviour, titled by what it does.
- **One page per important flow**: from entry to effect, such as
  `How a request is handled`; for a single main path, `Main flow` / `主流程`.
- **Data and storage**, **configuration**, **release and deploy**,
  **integrations with outside services**.
- **How tests run** (`測試怎麼跑`) when the repository has tests: the layers,
  and which are slow.

Let the repository decide the count. As a rough guide, a small repository
(under about 50 tracked source files) needs 4 to 6 pages, a medium one 8 to
15, a large monorepo 15 to 30. Never plan more than 30. Do not make a page for
a folder that only holds glue, and do not split a topic that fits on one
screen.

Titles are short noun phrases in the user's language, taken from how they typed
the request. When a page already in the Space covers a planned topic (for
example an earlier wiki's `Module map`), use that page's exact title in the
plan rather than a new one.

## 3. Decide per page: write or leave

For each page in the plan, find it in the `list_pages` result by exact title,
never fuzzy.

- Missing: write it (step 4).
- Present, and listed with a `code_basis` entry in state `pending` whose
  `repo_root` is this repository: leave it. It is already on steep's list;
  say in the report that it waits for steep.
- Present, and listed with a `code_basis` entry in state `unavailable` whose
  `repo_root` is this repository: the file or repository it cited is gone.
  Re-point it (step 5).
- Present with nothing outstanding: leave it, and say so in the report.
  `list_pages` rechecks the cited files first and lists only code bases that
  are not `ok`, so a page with none listed is current.

Do not call `read_page` on a page you are leaving as is.

Write at most 12 new pages in one run, so a first run on a large repository
stays affordable. Write the overview first, then the rest in plan order, most
important first. The pages left over are written by the next run.

**The overview's page list.** A new overview ends with a `## Pages` section
(`## 頁面` in Chinese) listing every planned page, written or not, as
`- [[<title>]]: <what it answers, one line>`. When the plan gained pages and
the overview already exists, update only that section: take `version` from
your `read_page`, then call `write_page` with `page_id`, `expected_version`,
`minor: true`, and `content` that is the page you read with only the list
changed. Pass no `code_basis`; the page keeps the bases it has. Skip this when
the overview has a `pending` or `unavailable` code basis (steep or step 5
handles it first), and say so in the report.

## 4. Writing a page

Read the repository with Read, Glob, Grep and `git ls-files`; for a file too
large to read whole, read the part you need. Never read a file
`git check-ignore` reports, a `.env` or other secrets, or anything outside
`repo_root`. Do not run build, test or install commands, and write nothing into
the repository.

Compose the page in the user's language: about a screen, up to two for a large
part. Link other pages in the plan as `[[<title>]]` where a reader would want
to go next. Every sentence that states something read from code gets one
`code_basis` entry `{repo_root, path, line_start, line_end}` with the exact
lines it rests on. Keep 3 to 12 entries per page (up to 20 for a page about a
large part), and never more than 100, the most one write accepts. A refusal
naming a span outside the file means the line numbers are off: read those lines
again and retry once with the right span.

```
write_page(space="<space>", title="<page title>", content="<page>",
           code_basis=[{repo_root, path, line_start, line_end}, ...])
```

Whenever a tool offers a `space` field, pass the resolved Space in it; a write without it goes to the connection's default Space, which may not be this repository's. When the connection is locked to one Space the field is absent, and nothing needs passing.

Pass no `source_memory_ids`. If the reply says `code_basis is not available
over remote connections`, say wiki needs local Wenlan and stop. A `warnings`
entry saying a code basis was not recorded is reported, not hidden.

## 5. Re-pointing a page whose code is gone

Only for a page with an `unavailable` entry. Call `read_page` for its
`version` and full `code_basis`. For each `unavailable` entry, find where that
fact now lives in the repository, or drop the sentence if it is no longer
true. Then call `write_page` with `page_id`, `expected_version` set to the
`version` you read, the edited `content`, and a `code_basis` list naming the
new lines; call `page_code_basis` with `action: "remove"` for each old entry.
Leave `pending` entries on that page alone: steep handles them.

A `gated` result means the user edited that page: report it as staged for
review. A refusal saying the page moved means someone edited it after your
read: read it again and redo the edit once.

## 6. Report

One line per planned page with its title: written, left as is, waiting for
steep, re-pointed, or staged for review. When pages are left over for the next
run, say how many and that `/wenlan wiki` again writes them. Then one line on
what happens later: `/brief` names how many pages rely on code that changed,
and `/wenlan steep` brings them up to date.

## 7. Put the wiki in the repository

Wenlan can write the pages that rest on this repository's code into a folder of
the repository and offer them as a pull request. Skip this step when
`WENLAN_AGENT_NAME` is `wenlan-daily`: that is Wenlan's daily run, and Wenlan
publishes after it. Otherwise check whether the Space is bound:

```bash
W="$(command -v wenlan || echo "$HOME/.wenlan/bin/wenlan")"
"$W" wiki status --space "$space" --format json
```

- It prints a binding: run `"$W" wiki publish --space "$space"` and add its
  line to the report (pull request opened or updated, already up to date, or
  a link to open the pull request by hand).
- It prints `null`: ask once whether to put the pages in this repository's
  `wiki/` folder and open a pull request. On yes, run
  `"$W" wiki bind --space "$space" --repo "$repo_root"`, then `wiki publish`
  as above, and mention that `wiki bind --daily <hour>` keeps the wiki up to
  date every day with the user's own AI tool. On no, leave it.
- The command is missing or has no `wiki` subcommand: skip this step without
  saying so.

Pages that cite the user's notes or chats never go into the repository; the
publish line says how many stayed out.

## Safety

- Reads and writes of pages just happen; no approval is needed.
- Wiki never deletes a page and never edits the repository's files itself.
  Step 7 asks Wenlan to write the wiki folder on its own `wenlan/wiki` branch,
  apart from the working copy, and open a pull request; only with the user's
  yes or an existing binding.
- Wiki only reads the repository. Shell commands are read-only ones such as
  `git rev-parse`, `git ls-files`, `git check-ignore`, `git log`, `git show`,
  `ls`, `wc`, `grep` and `sed -n`, plus the `wenlan wiki` commands of step 7;
  nothing that builds, installs, or changes a file.
