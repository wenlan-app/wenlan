## What this PR does
<!-- 1-2 sentences -->

## How to test
<!-- Steps to verify the change works -->

## Overlaps checked
<!-- PRs merged since you branched that touched the same files:
git log --oneline $(git merge-base origin/main HEAD)..origin/main -- $(git diff --name-only origin/main...HEAD)
For each one, say in a line whether its intent still holds after your change. "None" is fine. -->

## Checklist
- [ ] Tests pass (`cargo test --workspace`)
- [ ] `cargo clippy` and `cargo fmt` pass
- [ ] New files use the appropriate SPDX header for their package, even if nearby legacy files have not been normalized yet
- [ ] No secrets or credentials committed
