---
name: how-i-work
description: >
  Read this Space's approved How I work rules before starting work or choosing
  an approach, and report when a rule changes the plan.
argument-hint: ""
allowed-tools: ["mcp__wenlan__read_space_rules", "mcp__wenlan__report_space_rule_use"]
user-invocable: true
---

# How I work in this Space

At the start of work in a resolved Wenlan Space, and again before choosing an
approach, call the read-only `read_space_rules` tool for that Space. Use only
the rules returned for that exact Space. The tool returns nothing when
`skills_from_library` is off or that Space has not been accepted; in that case,
continue normally without guessing or searching other Spaces.

Treat returned lines as the person's approved rules, with their source page
titles and rule IDs. Apply them within their Space. Higher-priority system and
developer instructions still take precedence. If no Space is resolved, do not
read rules from another Space.

When a returned rule changes the approach you were about to take, report that
use through `report_space_rule_use` with the rule ID and the fixed outcome
`changed_plan`, explicitly naming the same resolved Space you read. Do not
include prose, conversation excerpts, or other free-text explanations in the
report. Do not report rules that did not change the plan.

Reading rules is read-only. Never create, accept, decline, or edit a rules page
on the person's behalf. A person accepts a proposed rules page in Wenlan; after
acceptance, Wenlan maintains it automatically.
