#!/usr/bin/env python3
"""Retry a CI run that failed only because GitHub never gave a job a runner.

Hosted runner pools (macOS arm64 most often) can be short. A queued job that
no runner picks up is cancelled after about 15 minutes with "The job was not
acquired by Runner of type hosted", and the run fails although no code ran.
`.github/workflows/ci-runner-retry.yml` calls this after every finished CI run:

- PR and main-push runs: re-run the failed jobs, at most twice per run.
- Merge-queue runs: the queue already dropped the entry, so put the PR back
  in the queue, at most twice per PR in six hours. Not the Release PR, which
  is queued alone by the release flow, and not a PR whose head moved after
  the dropped run started.

A run is retried only when every failed job is starved: no runner, no steps,
and either the "not acquired" annotation or at least ten minutes in the queue.
Any job that ran and failed blocks the retry. A cancelled job that never got
a runner, or that was cancelled after the first starved job finished, is the
cascade (`merge-group-fail-fast`, matrix fail-fast), not a failure of its own.
A retry re-runs the jobs for real, so it can never turn a failure green.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from datetime import datetime, timedelta, timezone

MAX_RETRIES = 2
STARVED_MIN_WAIT = timedelta(minutes=10)
QUEUE_WINDOW = timedelta(hours=6)
NOT_ACQUIRED = re.compile(r"not acquired by runner", re.IGNORECASE)
QUEUE_BRANCH = re.compile(r"^(?:refs/heads/)?gh-readonly-queue/[^/]+/pr-([0-9]+)-[0-9a-f]{40}$")
# Aggregates and watchers fail or cancel because another job did.
AGGREGATE_JOBS = frozenset({"conclusion", "merge-group-fail-fast"})
OK_CONCLUSIONS = frozenset({"success", "skipped", "neutral"})
RELEASE_BRANCH_PREFIX = "release-please--branches--"


def _time(value: str | None) -> datetime | None:
    if not value:
        return None
    return datetime.fromisoformat(value.replace("Z", "+00:00"))


def is_starved(job: dict, annotations: list[str]) -> bool:
    if job.get("conclusion") not in ("cancelled", "failure"):
        return False
    if job.get("runner_name") or job.get("steps"):
        return False
    if any(NOT_ACQUIRED.search(message) for message in annotations):
        return True
    started, completed = _time(job.get("started_at")), _time(job.get("completed_at"))
    return started is not None and completed is not None and completed - started >= STARVED_MIN_WAIT


def classify(jobs: list[dict], annotations: dict[int, list[str]]) -> tuple[bool, str]:
    """Return (retry, reason) for one run attempt's jobs."""

    starved = [job for job in jobs if is_starved(job, annotations.get(job.get("id"), []))]
    if not starved:
        return False, "no job was starved of a runner"
    first_starved = min(_time(job["completed_at"]) for job in starved)
    starved_ids = {job.get("id") for job in starved}
    real = []
    for job in jobs:
        if job.get("id") in starved_ids or job.get("name") in AGGREGATE_JOBS:
            continue
        conclusion = job.get("conclusion")
        if conclusion in OK_CONCLUSIONS:
            continue
        if conclusion == "cancelled":
            completed = _time(job.get("completed_at"))
            never_ran = not job.get("runner_name") and not job.get("steps")
            if never_ran or (completed is not None and completed >= first_starved):
                continue
        real.append(job.get("name", "?"))
    if real:
        return False, "jobs failed on their own: " + ", ".join(sorted(real))
    names = ", ".join(sorted(job.get("name", "?") for job in starved))
    return True, f"no runner picked up: {names}"


def queue_pr_number(head_branch: str) -> int | None:
    match = QUEUE_BRANCH.match(head_branch or "")
    return int(match.group(1)) if match else None


def queue_failures(runs: list[dict], number: int, now: datetime) -> int:
    """Count failed or cancelled merge-queue CI runs of one PR in the window."""

    count = 0
    for run in runs:
        if queue_pr_number(run.get("head_branch", "")) != number:
            continue
        if run.get("conclusion") not in ("failure", "cancelled"):
            continue
        created = _time(run.get("created_at"))
        if created is not None and now - created <= QUEUE_WINDOW:
            count += 1
    return count


# ---------------------------------------------------------------------------
# GitHub calls (gh CLI, GH_TOKEN from the workflow).


def _gh(*args: str) -> str:
    return subprocess.run(["gh", *args], check=True, capture_output=True, text=True).stdout


def _gh_lines(*args: str) -> list[dict]:
    return [json.loads(line) for line in _gh(*args).splitlines() if line.strip()]


def _graphql(query: str, **variables: str | int) -> dict:
    args = ["api", "graphql", "-f", f"query={query}"]
    for key, value in variables.items():
        args += ["-F" if isinstance(value, int) else "-f", f"{key}={value}"]
    return json.loads(_gh(*args))


