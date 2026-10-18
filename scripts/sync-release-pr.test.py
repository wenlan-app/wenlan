#!/usr/bin/env python3
"""Focused tests for fail-closed Release PR branch synchronization."""

from __future__ import annotations

import importlib.util
import subprocess
import sys
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("sync-release-pr.py")
SPEC = importlib.util.spec_from_file_location("sync_release_pr", SCRIPT)
assert SPEC and SPEC.loader
SYNC = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = SYNC
SPEC.loader.exec_module(SYNC)

REPOSITORY = "wenlan-app/wenlan"
HEAD_SHA = "a" * 40


def release_pr(**updates: object) -> dict[str, object]:
    fixture: dict[str, object] = {
        "number": 430,
        "state": "open",
        "draft": False,
        "user": {"login": "7xuanlu"},
        "base": {"ref": "main", "repo": {"full_name": REPOSITORY}},
        "head": {
            "ref": "release-please--branches--main",
            "sha": HEAD_SHA,
            "repo": {"full_name": REPOSITORY, "fork": False},
        },
    }
    fixture.update(updates)
    return fixture


class FakeApi:
    def __init__(self, payload: object) -> None:
        self.payload = payload
        self.calls: list[tuple[str, dict[str, object]]] = []

    def get_json(self, path: str, *, params: dict[str, object]) -> object:
        self.calls.append((path, params))
        return self.payload


class FakeRunner:
    def __init__(
        self,
        *,
        push_fails: bool = False,
        remote_sha: str = HEAD_SHA,
        unmerged: tuple[str, ...] | None = None,
    ) -> None:
        self.commands: list[tuple[str, ...]] = []
        self.push_fails = push_fails
        self.remote_sha = remote_sha
        # None = the merge succeeds; a tuple = it stops with these paths unmerged.
        self.unmerged = unmerged

    def __call__(
        self,
        command: list[str],
        *,
        check: bool,
        capture_output: bool = False,
    ) -> subprocess.CompletedProcess[str]:
        del capture_output
        recorded = tuple(command)
        self.commands.append(recorded)
        if command == ["git", "rev-parse", "HEAD"]:
            return subprocess.CompletedProcess(command, 0, stdout=HEAD_SHA + "\n")
        if command == ["git", "merge", "--no-edit", "origin/main"]:
            return subprocess.CompletedProcess(
                command, 0 if self.unmerged is None else 1, stdout=""
            )
        if command == ["git", "diff", "--name-only", "--diff-filter=U"]:
            return subprocess.CompletedProcess(
                command, 0, stdout="\n".join(self.unmerged or ()) + "\n"
            )
        if command == ["git", "diff", "--cached", "--quiet"]:
            return subprocess.CompletedProcess(command, 0, stdout="")
        if command[:3] == ["git", "ls-remote", "--exit-code"]:
            ref = "refs/heads/release-please--branches--main"
            return subprocess.CompletedProcess(
                command, 0, stdout=f"{self.remote_sha}\t{ref}\n"
            )
        if command[:3] == ["git", "push", "origin"] and self.push_fails:
            raise subprocess.CalledProcessError(1, command)
        return subprocess.CompletedProcess(command, 0, stdout="")


