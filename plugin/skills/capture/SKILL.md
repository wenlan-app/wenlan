---
name: capture
description: >
  Save a memory to Wenlan in flow, or save something from this conversation as
  one note when the user asks for a note. Use proactively when the user states
  a preference, makes a decision, corrects you, or shares a durable fact.
  Invoked as `/capture <content>`.
argument-hint: "<content>"
allowed-tools: ["mcp__plugin_wenlan_wenlan__capture", "mcp__plugin_wenlan_wenlan__save_source", "mcp__plugin_wenlan_wenlan__recall", "mcp__plugin_wenlan_wenlan__create_entity", "mcp__plugin_wenlan_wenlan__create_relation", "mcp__plugin_wenlan_wenlan__list_rejections", "mcp__plugin_wenlan_wenlan__list_pages", "mcp__plugin_wenlan_wenlan__read_page", "mcp__plugin_wenlan_wenlan__write_page", "Bash"]
---

# /capture

Capture a single memory in the moment. Active verb: agent captures the
moment of insight, like a photograph.

When the session start printed a `Wenlan session:` line, pass its value as
`session` on `capture` and `write_page`.

## Resolve the active space

The `/capture` skill accepts one optional inline token of the form
`space:<name>` anywhere in the argument string. The bundled resolver strips it
and returns the rest as the content. It prints the resolved Space line itself:

```bash
raw_args="<the full argument string passed to /capture>"
resolved="$("${CLAUDE_PLUGIN_ROOT}/scripts/resolve-space.sh" --cwd "$PWD" --args "$raw_args")"
space="$(printf '%s\n' "$resolved" | cut -f1)"
source_layer="$(printf '%s\n' "$resolved" | cut -f2)"
content="$(printf '%s\n' "$resolved" | cut -f3-)"
```

If `content` is empty, ask the user what they want to capture before calling
the tool. Pass `space="$space"` to the `capture` MCP tool only when `space` is
non-empty.

The resolver line reports the proposed client context. After capture, relay
the tool's returned `space`, `space_source`, and `write_outcome`; those fields
describe what the daemon actually persisted and are authoritative.

Unknown spaces are not auto-created. Register one through the resolved CLI
binary, or omit `space` to store uncategorized:

```bash
W="$(command -v wenlan || echo "$HOME/.wenlan/bin/wenlan")"
"$W" spaces add <space>
```

## Memory or note

Default to one memory. Save a note instead only when the user asks for one in
their own words: "save this as a note", "write this up", "存成筆記",
"寫成一篇筆記", or `/capture` text that starts with "note:". Proactive
captures are always memories; never create a note on your own initiative, and
never save the same thing as both a memory and a note in one turn. Turning many
memories into notes is a different job: that is `/wenlan distill`.

To save a note, follow "Save a note" below and skip the memory steps.

## Something from another app or the web

When the user wants to keep something you read in another app or on the web
(an email, a Notion or Drive page, a Slack thread, an issue, an article), save
the original with `save_source` instead of capturing it. Pass its permanent
link as `url`, its title, the text as you read it (not a summary), a
`source_label` such as `gmail`, `notion`, `drive`, `slack`, `github` or `web`,
and `space` when it is non-empty. Never make up a link: if you have no link to
the original, capture the insight as a memory instead. Save only what the user
asked to keep; never copy a whole inbox or channel.

The saved copy does not update on its own. When the original changes and the
user wants Wenlan to follow, save it again with the same link; notes built on
it are then marked for refresh. Report the link and the Space the result names.

## How to invoke

Call the `wenlan` MCP server's `capture` tool with the user's content as a
complete, self-contained statement. Attach `topic` from cwd or the
conversation — don't make the user type it.

```
capture(content="<args, written as a full sentence with WHY>",
        memory_type="<picked from the 6 types>",
        entity="<primary entity name, if any>",
        space=<resolved if non-empty>)
```

### `memory_type` — agent picks one of 6

The daemon classifies when a local model or API key is configured. In
local memory mode it does not, so the agent picks the type from the content itself. Use this
mapping:

| Type | Use for |
|---|---|
| `identity` | Durable facts about the user (role, company, language preference) |
| `preference` | "I prefer X because Y" — a habit, a correction, a stylistic choice |
| `decision` | "Going with A over B because C" — a specific choice with rationale |
| `lesson` | Root cause found, workaround discovered, technical insight earned |
| `gotcha` | Sharp edge, surprising behavior, a thing to watch out for |
| `fact` | Durable info about people, projects, tools — anchor to `entity` when possible |

