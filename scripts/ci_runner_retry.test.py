#!/usr/bin/env python3

import sys
import unittest
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts"))

from ci_runner_retry import classify, is_starved, queue_failures, queue_pr_number  # noqa: E402

SHA = "a8108f257f70016a74ede72a829bb6e9dcc5035f"


def job(job_id, name, conclusion, *, runner="GitHub Actions 1", steps=3, start="T19:07:00Z", end="T19:10:00Z"):
    return {
        "id": job_id,
        "name": name,
        "conclusion": conclusion,
        "runner_name": runner,
        "steps": [{}] * steps,
        "started_at": "2026-11-18" + start,
        "completed_at": "2026-11-18" + end,
    }


def starved(job_id, name, *, start="T19:07:15Z", end="T19:22:16Z"):
    return job(job_id, name, "cancelled", runner=None, steps=0, start=start, end=end)


class StarvedTests(unittest.TestCase):
    def test_no_runner_and_long_wait_is_starved(self):
        self.assertTrue(is_starved(starved(1, "mac"), []))

    def test_annotation_alone_is_enough(self):
        quick = starved(1, "mac", start="T19:07:15Z", end="T19:08:00Z")
        self.assertFalse(is_starved(quick, []))
        self.assertTrue(
            is_starved(
                quick,
                ["The job was not acquired by Runner of type hosted even after multiple attempts"],
            )
        )

    def test_a_job_that_ran_is_never_starved(self):
        self.assertFalse(is_starved(job(1, "mac", "failure", start="T19:00:00Z", end="T19:30:00Z"), []))
        self.assertFalse(
            is_starved(job(1, "mac", "cancelled", runner=None, steps=2, end="T19:30:00Z"), [])
        )
        self.assertFalse(is_starved(starved(1, "mac") | {"conclusion": "success"}, []))


class ClassifyTests(unittest.TestCase):
    def test_merge_queue_drop_from_2026_11_18_is_retried(self):
        # Run 44376916467: one macOS leg never got a runner, fail-fast then
        # cancelled the rest, and `conclusion` failed.
        jobs = [
            job(1, "merge-group-fail-fast", "cancelled", end="T19:22:35Z"),
            job(2, "test (windows-2022)", "cancelled", steps=29, end="T19:22:51Z"),
            job(3, "linux-nextest (slice:1/3)", "cancelled", steps=11, end="T19:22:40Z"),
            starved(4, "native-platform-contracts (macos-15)"),
            job(5, "macos-nextest (${{ matrix.partition }})", "cancelled", runner=None, steps=0,
                start="T19:23:04Z", end="T19:23:03Z"),
            job(6, "conclusion", "failure", end="T19:23:10Z"),
            job(7, "fmt", "success"),
            job(8, "docs", "skipped"),
        ]
        retry, reason = classify(jobs, {})
        self.assertTrue(retry, reason)
        self.assertIn("native-platform-contracts (macos-15)", reason)

    def test_a_real_failure_is_never_retried(self):
        jobs = [
            starved(1, "macos-nextest (slice:1/3)"),
            job(2, "linux-nextest (slice:2/3)", "failure", end="T19:20:00Z"),
            job(3, "conclusion", "failure"),
        ]
        retry, reason = classify(jobs, {})
        self.assertFalse(retry)
        self.assertIn("linux-nextest (slice:2/3)", reason)

    def test_a_job_cancelled_before_the_starved_one_finished_blocks(self):
        # Cancelled with steps before any runner shortage showed: its own
        # timeout, not the fail-fast cascade.
        jobs = [
            starved(1, "app-check"),
            job(2, "app-e2e (shard 1/4)", "cancelled", steps=9, end="T19:15:00Z"),
        ]
        self.assertFalse(classify(jobs, {})[0])

    def test_nothing_starved_means_no_retry(self):
        jobs = [job(1, "fmt", "failure"), job(2, "conclusion", "failure")]
        self.assertEqual(classify(jobs, {}), (False, "no job was starved of a runner"))

    def test_annotation_marks_a_quick_failure_as_starved(self):
        jobs = [
            job(9, "test (macos-15)", "failure", runner=None, steps=0, start="T19:07:00Z", end="T19:08:00Z"),
            job(10, "conclusion", "failure"),
        ]
        self.assertFalse(classify(jobs, {})[0])
        self.assertTrue(classify(jobs, {9: ["was not acquired by Runner of type hosted"]})[0])


class QueueTests(unittest.TestCase):
    def test_pr_number_from_queue_branch(self):
        self.assertEqual(queue_pr_number(f"gh-readonly-queue/main/pr-1327-{SHA}"), 1327)
        self.assertEqual(queue_pr_number(f"refs/heads/gh-readonly-queue/main/pr-7-{SHA}"), 7)
        self.assertIsNone(queue_pr_number("main"))
        self.assertIsNone(queue_pr_number("gh-readonly-queue/main/pr-7-short"))

    def test_drop_budget_counts_only_this_pr_in_the_window(self):
        now = datetime(2026, 11, 18, 20, 0, tzinfo=timezone.utc)

        def run(number, conclusion, created):
            return {
                "head_branch": f"gh-readonly-queue/main/pr-{number}-{SHA}",
                "conclusion": conclusion,
                "created_at": created,
            }

        runs = [
            run(1327, "failure", "2026-11-18T17:25:16Z"),
            run(1327, "cancelled", "2026-11-18T19:06:20Z"),
            run(1327, "success", "2026-11-18T19:40:00Z"),
            run(1327, "failure", "2026-11-18T09:00:00Z"),  # outside six hours
            run(1328, "failure", "2026-11-18T19:30:00Z"),
        ]
        self.assertEqual(queue_failures(runs, 1327, now), 2)
        self.assertEqual(queue_failures(runs, 1328, now), 1)
        self.assertEqual(queue_failures(runs, 9, now), 0)


if __name__ == "__main__":
    unittest.main()
