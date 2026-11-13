# CI flake policy

Use this response policy when a required CI check fails intermittently. A red
job is not automatically a flake, and a green rerun is not automatically a fix.

## Automatic retry in CI

A flaky test must not drop a green PR from the merge queue. In CI only, every
Rust test (nextest `ci` profile, `.config/nextest.toml`, selected by
`NEXTEST_PROFILE=ci` in `ci.yml`) and every Playwright test (`retries` in both
Playwright configs) that fails runs once more:

- Passes on the retry: the run stays green and the test is reported as flaky.
  nextest prints `FLAKY`, and `scripts/ci_test_plan.py` turns each one into a
  "Flaky test (passed on retry)" warning annotation and a job-summary line.
  Playwright lists it as flaky in its summary annotation.
- Fails both tries: the run fails, exactly as before.

The retry hides nothing: a flaky report is a defect and gets the steps below
(two matching reports require an owner and a fix). Local runs do not retry.
Network setup steps (sccache, cargo-nextest, Playwright browsers, vcpkg
sqlite3) also retry with a time limit; that is not a test retry.

1. **Name the exact failure from the full job log.** Record the job, command,
   test, assertion or timeout, runner OS, and run URL. Check-run annotations such
   as "exit code 1" are not enough. First separate test failures from setup,
   infrastructure, and CI time-budget failures.
2. **Use at most one rerun for diagnosis.** Rerun only when the first log points
   to timing or runner state. Compare both signatures. A pass strengthens the
   flake hypothesis but does not close the defect; repeated failure points to a
   deterministic problem and should be debugged without more reruns.
3. **Two matching occurrences require an owner and a fix.** Two independent
   occurrences of the same test and symptom, on any branches or runners, are
   enough to prioritize deflaking. Record both occurrences in the fixing PR (or,
   if no one is fixing it yet, in the task's handoff); agents do not open a
   tracking issue unless the maintainer asks. Prefer event-driven
   waits (`wait_until_reached`, as used in
   `crates/wenlan-core/src/lint/runner.rs`) or poll-with-deadline (as
   used in `crates/wenlan-cli/src/commands/service.rs`). Do not use bare sleeps
   or single-shot assertions on asynchronous or process state.
4. **Quarantine must fail closed.** Do not add `#[ignore]` or exclude a test from
   its owning plan unless the same PR first wires that exact test into an
   explicit, visible, non-blocking main-push or scheduled job. Ask the maintainer
   for a tracking issue with an owner, removal condition, and deadline. Without that replacement
   execution path, fix the test or leave the required check red; never disable
   or reroute an entire owning CI lane to hide one flaky test. The existing
   [`main-canary.yml`](../.github/workflows/main-canary.yml) runs selected eval
   tests only and is not a general quarantine lane.
5. **Do not revert an innocent trigger.** Cold caches, rotated cache keys, or a
   busy runner can expose an existing race. Revert only when the changed code or
   CI route caused the failure; otherwise fix or quarantine the failing test.
   PR #401 exposed two existing timing races after a toolchain cache rotation;
   PR #403 fixed the tests instead of reverting the trigger.

## Multiple merge-ready PRs

`main` uses GitHub's
[merge queue](https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/configuring-pull-request-merges/managing-a-merge-queue).
Add each ready PR with **Merge when ready**, or `gh pr merge <number>`. The
queue rebuilds the PR on top of `main` plus every PR ahead of it, runs `ci.yml`
on that exact commit (a `merge_group` event, planned like a PR from the files
that differ between the queue base and that commit, which can include PRs
queued ahead of it), and lands it only when the required `conclusion` check
passes. Each PR still lands as its own commit with its own `main` push run. A PR
that fails leaves the queue, and the PRs behind it are rebuilt without it, so no
PR needs a manual update and rerun after another one lands. Use stacked PR bases
only for real code dependencies, and never rerun a stale SHA.

Queue the release PR by itself, last, after release PR maintenance has synced
it with `main` and its PR CI and observer receipt have passed on that final
head. Its release receipt binds its exact head tree, so `detect-changes`
fails any queue entry for it whose tree differs; GitHub then drops it from the
queue before anything lands.

Every workflow that reports a check required on `main` must keep its
`merge_group` trigger. A required check that never reports leaves the queue
entry waiting until the queue's status-check timeout.