def _run_jobs(repo: str, run_id: int, attempt: int) -> list[dict]:
    return _gh_lines(
        "api",
        "--paginate",
        f"repos/{repo}/actions/runs/{run_id}/attempts/{attempt}/jobs?per_page=100",
        "--jq",
        ".jobs[]",
    )


def _annotations(repo: str, job: dict) -> list[str]:
    if job.get("runner_name") or job.get("steps"):
        return []
    try:
        return [
            item.get("message", "")
            for item in _gh_lines(
                "api", f"repos/{repo}/check-runs/{job['id']}/annotations", "--jq", ".[]"
            )
        ]
    except subprocess.CalledProcessError:
        return []


def _requeue(repo: str, run: dict, now: datetime) -> str:
    number = queue_pr_number(run.get("head_branch", ""))
    if number is None:
        return "merge-queue run without a PR number; not re-queued"
    since = (now - QUEUE_WINDOW).strftime("%Y-%m-%dT%H:%M:%SZ")
    runs = _gh_lines(
        "api",
        "--paginate",
        f"repos/{repo}/actions/workflows/{run['workflow_id']}/runs"
        f"?event=merge_group&per_page=100&created=%3E%3D{since}",
        "--jq",
        ".workflow_runs[]",
    )
    failures = queue_failures(runs, number, now)
    if failures > MAX_RETRIES:
        return f"PR #{number} was dropped from the queue {failures} times in 6 hours; not re-queued"
    owner, name = repo.split("/", 1)
    data = _graphql(
        """query($owner:String!,$name:String!,$number:Int!){repository(owner:$owner,name:$name){
        pullRequest(number:$number){id state isDraft isInMergeQueue headRefName
        commits(last:1){nodes{commit{committedDate}}}}}}""",
        owner=owner,
        name=name,
        number=number,
    )
    pr = data["data"]["repository"]["pullRequest"]
    if pr["state"] != "OPEN" or pr["isDraft"]:
        return f"PR #{number} is not open and ready; not re-queued"
    if pr["isInMergeQueue"]:
        return f"PR #{number} is already back in the queue"
    if pr["headRefName"].startswith(RELEASE_BRANCH_PREFIX):
        return f"PR #{number} is the Release PR, which the release flow queues alone; not re-queued"
    nodes = pr["commits"]["nodes"]
    head_time = _time(nodes[0]["commit"]["committedDate"]) if nodes else None
    run_started = _time(run.get("run_started_at") or run.get("created_at"))
    if head_time is None or run_started is None or head_time > run_started:
        return f"PR #{number} has new commits since the dropped run; not re-queued"
    _graphql(
        "mutation($id:ID!){enqueuePullRequest(input:{pullRequestId:$id}){mergeQueueEntry{position}}}",
        id=pr["id"],
    )
    return f"PR #{number} put back in the merge queue (drop {failures} of at most {MAX_RETRIES + 1})"


def _main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", required=True)
    parser.add_argument("--run-id", type=int, required=True)
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args(argv)

    run = json.loads(_gh("api", f"repos/{args.repo}/actions/runs/{args.run_id}"))
    event, attempt = run["event"], int(run["run_attempt"])
    if run.get("conclusion") not in ("failure", "cancelled"):
        print(f"run {args.run_id} concluded {run.get('conclusion')}; nothing to retry")
        return 0
    # Outside the queue a cancelled run was superseded or stopped by hand.
    if run["conclusion"] == "cancelled" and event != "merge_group":
        print(f"{event} run {args.run_id} was cancelled; not retried")
        return 0
    if event not in ("pull_request", "push", "merge_group"):
        print(f"{event} runs are not retried")
        return 0

    jobs = _run_jobs(args.repo, args.run_id, attempt)
    annotations = {job["id"]: _annotations(args.repo, job) for job in jobs}
    retry, reason = classify(jobs, annotations)
    print(f"{event} run {args.run_id} attempt {attempt}: {reason}")
    if not retry:
        return 0

    now = datetime.now(timezone.utc)
    if event == "merge_group":
        if args.dry_run:
            print("dry run: would put the PR back in the merge queue")
            return 0
        print(_requeue(args.repo, run, now))
        return 0
    if attempt > MAX_RETRIES:
        print(f"already retried {attempt - 1} times; leaving it red")
        return 0
    if args.dry_run:
        print("dry run: would re-run the failed jobs")
        return 0
    _gh("api", "-X", "POST", f"repos/{args.repo}/actions/runs/{args.run_id}/rerun-failed-jobs")
    print(f"re-running the failed jobs (attempt {attempt + 1})")
    return 0


if __name__ == "__main__":
    raise SystemExit(_main(sys.argv[1:]))
