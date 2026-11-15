---
name: wenlan-how-i-work
description: "Read the person's approved How I work rules (this Space's and the general ones) before starting work or choosing an approach, and report when a rule changes the plan."
---

# How I work in this Space

At the start of work in a resolved Wenlan Space, and again before choosing an
approach, call the read-only `read_space_rules` tool for that Space. Use only
the rules returned for that Space: its own rules plus the person's general
rules, which hold in every Space. When a general rule and this Space's own
rule disagree, follow the Space's rule. The tool returns nothing when
`skills_from_library` is off or nothing has been accepted; in that case,
continue normally without guessing or searching other Spaces. If the call
fails or Wenlan cannot be reached, tell the person the rules could not be
read; a failed read is not the same as having no rules.

Treat returned lines as the person's approved rules, with their source page
titles and rule IDs. Apply them within their Space. Higher-priority system and
developer instructions still take precedence, and so does what the person asks
for in this conversation: a rule is how they usually work, not an order
against what they are telling you now. If no Space is resolved, call
`read_space_rules` without a Space: it returns only the general rules. Never
read another Space's own rules.

When a returned rule changes the approach you were about to take, report that
use through `report_space_rule_use` with the rule ID and the fixed outcome
`changed_plan`, explicitly naming the same Space you read (none when you read
without a Space). Do not
include prose, conversation excerpts, or other free-text explanations in the
report. Do not report rules that did not change the plan.

Reading rules is read-only. Never create, accept, decline, or edit a rules page
on the person's behalf. A person accepts a proposed rules page in Wenlan; after
acceptance, Wenlan maintains it automatically.
