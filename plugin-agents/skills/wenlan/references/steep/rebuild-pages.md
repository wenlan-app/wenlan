# Rebuild a page or review a specific merge proposal

Load this for legacy `distill <page>` requests routed here, or when the user
explicitly asks to regenerate a particular page
from its current supporting memories, or explicitly asks to review a page
merge proposal. A rebuild replaces generated prose. State that before
proceeding and obtain the user's explicit confirmation for the target page.
Ordinary capture, refresh, or reorganization does not need this destructive
override.

Resolve the page by its exact page ID, read it and its current sources, and
confirm the target is a generated page eligible for rebuild. If its identity
or ownership is unclear, stop and report the ambiguity. Then call the
`distill` tool with `target=<page_id>, force=true`; the target must resolve to
one page. Do not use the force flag for a whole Space, entity, or global pass.

The daemon needs an available configured LLM. If it reports the provider
unavailable, stop without substituting direct database or file edits. The
force path regenerates AI-written prose from current evidence. User-authored
lines remain protected: when the page holds any sentence the person wrote or
edited, the rebuild leaves the page as it is and waits as one revision card
that the person accepts in the Wenlan app. Tell them that is where it waits. A provider failure, discarded output, or citation
failure must leave existing content and review protection intact. Read the
page back and inspect its revision after the call; report the exact outcome.

Do not run global distillation or clustering as a default step. New material
belongs to [new-material.md](new-material.md), and source-driven maintenance
belongs to [refresh-pages.md](refresh-pages.md). If the request describes
combining two pages, do not directly merge or delete either page. Page merges
are proposed as review refinements. Do not poll that queue during ordinary
steep work. When the user explicitly asks to review a specific proposal,
follow the curate refinement review instructions. Only the person accepts a
proposal, in the Wenlan app; tell them it waits there. Reject only after the
user gives a clear item-level decision for the displayed proposal; pass its
exact ID to `reject_refinement`. Never use generic acceptance to apply a
`lint_repair_review` proposal; route that through the protected lint repair
workflow.
