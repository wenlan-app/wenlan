---
description: On-demand harness retro. Harvest friction since the last sweep with retro-lib.sh, distill, propose promotions with retirements, re-validate old rules, batch allowlist candidates for one tap. The loop over loop.
---

# /harness-retro

Turn the friction since the last sweep into a better harness. Three stores, one job
each: Wenlan holds durable lessons (cross-tool), Claude auto-memory holds
CC-operational facts, CLAUDE.md/AGENTS.md hold the few always-obey rules. Re-derivable
signals are harvested fresh by `~/.claude/harness-retro/retro-lib.sh` and stored
nowhere else. The incidents behind every instrument and threshold are in
`~/.claude/harness-retro/harness-retro-why.md`; read an entry before changing that
instrument, not before running it.

**Cadence: on demand.** Attended = ask-and-apply. Background = every ask becomes a
staged proposal. The obey tier (CLAUDE.md, AGENTS.md, allowlist, settings) is
propose-then-ask, never auto-mutated. Exactly one auto-apply lane exists: §3c.

## 0. Open

1. Unlock check, first command, both branches exit 0:
   `if find ~/.claude/harness-edit-ok -mmin -480 2>/dev/null | grep -q .; then echo "unlock fresh"; else echo "unlock STALE"; fi`
   STALE: run the whole sweep anyway, but stage the digest, csv row, and heartbeat under
   `~/.wenlan/specs/shared-harness/retro-staged-<YYYY-MM-DD>/` with a copy-in command in
   the digest. Never write the heartbeat from a staged run. §3c and the Wenlan `distill`
   call keep their own standing authorization.
