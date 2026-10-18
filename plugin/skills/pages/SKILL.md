---
name: pages
description: >
  Browse, open, or print distilled pages, reusing the `wenlan pages` CLI for the
  actual open (one implementation on your PATH, no setup). `/wenlan pages <query>` opens
  the page when one matches, or shows a small native picker when 2-4 match.
  `/wenlan pages <query> --print` reads the page here instead of opening an editor.
  `/wenlan pages` lists recent pages. Invoked as `/wenlan pages [query] [--print]`.
argument-hint: "[query] [--print]"
allowed-tools: ["Bash", "AskUserQuestion", "mcp__plugin_wenlan_wenlan__get_page_revisions"]
user-invocable: false
---

# /wenlan pages

Open a distilled page fast. The **`wenlan pages`** CLI does the open (one local
command on your PATH — nothing to register, source, or install). This skill only
adds a small native picker for the ambiguous case.

**Do not read a page body unless the user asks to see it here. Never render a
metadata card.** By default resolve by title / filename only — the file opens in
your editor, which renders and searches it better than chat. Keeping bodies out
is what keeps this cheap. The one exception is `--print` below.

Resolve the binary once (the agent shell's PATH may omit `~/.wenlan/bin`):

```bash
W="$(command -v wenlan || echo "$HOME/.wenlan/bin/wenlan")"
```

## `/wenlan pages <query>` — open, or pick when ambiguous

Run ONE Bash block **with the command sandbox DISABLED** (opening calls the OS
launcher `open`/`xdg-open`, which a sandboxed shell can't reach, and would else
fail with `kLSUnknownErr`):

```bash
W="$(command -v wenlan || echo "$HOME/.wenlan/bin/wenlan")"
"$W" pages "<query>"
```

Act on its output — do NOT re-fetch, list again, or read bodies:

- **`Opened <path>`** → one page matched and is now open. Print that line. Done.
- **`no page matches: …`** → print it; suggest `/wenlan distill <query>`.
- **`N matches for … `** followed by `title · filename` lines → several matched:
  - **2-4 matches** → present them with `AskUserQuestion`, one option each:
    **label** = title, **description** = filename (titles can repeat, so the
    filename is the real handle). On pick, open it:

    ```bash
    "$W" pages "<picked-filename>"
    ```
    Print the `Opened` line it returns.
  - **more than 4 matches** → print the list verbatim and ask the user to narrow
    the query or pass one of the filenames. No picker — it can't show more than 4.

The picker stays small by construction: ≤4 short options, no page bodies, and
the open is a single CLI call that prints one line.

## `/wenlan pages <query> --print` — read it here

When the user passes `--print`, or asks to see the page in chat because there is
no editor or GUI at hand, print instead of opening. Sandbox can stay on; nothing
launches:

```bash
W="$(command -v wenlan || echo "$HOME/.wenlan/bin/wenlan")"
"$W" pages "<query>" --print
```

It prints the matched page's text followed by its sources. Show that output.
The `N matches` and `no page matches` cases work exactly as above; once one
filename is chosen, rerun with it and `--print`.

## `/wenlan pages` — list recent

```bash
W="$(command -v wenlan || echo "$HOME/.wenlan/bin/wenlan")"
"$W" --format table pages
```

Prints the newest ~20 titles (CLI-capped; `--limit 0` for all). Show them and
tell the user to `/wenlan pages <query>` to open one. No picker over the whole set —
that's the lossy, 4-cap case the CLI list avoids.

If `wenlan` is not found, tell the user to run `/wenlan`; the CLI is not installed.

## Explicit changelog lookup

Only when the user explicitly asks for a page's changelog or version history,
resolve its stable id without reading the page body. Resolve the page ID first
with `wenlan pages "<query-or-filename>" --resolve-id`.

```bash
"$W" pages "<query-or-filename>" --resolve-id
```

If several pages match, use the same picker/narrowing flow above and rerun with
the selected filename. Then call `get_page_revisions(page_id="<resolved-id>")`.
Do not fetch changelogs during ordinary `/wenlan pages` list/open flows.

## When NOT to use

- Raw memory lookups → `/recall`.
- Synthesize a new page → `/wenlan distill`.