If two types fit, pick the one closest to *why the memory matters*. A
decision *also* implies a preference, but `decision` is more specific.

### `entity` — extract the anchor

Pick the single most important named thing in the content: a person,
project, tool, place. Use the exact name. Example: "Alice prefers TDD
because…" → `entity="Alice"`. If the content has no named anchor,
omit `entity`.

### Space inference

- cwd inside a repo → repo name (e.g. `~/Repos/wenlan/...` → `"wenlan"`).
- Outside configured or registered repo context → omit the client Space and
  let the daemon Default decide the write.
- Pass `space` only when scope is known. If uncertain, omit it or inspect the
  registered spaces first:

```bash
W="$(command -v wenlan || echo "$HOME/.wenlan/bin/wenlan")"
"$W" spaces list
```

### Multiple entities or relations

Ordinary captures stop after `capture`: pass the single most important named
anchor through `capture.entity` and let daemon enrichment handle routine
extraction. Do not call `create_entity` for ordinary captures. Never infer a
relation the user did not state.

Use the explicit KG tools only when the user explicitly states a durable relation:

1. Call `create_entity` for both named endpoints first and collect their stable
   ids. This is idempotent and may return an existing id.
2. Call `capture` with the complete relation statement and pass the primary
   entity name as `entity` so the memory resolves and links to it.
3. Call `create_relation` with `from_entity_id`, `to_entity_id`,
   `relation_type`, and the capture result's required `source_memory_id`.

For a durable named entity explicitly established by the user, `create_entity`
may also be used alone when its stable id is needed. Its `Entity <id> ready`
result does not imply that a new row was created.

## Save a note

1. Resolve the Space exactly as above.
2. New note: call `write_page` with a short noun-phrase `title`, the note as
   Markdown `content` written from this conversation, and `space` when it is
   non-empty. `source_memory_ids` are optional; pass them only when the note
   is built from memories you can name by id.
3. Updating a note the user named: confirm the exact title with `list_pages`
   for the Space (never fuzzy-match), call `read_page`, keep `page.version`
   and `page.source_memory_ids`, edit the current body, then call
   `write_page` with `page_id`, the edited `content`, `expected_version` set
   to that version, and the existing `source_memory_ids`.
4. A refusal saying the page moved means it changed after your read: read it
   again and redo the edit once. A refusal that the user wrote or edited the
   page, or a `gated` result, means it is protected: stop and tell the user it
   is waiting for their review, not that it was saved.
5. Report the note's title and the path or id the result returns.

## What to capture, what to skip, how to write it

The `capture` tool description is the single source for this policy: what to
store proactively, what never to store (tool output, agent operating rules,
transient state, world facts), one atomic idea per call, and declarative
wording. Follow it as written; do not keep a second copy here.

## When to use

- User explicitly says "remember this", "save that", "capture this".
- User asks to save something from this conversation as a note.
- User asks to keep an email, page, thread, or article you read elsewhere:
  `save_source`, as above.
- User states a durable preference / decision / correction proactively (no
  ask required — that's the floor, not the trigger).

## When NOT to use

- End of session bulk store → use `/handoff` (multi-item batch).
- Pulling memories back out → use `/recall`.

If a capture the user expected is missing or the user explicitly asks why it
was rejected, call `list_rejections` to diagnose the quality gate. Do not list
rejections after successful ordinary captures.

## Read the capture receipt

The daemon does not resolve contradictions at capture time. The receipt
carries three signals worth acting on; everything else is informational.

### `gated: true` (write staged for human review)

The result opens with `Staged <id> for review. Not live yet.` This happens only
when the call carried `supersedes` and this agent's trust level is below full.
The memory it would replace keeps answering searches. Tell the user in one line
that the correction is waiting in the review queue (`/brief` shows the top
three; `/wenlan curate revisions` walks the full queue). Do not accept or dismiss the
staged revision from this skill: the gate exists because a person downgraded
this agent, so the decision is theirs.

### `near_duplicate: <id> (<similarity>)`

The daemon stored the memory anyway and names the closest existing one. If
the new capture restates that memory, capture again with
`supersedes=<id>` so the pair collapses with history preserved, or leave both
when they are genuinely distinct facts. Do not call `forget` on either without
the user's say-so.

### `Warnings:`

Schema-validation notes on `structured_fields`. Fix the fields and capture
again only when a warning says a field was dropped.

A receipt with none of these stored cleanly; report `Stored <id>` and move on.