class SyncReleasePrTests(unittest.TestCase):
    def test_zero_one_and_multiple_release_prs_are_closed(self) -> None:
        self.assertIsNone(SYNC.resolve_release_pr(FakeApi([]), REPOSITORY))
        api = FakeApi([release_pr()])
        resolved = SYNC.resolve_release_pr(api, REPOSITORY)
        self.assertEqual(
            api.calls[0][1]["head"], "wenlan-app:release-please--branches--main"
        )
        self.assertEqual(resolved.number, 430)
        self.assertEqual(resolved.head_sha, HEAD_SHA)
        with self.assertRaisesRegex(SYNC.SyncError, "at most one"):
            SYNC.resolve_release_pr(
                FakeApi([release_pr(), release_pr(number=431)]), REPOSITORY
            )

    def test_wrong_release_pr_identity_fails_closed(self) -> None:
        for repository in ("mallory/wenlan", "7xuanlu/wenlan"):
            with self.subTest(repository=repository), self.assertRaisesRegex(
                SYNC.SyncError, "pinned release repository owner"
            ):
                SYNC.resolve_release_pr(FakeApi([release_pr()]), repository)
        mutations = {
            "draft": {"draft": True},
            "owner": {"user": {"login": "mallory"}},
            "head repository": {
                "head": {
                    "ref": "release-please--branches--main",
                    "sha": HEAD_SHA,
                    "repo": {"full_name": "mallory/wenlan", "fork": True},
                }
            },
            "base": {
                "base": {"ref": "other", "repo": {"full_name": REPOSITORY}}
            },
        }
        for label, mutation in mutations.items():
            with self.subTest(label=label), self.assertRaisesRegex(
                SYNC.SyncError, "identity"
            ):
                SYNC.resolve_release_pr(FakeApi([release_pr(**mutation)]), REPOSITORY)

    def test_noop_still_pushes_exact_nonforce_refspec(self) -> None:
        runner = FakeRunner()
        SYNC.synchronize_release_branch(
            HEAD_SHA, Path("/trusted/bump-version.sh"), run_command=runner
        )
        self.assertIn(("git", "fetch", "--no-tags", "origin", "main"), runner.commands)
        self.assertIn(("git", "merge", "--no-edit", "origin/main"), runner.commands)
        self.assertEqual(
            runner.commands[-1],
            (
                "git",
                "push",
                "origin",
                "HEAD:refs/heads/release-please--branches--main",
            ),
        )
        self.assertNotIn("--force", runner.commands[-1])

    def test_branch_move_is_rejected_before_nonforce_push(self) -> None:
        runner = FakeRunner(remote_sha="b" * 40)
        with self.assertRaisesRegex(SYNC.SyncError, "moved or disappeared"):
            SYNC.synchronize_release_branch(
                HEAD_SHA, Path("/trusted/bump-version.sh"), run_command=runner
            )
        self.assertFalse(
            any(command[:3] == ("git", "push", "origin") for command in runner.commands)
        )

    def test_racing_nonforce_push_rejection_fails_closed(self) -> None:
        runner = FakeRunner(push_fails=True)
        with self.assertRaisesRegex(SYNC.SyncError, "branch may have moved"):
            SYNC.synchronize_release_branch(
                HEAD_SHA, Path("/trusted/bump-version.sh"), run_command=runner
            )

    def test_version_only_conflict_takes_main_then_rebumps(self) -> None:
        paths = (
            "crates/wenlan-cli/npm/package.json",
            "crates/wenlan-mcp/npm/package.json",
        )
        runner = FakeRunner(unmerged=paths)
        SYNC.synchronize_release_branch(
            HEAD_SHA, Path("/trusted/bump-version.sh"), run_command=runner
        )
        resolve = [
            ("git", "checkout", "--theirs", "--", *paths),
            ("git", "add", "--", *paths),
            ("git", "commit", "--no-edit"),
            ("bash", "/trusted/bump-version.sh"),
        ]
        start = runner.commands.index(resolve[0])
        self.assertEqual(runner.commands[start : start + len(resolve)], resolve)
        self.assertNotIn(("git", "merge", "--abort"), runner.commands)
        self.assertEqual(
            runner.commands[-1],
            ("git", "push", "origin", "HEAD:refs/heads/release-please--branches--main"),
        )

    def test_conflict_outside_version_files_aborts_without_push(self) -> None:
        runner = FakeRunner(unmerged=("package.json", "CHANGELOG.md"))
        with self.assertRaisesRegex(SYNC.SyncError, "manual resolution: CHANGELOG.md"):
            SYNC.synchronize_release_branch(
                HEAD_SHA, Path("/trusted/bump-version.sh"), run_command=runner
            )
        self.assertIn(("git", "merge", "--abort"), runner.commands)
        for command in runner.commands:
            self.assertNotEqual(command[:2], ("git", "checkout"))
            self.assertNotEqual(command[:1], ("bash",))
            self.assertNotEqual(command[:3], ("git", "push", "origin"))

    def test_failed_merge_without_conflicts_fails_closed(self) -> None:
        runner = FakeRunner(unmerged=())
        with self.assertRaisesRegex(SYNC.SyncError, "no conflicted paths"):
            SYNC.synchronize_release_branch(
                HEAD_SHA, Path("/trusted/bump-version.sh"), run_command=runner
            )
        self.assertIn(("git", "merge", "--abort"), runner.commands)

    def test_version_synced_paths_are_all_rewritten_by_bump_script(self) -> None:
        bump = SCRIPT.with_name("bump-version.sh").read_text(encoding="utf-8")
        for path in sorted(SYNC.VERSION_SYNCED_PATHS):
            with self.subTest(path=path):
                self.assertIn(path, bump)


if __name__ == "__main__":
    unittest.main()
