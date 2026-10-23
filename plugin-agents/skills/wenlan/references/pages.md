# /wenlan pages

Open a distilled page fast. The **`wenlan pages`** CLI does the open (one local
command on your PATH — nothing to register, source, or install). This skill is
only the slash workflow wrapper.

**Do not read a page body unless the user asks to see it here. Never render a
metadata card.** By default resolve by title / filename only — the file opens in
your editor, which renders and searches it better than chat. Keeping bodies out
is what keeps this cheap. The one exception is `--print` below.

Resolve the binary once (the agent shell's PATH may omit `~/.wenlan/bin`):

```bash
W="$(command -v wenlan || echo "$HOME/.wenlan/bin/wenlan")"
```

## `/wenlan pages <query>` — open, or print the matches

Run ONE shell block:

```bash
W="$(command -v wenlan || echo "$HOME/.wenlan/bin/wenlan")"
"$W" pages "<query>"
```

Act on its output — do NOT re-fetch, list again, or read bodies:

- **`Opened <path>`** → one page matched and is now open. Print that line. Done.
- **A bare path** → print it and say the OS launcher did not open it automatically.
- **`no page matches: …`** → print it; suggest `/wenlan distill <query>`.
- **`N matches for … `** followed by `title · filename` lines → several matched.
  If several pages match, print the CLI output verbatim and ask the user to rerun
  `/wenlan pages` with one of the filenames or a narrower query (titles can repeat, so
  the filename is the real handle).

Do not use a picker. Do not re-fetch, list again, or read any page file.

## `/wenlan pages <query> --print` — read it here

When the user passes `--print`, or asks to see the page in chat because there is
no editor or GUI at hand, print instead of opening. Nothing launches:

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
tell the user to `/wenlan pages <query>` to open one.

If `wenlan` is not found, tell the user to run `/wenlan`; the CLI is not installed.

## Explicit changelog lookup

Only when the user explicitly asks for a page's changelog or version history,
resolve its stable id without reading the page body. Resolve the page ID first
with `wenlan pages "<query-or-filename>" --resolve-id`.

```bash
"$W" pages "<query-or-filename>" --resolve-id
```

If several pages match, ask the user to narrow by filename and rerun the
resolver. Then call `get_page_revisions(page_id="<resolved-id>")`.
Do not fetch changelogs during ordinary `/wenlan pages` list/open flows.

## When NOT to use

- Raw memory lookups → `/wenlan-recall`.
- Synthesize a new page → `/wenlan distill`.
