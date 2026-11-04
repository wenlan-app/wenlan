# Connection and starting status

When the argument is empty, report status. Read only; change nothing. Run
the three checks in order; only a failed connection check stops early.

1. Project: resolve it.

   ```bash
   resolved="$("${CLAUDE_PLUGIN_ROOT}/scripts/resolve-space.sh" --cwd "$PWD" --args "")"
   space="$(printf '%s\n' "$resolved" | cut -f1)"
   ```

2. Connection: call `brief` with `space` when non-empty. Success means Wenlan
   is running and this AI tool reaches it; there is nothing else to check.
   - It says the Space is not resolved: the connection works, and this folder
     is not linked to a project yet. Do not guess one.
   - Any other answer from Wenlan, including an error about the Space or the
     request: the connection still works. Say Wenlan is connected but could
     not find this folder's project, and do not run setup.
   - Only when Wenlan cannot be reached at all (the call fails to connect or
     the tool is missing): say Wenlan is not running or this AI tool cannot
     reach it, and run the **setup** workflow (step 3). Setup asks before it
     installs anything; do not skip its consent.
3. Waiting items: call `list_pending_revisions(limit=10)`. If exactly 10 come
   back, say "at least 10".

**New user.** When `brief` ends with a "Getting started" line, the person is
new to Wenlan: answer with that instead of the usual status. In one or two
plain sentences say that Wenlan is connected, how far they are (pages so far,
items waiting), and the one next step it names, as the exact thing to type or
do, in the person's language. Leave out the project sentence and the
waiting-changes count, and never say "steep", "distill" or other internal
words: name writing pages in plain words. For example: "Wenlan is connected.
12 of your notes are waiting to become pages; type `/wenlan write my pages`
and I'll write them." (Chinese: `/wenlan 整理成筆記`) or "Wenlan is connected
and you have 8 pages. Ask me anything about them, like `/wenlan what do my
pages say about the trip?`". Use a real page title only when you already
know one. Do not ask them to link a project. Then stop; no second list of
examples.

Otherwise, answer in one to three short, plain sentences, the way you
would tell a person: that Wenlan is connected, which project this folder belongs to (or
that it is not linked to one yet), and how many changes wait for their OK
(leave this out when there are none). For example: "Wenlan is connected.
This folder is not linked to a project yet." No label
lines such as "Runtime:" or "Project:", nothing about a check that was
skipped or could not run, and no internal words such as runtime, daemon,
Space, MCP, tool names or HTTP status codes. Then give two concrete examples of what to type
next, one of them about notes or waiting items, for example
`/wenlan open my note on the release plan` and `/wenlan review what's
waiting`. Do not print the Brief body here; that is `/brief`.

Import progress only when the user asks about an import: call
`list_pending_imports`. Never call it during plain status.