2. Window: `bash ~/.claude/harness-retro/retro-lib.sh window`. Open the digest with it
   in plain words ("sweep covers 2026-08-30 to today, 6.4 days, 20 sessions, 191
   workers"). A missing heartbeat falls back to 14 days; say so.
3. Self-check: `bash ~/.claude/harness-retro/retro-lib.test.sh` must end `PASS n/n`
   (sandbox off; it calls `mktemp -d`). A retro that harvests with dead instruments
   manufactures clean sweeps. If the write gate blocks an instrument, report
   `unchecked: <name>`, never a zero and never a rephrased command.

## 1. Harvest

Run from the project root, sandbox off (the hook suites need `mktemp -d`):
`bash ~/.claude/harness-retro/retro-lib.sh all > "$S/all.json"` where `$S` is the
staging dir. Each key is one instrument; read the JSON, then judge:

| key | RED when | what you add |
|---|---|---|
| `counters` | never on its own | cluster `tool_errors` by cause; the clusters are the friction, the count is not |
| `grumbles` | never on its own | cluster repeated corrections into "what keeps tripping me"; catches only the wrong you reacted to |
| `readability` | any hit | answer by placement (§3), never by rewording the same rule |
| `hooks` | any `reds` entry | a hook that cannot prove it still blocks is silent-wrong |
| `evasion` | any hit until a human clears it | read each file; gate-adjacent phrasing in a new memory or rule |
| `danger` | any A1 the owning main session did not surface | review every `a1_list` command (never sample); a heredoc consumed by `cat`/`tee` is prose, by an interpreter is acted; destructive MCP calls and `Write` over files the agent did not create are A1 too and are found by reading, not grep |
| `silent`, `narration` | never; diagnostic | report `silent_pct`, `longest_run`, and the narration row; a rise is a regression to name, not a target to chase |
| `reds`, `delivered` | delivery below 100% | report the raw pair and cap the rate at 100; name the gap |
| `shape` | never; diagnostic | report `block_pct`, `rewrite_miss_pct`, `prose_p90` as a trend; a block rate that does not fall after a length rule lands means that rule is inert and is a retirement candidate |
| `labour` | floor above ~120 min | the stop-loss: two consecutive over-budget sweeps mean the loop costs more than it pays, say so and propose the next cut |
| `obey` | `over_budget` true, or `unreceipted` > 0 | eviction is enforced before any add; each unreceipted line is a demotion candidate until a `promotions.log` record names its incident and check |
| `memory` | never | §3c candidates |
| `ledger` | any violator | list each with its fix; an unparseable promotion can never be scored |

Two things the library does not do: read `~/.claude/settings.local.json` for allowlist
rot (one-shot pids, tmp paths, dead repos, redundant entries), and add
`unchecked_records` to every count you quote (a record with no parseable timestamp must
never vanish silently).

Danger revert threshold, pre-agreed: two or more unsurfaced A1 in one sweep and the
digest must propose re-promoting a minimal always-on surfacing layer (one tap, never
auto). Report `Tier A a1=<n> a2=<m> (all reviewed) · Tier B <k> files`; a zero is real
only if workers existed and Tier A was reviewed in full.

## 2. Read durable lessons

Call the Wenlan `recall` MCP tool two or three times with specific queries (the gates,
files, and grumble topics this sweep touched), not one generic query. Capture this
sweep's top durable lesson back with `capture`, superseding the near-duplicate it
reports when there is one.

## 3. Distill: at most 3 promotions, each weighed against a retirement

Rank by severity times independence, not raw frequency; two clones of one repo are one
signal. The bar is one incident for a solo dev in one monorepo.

**Route by checkability first.** A candidate that is mechanically checkable (a path
resolves, a flag is documented, a message takes a required form) becomes an executable
check with its own positive control, never a CLAUDE.md line. Prose is the fallback for
judgment-only rules. Each sweep, also name the one existing bullet a hook or test could
enforce, draft the check, and propose deleting the prose: a check costs zero context.

**A rule that exists and keeps being broken is a placement problem, not a promotion.**
An output style binds harder than CLAUDE.md context. The active style is the built-in
one named by `outputStyle` in `~/.claude/settings.json` and `~/.claude/output-styles/`
is empty (checked 2026-09-05), so a placement diff proposes a custom style file there
rather than rewriting the rule. A rule broken twice
or more inside one window is the sweep's strongest signal and must leave as (a) a
placement diff, (b) an executable check proposal, or (c) an explicit `blocker:` line.

**Obey-tier admission.** Two mechanisms, both from the `obey` instrument:
- Byte budget (cost): the always-loaded set must stay at or under
  `RETRO_BYTES_BUDGET` (22591, the 2026-09-05 baseline). Over budget: demote before add.
- Admission by receipt (value): a bullet stays only if a `promotions.log` record's
  `rule:<file>:<phrase>` phrase appears in it. Re-linking is one record per bullet,
  never a bulk line.
Retirement demotes to Wenlan (searchable), never deletes. Never force a deletion to
make room; mechanical net-zero can evict a rare, load-bearing safeguard.

**Source discipline for promotions.** A rule lifted from documentation quotes a normative sentence
(must, never, always), never an example; the quote goes in the record's `<source>`.

| Patch | Action |
|---|---|
| Executable check (tooth, hook, test, with positive control) | draft, show diff, ask |
| Allowlist add | one-tap batch in the digest, never auto-apply |
| CLAUDE.md/AGENTS.md add (plus retirement if one applies) | show diffs, ask |
| Output-style copy | show diff, ask |
| Dispatcher or routing tweak | one line |

Allowlist eligibility, used only to filter what is worth proposing: an MCP tool whose
name contains `read`/`get`/`list`/`search`/`view`/`recall`, or an exact read-only Bash
command. Never a `*` wildcard on an interpreter, shell, package runner, `gh api`, or any
arbitrary-exec verb. Applied entries are logged to
`~/.claude/harness-retro/allowlist-applied.log` (reversible).

## 3b. Re-validate

Spot-check a few CLAUDE.md/AGENTS.md rules against current tools and models; one that
names a flag, file, or skill that no longer exists is a retirement candidate. Do the
same for this file and `retro-lib.sh`: a path or command that no longer exists gets
fixed before it is run.

## 3c. Auto-memory distill (the one auto-apply lane)

Scope: `~/.claude/projects/*/memory/` topic files only, never CLAUDE.md, settings, or
allowlists. Candidates come from the `memory` instrument plus files whose only content
is a one-time action (verify completion against the live artifact first).
1. Archive first: byte-exact copy to a sibling `memory-archive-YYYYMMDD/` next to the
   memory dir, sandbox off, proved with `cmp`. No archive, no rewrite.
2. Rewrite to the settled verdict with the Write tool: final state plus at most one
   "corrected from" line. Verify every path, flag, and crate the memory names; an
   unverifiable claim leaves the file untouched and flagged.
3. Delete only a verified-complete action that carries no durable rule.
4. Update the MEMORY.md index line. Digest one line per action: file, bytes before and
   after, archive path. Zero candidates: one "memory clean" line.

## 4. Close

- Call the Wenlan `distill` MCP tool.
- Promotions and retirements append to `~/.claude/harness-retro/promotions.log`:
  `YYYY-MM-DD | <slug> | promoted|retired | <source> | hook:<name>` or
  `| rule:<file>:<phrase>`. A hook survives while wired and its suite passes; a rule
  survives while the phrase is present in the file.
- Csv row: `bash ~/.claude/harness-retro/retro-lib.sh csvrow <retro_min>` prints the
  line; append it with `>>` to `~/.claude/harness-retro/instruments-v2.csv` (`NA` for a
  missing number, never blank). Slopes are read from that file, never from memory; no trend claim under two
  rows. 90-day survival stays NA until 2026-10-10.
- Queue with teeth: carry `queued-for-you` items forward with a count. Second digest:
  `⚠ QUEUED 2×` at the top. Third: convert to a proposal or propose dropping it.
- Digest, one screen: window, the harvest table's REDs and clusters, allowlist batch,
  promote/retire, re-validate flags, the instrument row, queue. Quiet sweep: one line.
  End with `PROMOTIONS-PROPOSED: <n>`.
- Heartbeat last, and only after a real digest (at least one item harvested, or an
  explicit clean-sweep line): `date -u +%FT%TZ > ~/.claude/harness-retro/last-retro.txt`.
  It also defines the next window, so an early write shrinks the next harvest.

No transcript dumps.
