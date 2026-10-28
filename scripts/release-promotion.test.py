#!/usr/bin/env python3
"""Focused contracts for the trusted release-promotion consumer."""

from __future__ import annotations

import io
import hashlib
import importlib.util
import json
import sys
import tempfile
import unittest
import zipfile
from pathlib import Path
from unittest import mock


SCRIPT = Path(__file__).with_name("release-promotion.py")
SPEC = importlib.util.spec_from_file_location("release_promotion", SCRIPT)
assert SPEC and SPEC.loader
PROMOTION = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = PROMOTION
SPEC.loader.exec_module(PROMOTION)

REPOSITORY = "wenlan-app/wenlan"
HEAD_SHA = "1" * 40


class FakeApi:
    def __init__(self, responses: dict[tuple[str, tuple], object]) -> None:
        self.responses = responses
        self.calls: list[tuple[str, tuple]] = []

    def get_json(self, path: str, *, params: dict[str, object] | None = None):
        key = (path, tuple(sorted((params or {}).items())))
        self.calls.append(key)
        return self.responses[key]


class ReleasePromotionTests(unittest.TestCase):
    ASSET_NAME = "wenlan-test.zip"
    ASSET_BYTES = b"exact validated release asset"

    def _queue_fixture(self):
        base_sha, queue_sha, tree_sha = "2" * 40, "3" * 40, "4" * 40
        pr = {
            "number": 1020, "state": "open", "draft": False, "merged_at": None,
            "head": {"ref": PROMOTION.RELEASE_BRANCH, "sha": HEAD_SHA},
            "base": {"ref": "main", "sha": base_sha},
        }
        event = {"merge_group": {
            "head_ref": f"refs/heads/gh-readonly-queue/main/pr-1020-{base_sha}",
            "head_sha": queue_sha, "base_ref": "refs/heads/main", "base_sha": base_sha,
        }}
        api = FakeApi({
            (f"/repos/{REPOSITORY}/pulls/1020", ()): pr,
            (f"/repos/{REPOSITORY}/git/commits/{HEAD_SHA}", ()): {"tree": {"sha": tree_sha}},
            (f"/repos/{REPOSITORY}/git/commits/{queue_sha}", ()): {"tree": {"sha": tree_sha}},
            (f"/repos/{REPOSITORY}", ()): {"id": 2, "full_name": REPOSITORY},
            (f"/repos/{REPOSITORY}/git/ref/heads/main", ()): {
                "object": {"type": "commit", "sha": base_sha}},
        })
        return api, event, pr

    def _queue_proof(self, api, event, *, candidate_error=None, main_error=None, receipt_error=None, receipt_version="0.20.3"):
        with tempfile.TemporaryDirectory() as raw, mock.patch.object(
            PROMOTION.VALIDATOR, "validate_trusted_release_candidate",
            return_value=("0.20.3", HEAD_SHA), side_effect=candidate_error,
        ) as candidate, mock.patch.object(
            PROMOTION, "_conclusion_check_suite", return_value=11,
        ), mock.patch.object(
            PROMOTION, "_source_run", return_value={"id": 100, "run_attempt": 1},
        ), mock.patch.object(
            PROMOTION, "verify_main_ci", side_effect=main_error,
        ) as main, mock.patch.object(
            PROMOTION, "_find_receipt", side_effect=receipt_error,
            return_value=({"version": receipt_version, "pr_number": 1020}, {"id": 77}),
        ) as receipt:
            result = PROMOTION.verify_queue(api, REPOSITORY, event, Path(raw))
        return result, candidate, main, receipt

    def test_release_queue_reuses_only_the_closed_same_tree_current_base_proof(self):
        api, event, pr = self._queue_fixture()
        result, candidate, main, receipt = self._queue_proof(api, event)
        self.assertEqual(result.state, "validated", result.reason)
        candidate.assert_called_once_with({"pull_request": pr}, api, REPOSITORY)
        main.assert_called_once_with(api, REPOSITORY, event["merge_group"]["base_sha"])
        self.assertEqual(receipt.call_args.args[4:7], (1020, HEAD_SHA, "4" * 40))
        self.assertIsNone(result.main_run_id)  # A queue result is never a main promotion plan.

    def test_ordinary_queue_keeps_matrix_without_reading_release_evidence(self):
        api, event, pr = self._queue_fixture()
        pr["head"]["ref"] = "feature/ordinary"
        result, candidate, main, receipt = self._queue_proof(api, event)
        self.assertEqual(result.state, "ordinary")
        candidate.assert_not_called()
        main.assert_not_called()
        receipt.assert_not_called()

    def test_release_queue_rejects_changed_tree_base_or_malformed_queue(self):
        for mutation, reason in [
            ("tree", "queue tree differs"),
            ("base", "queue base differs"),
            ("ref", "queue ref does not identify"),
            ("base_ref", "queue base ref is not main"),
        ]:
            with self.subTest(mutation=mutation):
                api, event, pr = self._queue_fixture()
                if mutation == "tree":
                    api.responses[(f"/repos/{REPOSITORY}/git/commits/{'3' * 40}", ())] = {
                        "tree": {"sha": "5" * 40}}
                elif mutation == "base":
                    pr["base"]["sha"] = "5" * 40
                elif mutation == "ref":
                    event["merge_group"]["head_ref"] = "refs/heads/feature/release"
                else:
                    event["merge_group"]["base_ref"] = "refs/heads/other"
                result, _, _, receipt = self._queue_proof(api, event)
                self.assertEqual(result.state, "blocked")
                self.assertIn(reason, result.reason)
                receipt.assert_not_called()

    def test_queue_cli_fails_closed_on_invalid_event_without_writing_a_plan(self):
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            event = root / "event.json"
            output = root / "outputs"
            event.write_text("{invalid", encoding="utf-8")
            with mock.patch.object(PROMOTION.VALIDATOR, "GitHubApi"), mock.patch(
                "sys.stdout", new=io.StringIO()
            ):
                status = PROMOTION._main([
                    "gate-queue", "--repository", REPOSITORY, "--event", str(event),
                    "--temp-root", str(root), "--github-output", str(output),
                ])
            self.assertEqual(status, 2)
            self.assertIn("verified-release-merge=false", output.read_text(encoding="utf-8"))
            self.assertEqual({path.name for path in root.iterdir()}, {"event.json", "outputs"})

    def test_release_queue_rejects_missing_failed_or_conflicting_evidence(self):
        for kwargs, reason in [
            ({"candidate_error": PROMOTION.VALIDATOR.CandidateError("noncanonical diff")}, "noncanonical diff"),
            ({"main_error": PROMOTION.PromotionError("base CI failed")}, "base CI failed"),
            ({"receipt_error": PROMOTION.PendingEvidence("receipt absent")}, "receipt absent"),
            ({"receipt_error": PROMOTION.PromotionError("conflicting receipt")}, "conflicting receipt"),
            ({"receipt_version": "0.20.2"}, "version differs"),
        ]:
            with self.subTest(reason=reason):
                api, event, _ = self._queue_fixture()
                result, _, _, _ = self._queue_proof(api, event, **kwargs)
                self.assertEqual(result.state, "blocked")
                self.assertIn(reason, result.reason)

    def test_release_queue_with_real_semantic_validator_rejects_source_changes(self):
        fixture_spec = importlib.util.spec_from_file_location(
            "queue_semantic_fixtures", Path(__file__).with_name("validate-release-candidate.test.py")
        )
        fixtures = importlib.util.module_from_spec(fixture_spec)
        fixture_spec.loader.exec_module(fixtures)
        old, new = fixtures.release_contents()
        base_api = fixtures.TrustedCandidateApi(old, new)
        queue_sha = "f" * 40
        event = {"merge_group": {
            "head_ref": f"refs/heads/gh-readonly-queue/main/pr-42-{fixtures.BASE_SHA}",
            "head_sha": queue_sha, "base_ref": "refs/heads/main", "base_sha": fixtures.BASE_SHA,
        }}
        class Api:
            def get_json(self, path, *, params=None):
                if path.endswith(f"/git/commits/{queue_sha}"):
                    return {"tree": {"sha": fixtures.HEAD_TREE_SHA}}
                return base_api.get_json(path, params=params)
        with tempfile.TemporaryDirectory() as raw, mock.patch.object(
            PROMOTION, "_conclusion_check_suite", return_value=11,
        ), mock.patch.object(
            PROMOTION, "_source_run", return_value={"id": 100, "run_attempt": 1},
        ), mock.patch.object(PROMOTION, "verify_main_ci"), mock.patch.object(
            PROMOTION, "_find_receipt", return_value=(
                {"version": new["version.txt"].strip(), "pr_number": 42}, {"id": 77}),
        ) as receipt:
            result = PROMOTION.verify_queue(Api(), REPOSITORY, event, Path(raw))
            self.assertEqual(result.state, "validated", result.reason)
            # A same-name release with extra Cargo behaviour cannot reuse the proof.
            new["Cargo.toml"] += '\n[profile.release]\nlto = true\n'
            result = PROMOTION.verify_queue(Api(), REPOSITORY, event, Path(raw))
            self.assertEqual(result.state, "blocked")
            self.assertIn("version-only", result.reason)
            self.assertEqual(receipt.call_count, 1)

    def test_release_queue_rejects_main_or_candidate_moving_during_receipt_read(self):
        for moved in ["main", "candidate"]:
            with self.subTest(moved=moved):
                api, event, pr = self._queue_fixture()
                if moved == "main":
                    api.responses[(f"/repos/{REPOSITORY}/git/ref/heads/main", ())]["object"]["sha"] = "5" * 40
                else:
                    original = api.get_json
                    reads = 0
                    def moving(path, *, params=None):
                        nonlocal reads
                        if path.endswith("/pulls/1020"):
                            reads += 1
                            if reads > 1:
                                return {**pr, "head": {**pr["head"], "sha": "5" * 40}}
                        return original(path, params=params)
                    api.get_json = moving
                result, _, _, _ = self._queue_proof(api, event)
                self.assertEqual(result.state, "blocked")
                self.assertIn("moved during queue verification", result.reason)

    def test_queue_composes_real_control_plane_and_closed_receipt_validation(self):
        spec = importlib.util.spec_from_file_location(
            "queue_receipt_fixtures", Path(__file__).with_name("validate-release-candidate.test.py")
        )
        fixtures = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(fixtures)
        old, new = fixtures.release_contents()
        content = fixtures.TrustedCandidateApi(old, new)
        queue_sha = "f" * 40
        repo = {"id": 2, "full_name": REPOSITORY, "fork": False}
        source = {
            "id": 100, "run_attempt": 1, "workflow_id": 9, "check_suite_id": 11,
            "path": PROMOTION.CI_WORKFLOW_PATH, "event": "pull_request",
            "status": "completed", "conclusion": "success",
            "head_branch": PROMOTION.RELEASE_BRANCH, "head_sha": fixtures.HEAD_SHA,
            "repository": repo, "head_repository": repo,
        }
        main = {**source, "id": 200, "event": "push", "head_branch": "main", "head_sha": fixtures.BASE_SHA}
        observer = {
            **source, "id": 300, "workflow_id": 10, "event": "workflow_run",
            "path": PROMOTION.OBSERVER_WORKFLOW_PATH, "head_sha": fixtures.BASE_SHA,
            "display_title": "release-candidate-observer source 100/1",
        }
        assets = {
            "id": 500, "name": "validated-release-assets-100-1-300-1",
            "digest": "sha256:" + "d" * 64, "size_in_bytes": 100, "expired": False,
            "workflow_run": {"id": 300, "repository_id": 2, "head_repository_id": 2},
        }
        event = {"merge_group": {
            "head_ref": f"refs/heads/gh-readonly-queue/main/pr-42-{fixtures.BASE_SHA}",
            "head_sha": queue_sha, "base_ref": "refs/heads/main", "base_sha": fixtures.BASE_SHA,
        }}
        receipt = {
            "status": "passed", "repository": REPOSITORY, "repository_id": 2,
            "run_id": 100, "run_attempt": 1, "check_suite_id": 11, "workflow_id": 9,
            "pr_number": 42, "pr_state": "open", "merge_commit_sha": None,
            "head_sha": fixtures.HEAD_SHA, "tree_sha": fixtures.HEAD_TREE_SHA,
            "version": new["version.txt"].strip(),
            "paths": sorted(PROMOTION.VALIDATOR.REQUIRED_RELEASE_PATHS),
            "artifacts": [
                {"id": i + 1, "name": f"release-candidate-100-1-{entry['target']}",
                 "size": 100, "digest": "sha256:" + "a" * 64}
                for i, entry in enumerate(PROMOTION.VALIDATOR.release_matrix()["include"])
            ],
            "assets": [
                {"name": asset["name"], "target": asset["target"], "size": 100, "sha256": "b" * 64}
                for asset in PROMOTION.release_assets()
            ],
        }
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            path = root / "closed.json"
            PROMOTION.VALIDATOR.write_validated_receipt(path, receipt)
            PROMOTION.VALIDATOR.close_receipt(
                path, artifact_name=assets["name"], artifact_id=500,
                artifact_digest="d" * 64, observer_run_id=300, observer_run_attempt=1,
                observer_workflow_id=10, observer_code_sha=fixtures.BASE_SHA,
            )
            buffer = io.BytesIO()
            with zipfile.ZipFile(buffer, "w", zipfile.ZIP_STORED) as archive:
                archive.writestr("validated-release-receipt.json", path.read_bytes())
            wrapper = buffer.getvalue()
            artifact = {
                "id": 400, "name": "validated-release-receipt-100-1", "expired": False,
                "size_in_bytes": len(wrapper), "digest": "sha256:" + hashlib.sha256(wrapper).hexdigest(),
                "workflow_run": {"id": 300},
            }
            class Api:
                def get_json(self, route, *, params=None):
                    prefix = f"/repos/{REPOSITORY}"
                    if route == f"{prefix}/git/commits/{queue_sha}":
                        return {"tree": {"sha": fixtures.HEAD_TREE_SHA}}
                    if route == f"{prefix}/commits/{fixtures.HEAD_SHA}/check-runs":
                        return {"check_runs": [{
                            "name": "conclusion", "status": "completed", "conclusion": "success",
                            "head_sha": fixtures.HEAD_SHA, "app": {"slug": "github-actions"},
                            "check_suite": {"id": 11},
                        }]}
                    if route == f"{prefix}/actions/workflows/ci.yml/runs":
                        return {"total_count": 1, "workflow_runs": [source if params["event"] == "pull_request" else main]}
                    if route == f"{prefix}/actions/runs/200":
                        return main
                    if route in [f"{prefix}/actions/workflows/9", f"{prefix}/actions/workflows/10"]:
                        workflow_id = int(route.rsplit("/", 1)[1])
                        return {"id": workflow_id, "state": "active", "path": (
                            PROMOTION.CI_WORKFLOW_PATH if workflow_id == 9 else PROMOTION.OBSERVER_WORKFLOW_PATH)}
                    if route == f"{prefix}/actions/workflows/release-candidate-observer.yml/runs":
                        return {"workflow_runs": [observer]}
                    if route == f"{prefix}/actions/artifacts":
                        return {"total_count": 1, "artifacts": [artifact]}
                    if route == f"{prefix}/actions/runs/300/attempts/1":
                        return observer
                    if route == f"{prefix}/actions/artifacts/500":
                        return assets
                    return content.get_json(route, params=params)

                def download(self, route, destination, maximum_bytes):
                    if route != f"/repos/{REPOSITORY}/actions/artifacts/400/zip":
                        raise AssertionError(f"unexpected download: {route}")
                    destination.write_bytes(wrapper)
                    return len(wrapper), hashlib.sha256(wrapper).hexdigest()

            # No internal verifier is mocked: semantic validation, exact runs,
            # archive bytes/digest, closed schema and observer/asset metadata compose.
            result = PROMOTION.verify_queue(Api(), REPOSITORY, event, root)
            self.assertEqual(result.state, "validated", result.reason)
            for field, replacement, reason in [
                ("digest", "sha256:" + "e" * 64, "REST metadata differs"),
                ("expired", True, "REST metadata differs"),
            ]:
                previous = assets[field]
                assets[field] = replacement
                result = PROMOTION.verify_queue(Api(), REPOSITORY, event, root)
                self.assertEqual(result.state, "blocked")
                self.assertIn(reason, result.reason)
                assets[field] = previous

    def _validated_assets_fixture(self) -> tuple[object, dict, dict]:
        """Build an exact single-asset promotion plan and its download API."""

        artifact_id = 77
        wrapper_buffer = io.BytesIO()
        with zipfile.ZipFile(wrapper_buffer, "w", zipfile.ZIP_STORED) as archive:
            archive.writestr(self.ASSET_NAME, self.ASSET_BYTES)
        wrapper_bytes = wrapper_buffer.getvalue()
        wrapper_digest = hashlib.sha256(wrapper_bytes).hexdigest()
        asset_digest = hashlib.sha256(self.ASSET_BYTES).hexdigest()

        class DownloadApi:
            def get_json(self, path: str, *, params=None):
                self.assert_path = path
                return {
                    "id": artifact_id,
                    "name": "validated-release-assets-100-1",
                    "digest": f"sha256:{wrapper_digest}",
                    "expired": False,
                    "size_in_bytes": len(wrapper_bytes),
                }

            def download(self, path: str, destination: Path, maximum_bytes: int):
                self.download_path = path
                self.maximum_bytes = maximum_bytes
                destination.write_bytes(wrapper_bytes)
                return len(wrapper_bytes), wrapper_digest

        plan = {
            "receipt": {
                "validated_assets_artifact": {
                    "id": artifact_id,
                    "name": "validated-release-assets-100-1",
                    "digest": f"sha256:{wrapper_digest}",
                },
                "assets": [
                    {
                        "name": self.ASSET_NAME,
                        "size": len(self.ASSET_BYTES),
                        "sha256": asset_digest,
                    }
                ],
            }
        }
        definition = {
            "name": self.ASSET_NAME,
            "archive": "zip",
            "members": ["wenlan.exe"],
        }
        return DownloadApi(), plan, definition

    def test_download_validated_assets_lets_safe_extractor_create_destination(self) -> None:
        api, plan, definition = self._validated_assets_fixture()

        with tempfile.TemporaryDirectory() as raw, mock.patch.object(
            PROMOTION, "release_assets", return_value=[definition]
        ), mock.patch.object(PROMOTION, "safe_extract_archive") as inspect:
            output_dir = Path(raw) / "promoted-assets"
            paths = PROMOTION.download_validated_assets(
                api, REPOSITORY, plan, output_dir
            )

            self.assertEqual(paths, [output_dir / self.ASSET_NAME])
            self.assertEqual(
                (output_dir / self.ASSET_NAME).read_bytes(), self.ASSET_BYTES
            )
            inspect.assert_called_once_with(
                output_dir / self.ASSET_NAME,
                output_dir.parent / f"inspect-{self.ASSET_NAME}",
                "zip",
                ["wenlan.exe"],
            )

    def test_download_validated_assets_fails_closed_on_existing_destination(self) -> None:
        api, plan, definition = self._validated_assets_fixture()

        with tempfile.TemporaryDirectory() as raw, mock.patch.object(
            PROMOTION, "release_assets", return_value=[definition]
        ), mock.patch.object(PROMOTION, "safe_extract_archive") as inspect:
            output_dir = Path(raw) / "promoted-assets"
            output_dir.mkdir()

            # The safe extractor owns the destination, so a destination that
            # already exists is an anomaly that must still refuse to promote.
            with self.assertRaises(FileExistsError):
                PROMOTION.download_validated_assets(
                    api, REPOSITORY, plan, output_dir
                )

            self.assertEqual(list(output_dir.iterdir()), [])
            inspect.assert_not_called()

    def test_release_context_scopes_compatibility_client_to_pr_proof(self) -> None:
        main_sha = "2" * 40
        head_sha = "3" * 40
        association_path = f"/repos/{REPOSITORY}/commits/{main_sha}/pulls"
        compatibility = FakeApi(
            {
                (
                    association_path,
                    (("per_page", 100),),
                ): [
                    {
                        "number": 430,
                        "base": {"ref": "main"},
                        "head": {"ref": PROMOTION.RELEASE_BRANCH, "sha": head_sha},
                        "merge_commit_sha": main_sha,
                    }
                ]
            }
        )
        modern = FakeApi({})
        proof = mock.Mock(verified=True, pr_head_sha=head_sha)

        with mock.patch.object(
            PROMOTION.MERGE_PROOF,
            "verify_release_merge",
            return_value=proof,
        ) as verify:
            resolved, resolved_head = PROMOTION._release_context(
                modern, REPOSITORY, main_sha, pr_api=compatibility
            )

        self.assertIs(resolved, proof)
        self.assertEqual(resolved_head, head_sha)
        self.assertEqual(modern.calls, [])
        self.assertEqual(
            compatibility.calls,
            [(association_path, (("per_page", 100),))],
        )
        verify.assert_called_once_with(
            modern,
            REPOSITORY,
            event_name="push",
            ref="refs/heads/main",
            sha=main_sha,
            associated_pulls=compatibility.responses[
                (association_path, (("per_page", 100),))
            ],
            pr_api=compatibility,
        )

    def test_release_context_reuses_the_association_snapshot(self) -> None:
        main_sha = "2" * 40
        head_sha = "3" * 40
        association = {
            "number": 430,
            "base": {"ref": "main"},
            "head": {"ref": PROMOTION.RELEASE_BRANCH, "sha": head_sha},
        }
        path = f"/repos/{REPOSITORY}/commits/{main_sha}/pulls"
        api = FakeApi(
            {
                (path, (("per_page", 100),)): [association],
            }
        )
        proof = mock.Mock(verified=True, pr_head_sha=head_sha)

        with mock.patch.object(
            PROMOTION.MERGE_PROOF,
            "verify_release_merge",
            return_value=proof,
        ) as verify:
            resolved, resolved_head = PROMOTION._release_context(
                api, REPOSITORY, main_sha
            )

        self.assertIs(resolved, proof)
        self.assertEqual(resolved_head, head_sha)
        verify.assert_called_once_with(
            api,
            REPOSITORY,
            event_name="push",
            ref="refs/heads/main",
            sha=main_sha,
            associated_pulls=[association],
            pr_api=api,
        )
        self.assertEqual(api.calls, [(path, (("per_page", 100),))])

    def test_release_context_recovers_an_initially_missing_association(self) -> None:
        main_sha = "2" * 40
        head_sha = "3" * 40
        association_path = f"/repos/{REPOSITORY}/commits/{main_sha}/pulls"
        commit_path = f"/repos/{REPOSITORY}/git/commits/{main_sha}"
        pr_path = f"/repos/{REPOSITORY}/pulls/430"
        release_pr = {
            "number": 430,
            "state": "closed",
            "merged_at": "2026-08-02T19:57:49Z",
            "merge_commit_sha": main_sha,
            "base": {"ref": "main"},
            "head": {"ref": PROMOTION.RELEASE_BRANCH, "sha": head_sha},
        }
        api = FakeApi(
            {
                (association_path, (("per_page", 100),)): [],
                (commit_path, ()): {
                    "message": "chore(main): release 0.15.4 (#430)\n\nrelease body"
                },
                (pr_path, ()): release_pr,
            }
        )
        proof = mock.Mock(verified=True, pr_head_sha=head_sha)

        with mock.patch.object(
            PROMOTION.MERGE_PROOF,
            "verify_release_merge",
            return_value=proof,
        ) as verify:
            resolved, resolved_head = PROMOTION._release_context(
                api, REPOSITORY, main_sha
            )

        self.assertIs(resolved, proof)
        self.assertEqual(resolved_head, head_sha)
        verify.assert_called_once_with(
            api,
            REPOSITORY,
            event_name="push",
            ref="refs/heads/main",
            sha=main_sha,
            associated_pulls=[release_pr],
            pr_api=api,
        )

    def test_release_context_does_not_delay_an_ordinary_main_commit(self) -> None:
        main_sha = "2" * 40
        association_path = f"/repos/{REPOSITORY}/commits/{main_sha}/pulls"
        commit_path = f"/repos/{REPOSITORY}/git/commits/{main_sha}"
        api = FakeApi(
            {
                (association_path, (("per_page", 100),)): [],
                (commit_path, ()): {"message": "ci: ordinary main change"},
            }
        )

        resolved, resolved_head = PROMOTION._release_context(api, REPOSITORY, main_sha)

        self.assertIsNone(resolved)
        self.assertIsNone(resolved_head)
        self.assertEqual(
            api.calls,
            [
                (association_path, (("per_page", 100),)),
                (commit_path, ()),
            ],
        )

    def test_release_context_fails_closed_on_a_spoofed_release_title(self) -> None:
        main_sha = "2" * 40
        association_path = f"/repos/{REPOSITORY}/commits/{main_sha}/pulls"
        commit_path = f"/repos/{REPOSITORY}/git/commits/{main_sha}"
        pr_path = f"/repos/{REPOSITORY}/pulls/430"
        api = FakeApi(
            {
                (association_path, (("per_page", 100),)): [],
                (commit_path, ()): {
                    "message": "chore(main): release 0.15.4 (#430)"
                },
                (pr_path, ()): {
                    "number": 430,
                    "base": {"ref": "other"},
                    "head": {"ref": PROMOTION.RELEASE_BRANCH},
                },
            }
        )

        with self.assertRaisesRegex(
            PROMOTION.PromotionError, "does not resolve to the release-please PR"
        ):
            PROMOTION._release_context(api, REPOSITORY, main_sha)

    def test_release_context_rejects_an_old_release_pr_from_the_title(self) -> None:
        main_sha = "2" * 40
        association_path = f"/repos/{REPOSITORY}/commits/{main_sha}/pulls"
        commit_path = f"/repos/{REPOSITORY}/git/commits/{main_sha}"
        pr_path = f"/repos/{REPOSITORY}/pulls/430"
        api = FakeApi(
            {
                (association_path, (("per_page", 100),)): [],
                (commit_path, ()): {
                    "message": "chore(main): release 0.15.4 (#430)"
                },
                (pr_path, ()): {
                    "number": 430,
                    "state": "closed",
                    "merged_at": "2026-08-01T00:00:00Z",
                    "merge_commit_sha": "4" * 40,
                    "base": {"ref": "main"},
                    "head": {"ref": PROMOTION.RELEASE_BRANCH, "sha": "3" * 40},
                },
            }
        )

        with self.assertRaisesRegex(
            PROMOTION.PromotionError,
            "release merge proof failed",
        ):
            PROMOTION._release_context(api, REPOSITORY, main_sha)

    def test_conclusion_check_suite_maps_to_exact_ci_run(self) -> None:
        path = f"/repos/{REPOSITORY}/actions/workflows/{PROMOTION.CI_WORKFLOW_FILE}/runs"
        params = {
            "event": "pull_request",
            "head_sha": HEAD_SHA,
            "status": "success",
            "per_page": 100,
        }
        exact = {
            "id": 7,
            "run_attempt": 2,
            "workflow_id": 9,
            "check_suite_id": 11,
            "path": PROMOTION.CI_WORKFLOW_PATH,
            "event": "pull_request",
            "status": "completed",
            "conclusion": "success",
            "head_branch": PROMOTION.RELEASE_BRANCH,
            "head_sha": HEAD_SHA,
            "repository": {"id": 2, "full_name": REPOSITORY},
            "head_repository": {
                "id": 2,
                "full_name": REPOSITORY,
                "fork": False,
            },
        }
        wrong_suite = {**exact, "id": 8, "check_suite_id": 12}
        api = FakeApi(
            {
                (path, tuple(sorted(params.items()))): {
                    "workflow_runs": [wrong_suite, exact]
                }
            }
        )

        resolved = PROMOTION._source_run(
            api, REPOSITORY, 2, HEAD_SHA, check_suite_id=11
        )

        self.assertEqual(resolved["id"], 7)

    def test_source_run_never_falls_back_to_workflow_name(self) -> None:
        path = f"/repos/{REPOSITORY}/actions/workflows/{PROMOTION.CI_WORKFLOW_FILE}/runs"
        params = {
            "event": "pull_request",
            "head_sha": HEAD_SHA,
            "status": "success",
            "per_page": 100,
        }
        api = FakeApi(
            {
                (path, tuple(sorted(params.items()))): {
                    "workflow_runs": [
                        {
                            "id": 7,
                            "run_attempt": 1,
                            "workflow_id": 9,
                            "check_suite_id": 99,
                            "name": "CI",
                        }
                    ]
                }
            }
        )
        with self.assertRaisesRegex(PROMOTION.PromotionError, "maps to 0"):
            PROMOTION._source_run(api, REPOSITORY, 2, HEAD_SHA, check_suite_id=11)

    def test_main_plan_binds_exact_run_and_receipt_artifact(self) -> None:
        result = PROMOTION.GateResult(
            "validated",
            "ok",
            receipt={"validated_assets_artifact": {"id": 50}},
            receipt_artifact={
                "id": 40,
                "name": "validated-release-receipt-7-2",
                "digest": "sha256:" + "a" * 64,
                "size_in_bytes": 123,
            },
            main_sha="2" * 40,
            main_tree_sha="3" * 40,
            main_run_id=100,
            main_run_attempt=4,
        )
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "plan.json"
            PROMOTION.write_plan(path, result)
            plan = PROMOTION.read_plan(path)
        self.assertEqual(plan["main_run"]["run_id"], 100)
        self.assertEqual(plan["receipt_artifact"]["id"], 40)

    def test_semantic_reruns_ignore_wrapper_identity_but_not_asset_hashes(self) -> None:
        receipt = {
            "assets": [{"name": "a", "sha256": "1" * 64}],
            "check_suite_id": 1,
            "head_sha": "1" * 40,
            "paths": ["version.txt"],
            "pr_number": 4,
            "repository": REPOSITORY,
            "repository_id": 2,
            "run_attempt": 1,
            "run_id": 3,
            "tree_sha": "2" * 40,
            "version": "0.15.4",
            "workflow_id": 5,
        }
        rerun = {**receipt, "observer": {"run_id": 9}}
        self.assertEqual(
            PROMOTION._receipt_semantics(receipt),
            PROMOTION._receipt_semantics(rerun),
        )
        conflicting = {**receipt, "assets": [{"name": "a", "sha256": "2" * 64}]}
        self.assertNotEqual(
            PROMOTION._receipt_semantics(receipt),
            PROMOTION._receipt_semantics(conflicting),
        )

    def test_newest_terminal_observer_attempt_cannot_fall_back_to_old_success(self) -> None:
        path = (
            f"/repos/{REPOSITORY}/actions/workflows/"
            f"{PROMOTION.OBSERVER_WORKFLOW_FILE}/runs"
        )
        params = {"event": "workflow_run", "per_page": 100}
        base = {
            "id": 70,
            "display_title": "release-candidate-observer source 7/2",
            "path": PROMOTION.OBSERVER_WORKFLOW_PATH,
            "event": "workflow_run",
            "status": "completed",
            "repository": {"id": 2, "full_name": REPOSITORY},
        }
        api = FakeApi(
            {
                (path, tuple(sorted(params.items()))): {
                    "workflow_runs": [
                        {**base, "run_attempt": 1, "conclusion": "success"},
                        {**base, "run_attempt": 2, "conclusion": "failure"},
                    ]
                }
            }
        )
        with self.assertRaisesRegex(PROMOTION.PromotionError, "concluded 'failure'"):
            PROMOTION._latest_observer_run(api, REPOSITORY, 2, 7, 2)

    def test_receipt_locator_has_a_hard_candidate_cap(self) -> None:
        name = "validated-release-receipt-7-2"
        path = f"/repos/{REPOSITORY}/actions/artifacts"
        params = {"name": name, "per_page": 100, "page": 1}
        api = FakeApi(
            {
                (path, tuple(sorted(params.items()))): {
                    "total_count": PROMOTION.MAX_RECEIPT_CANDIDATES + 1,
                    "artifacts": [],
                }
            }
        )
        with self.assertRaisesRegex(PROMOTION.PromotionError, "candidate count"):
            PROMOTION._artifact_pages(api, REPOSITORY, name)

    def test_receipt_locator_rejects_incomplete_or_unstable_pagination(self) -> None:
        name = "validated-release-receipt-7-2"
        path = f"/repos/{REPOSITORY}/actions/artifacts"
        first = {"name": name, "id": 1}
        incomplete = FakeApi(
            {
                (
                    path,
                    (("name", name), ("page", 1), ("per_page", 100)),
                ): {"total_count": 2, "artifacts": [first]},
            }
        )
        with self.assertRaisesRegex(PROMOTION.PromotionError, "does not match pages"):
            PROMOTION._artifact_pages(incomplete, REPOSITORY, name)

        full_page = [dict(first, id=index + 1) for index in range(100)]
        unstable = FakeApi(
            {
                (
                    path,
                    (("name", name), ("page", 1), ("per_page", 100)),
                ): {"total_count": 20, "artifacts": full_page},
                (
                    path,
                    (("name", name), ("page", 2), ("per_page", 100)),
                ): {"total_count": 19, "artifacts": []},
            }
        )
        with self.assertRaisesRegex(PROMOTION.PromotionError, "changed during pagination"):
            PROMOTION._artifact_pages(unstable, REPOSITORY, name)

    def test_consistent_old_attempt_is_ignored_for_latest_successful_rerun(self) -> None:
        source = {
            "id": 7,
            "run_attempt": 2,
            "workflow_id": 9,
            "check_suite_id": 11,
        }
        base_receipt = {
            "assets": [{"name": "a", "sha256": "1" * 64}],
            "check_suite_id": 11,
            "head_sha": HEAD_SHA,
            "paths": ["version.txt"],
            "pr_number": 4,
            "repository": REPOSITORY,
            "repository_id": 2,
            "run_attempt": 2,
            "run_id": 7,
            "tree_sha": "2" * 40,
            "version": "0.15.4",
            "workflow_id": 9,
        }
        old = {**base_receipt, "observer": {"run_id": 70, "run_attempt": 1}}
        newest = {**base_receipt, "observer": {"run_id": 70, "run_attempt": 2}}
        artifacts = [
            {
                "id": 1,
                "name": "validated-release-receipt-7-2",
                "expired": False,
                "workflow_run": {"id": 70},
            },
            {
                "id": 2,
                "name": "validated-release-receipt-7-2",
                "expired": False,
                "workflow_run": {"id": 70},
            },
        ]
        with tempfile.TemporaryDirectory() as raw, mock.patch.object(
            PROMOTION,
            "_latest_observer_run",
            return_value={"id": 70, "run_attempt": 2},
        ), mock.patch.object(
            PROMOTION, "_artifact_pages", return_value=artifacts
        ), mock.patch.object(
            PROMOTION,
            "_download_receipt",
            side_effect=[(newest, "sha256:" + "b" * 64), (old, "sha256:" + "a" * 64)],
        ), mock.patch.object(PROMOTION, "_validate_observer_and_assets"):
            receipt, artifact = PROMOTION._find_receipt(
                mock.Mock(),
                REPOSITORY,
                2,
                source,
                4,
                HEAD_SHA,
                "2" * 40,
                Path(raw),
            )
        self.assertEqual(receipt["observer"]["run_attempt"], 2)
        self.assertEqual(artifact["id"], 2)

    def test_consume_main_receipt_binds_exact_successful_main_run(self) -> None:
        receipt = {
            "assets": [{"name": "a", "sha256": "1" * 64}],
            "check_suite_id": 11,
            "head_sha": HEAD_SHA,
            "paths": ["version.txt"],
            "pr_number": 4,
            "repository": REPOSITORY,
            "repository_id": 2,
            "run_attempt": 2,
            "run_id": 7,
            "tree_sha": "2" * 40,
            "version": "0.15.4",
            "workflow_id": 9,
        }
        artifact = {
            "digest": "sha256:" + "a" * 64,
            "id": 40,
            "name": "validated-release-receipt-7-2",
            "size_in_bytes": 123,
        }
        result = PROMOTION.GateResult(
            "validated",
            "ok",
            receipt=receipt,
            receipt_artifact=artifact,
            main_sha="3" * 40,
            main_tree_sha="4" * 40,
        )
        main_run = {"id": 100, "run_attempt": 3}
        plan = {
            "schema_version": 1,
            "main_sha": "3" * 40,
            "main_tree_sha": "4" * 40,
            "main_run": {
                "run_id": 100,
                "run_attempt": 3,
                "workflow_path": PROMOTION.CI_WORKFLOW_PATH,
            },
            "receipt": receipt,
            "receipt_artifact": artifact,
        }
        api = FakeApi(
            {
                (f"/repos/{REPOSITORY}", ()): {
                    "id": 2,
                    "full_name": REPOSITORY,
                }
            }
        )
        with tempfile.TemporaryDirectory() as raw, mock.patch.object(
            PROMOTION, "verify_main", return_value=result
        ), mock.patch.object(
            PROMOTION, "_main_ci_run", return_value=main_run
        ), mock.patch.object(PROMOTION, "_download_main_plan", return_value=plan):
            consumed, closed_plan = PROMOTION.consume_main_receipt(
                api,
                REPOSITORY,
                "3" * 40,
                Path(raw),
                main_run_id=100,
                main_run_attempt=3,
            )
        self.assertEqual(consumed.state, "validated")
        self.assertEqual(consumed.main_run_id, 100)
        self.assertEqual(closed_plan, plan)

    def test_consume_main_receipt_rejects_plan_drift(self) -> None:
        result = PROMOTION.GateResult(
            "validated",
            "ok",
            receipt={
                "assets": [],
                "check_suite_id": 1,
                "head_sha": HEAD_SHA,
                "paths": ["version.txt"],
                "pr_number": 4,
                "repository": REPOSITORY,
                "repository_id": 2,
                "run_attempt": 1,
                "run_id": 7,
                "tree_sha": "2" * 40,
                "version": "0.15.4",
                "workflow_id": 9,
            },
            receipt_artifact={
                "digest": "sha256:" + "a" * 64,
                "id": 40,
                "name": "receipt",
                "size_in_bytes": 123,
            },
            main_sha="3" * 40,
            main_tree_sha="4" * 40,
        )
        drifted = {
            "main_sha": "5" * 40,
            "main_tree_sha": "4" * 40,
            "main_run": {"run_id": 100, "run_attempt": 3},
            "receipt": result.receipt,
            "receipt_artifact": result.receipt_artifact,
        }
        api = FakeApi(
            {
                (f"/repos/{REPOSITORY}", ()): {
                    "id": 2,
                    "full_name": REPOSITORY,
                }
            }
        )
        with tempfile.TemporaryDirectory() as raw, mock.patch.object(
            PROMOTION, "verify_main", return_value=result
        ), mock.patch.object(
            PROMOTION, "_main_ci_run", return_value={"id": 100, "run_attempt": 3}
        ), mock.patch.object(
            PROMOTION, "_download_main_plan", return_value=drifted
        ):
            consumed, closed_plan = PROMOTION.consume_main_receipt(
                api, REPOSITORY, "3" * 40, Path(raw)
            )
        self.assertEqual(consumed.state, "blocked")
        self.assertIsNone(closed_plan)

    def test_partial_rerun_accepts_attempt_one_receipt_after_attempt_two_success(self) -> None:
        main_sha = "3" * 40
        receipt = {
            "assets": [{"name": "a", "sha256": "1" * 64}],
            "check_suite_id": 11,
            "head_sha": HEAD_SHA,
            "paths": ["version.txt"],
            "pr_number": 4,
            "repository": REPOSITORY,
            "repository_id": 2,
            "run_attempt": 2,
            "run_id": 7,
            "tree_sha": "2" * 40,
            "version": "0.15.4",
            "workflow_id": 9,
        }
        receipt_artifact = {
            "digest": "sha256:" + "a" * 64,
            "id": 40,
            "name": "validated-release-receipt-7-2",
            "size_in_bytes": 123,
        }
        result = PROMOTION.GateResult(
            "validated",
            "ok",
            receipt=receipt,
            receipt_artifact=receipt_artifact,
            main_sha=main_sha,
            main_tree_sha="4" * 40,
        )
        plan = {
            "schema_version": 1,
            "main_sha": main_sha,
            "main_tree_sha": "4" * 40,
            "main_run": {
                "run_id": 100,
                "run_attempt": 1,
                "workflow_path": PROMOTION.CI_WORKFLOW_PATH,
            },
            "receipt": receipt,
            "receipt_artifact": receipt_artifact,
        }
        main_receipt = {
            "id": 60,
            "name": "main-release-promotion-receipt-100-1",
            "workflow_run": {"id": 100},
        }
        artifacts_path = f"/repos/{REPOSITORY}/actions/runs/100/artifacts"
        artifacts_params = {"per_page": 100, "page": 1}
        jobs_path = f"/repos/{REPOSITORY}/actions/runs/100/attempts/1/jobs"
        jobs_params = {"per_page": 100, "page": 1}
        api = FakeApi(
            {
                (f"/repos/{REPOSITORY}", ()): {
                    "id": 2,
                    "full_name": REPOSITORY,
                },
                (artifacts_path, tuple(sorted(artifacts_params.items()))): {
                    "total_count": 1,
                    "artifacts": [main_receipt],
                },
                (jobs_path, tuple(sorted(jobs_params.items()))): {
                    "total_count": 1,
                    "jobs": [
                        {
                            "id": 70,
                            "name": "detect-changes",
                            "run_id": 100,
                            "run_attempt": 1,
                            "head_sha": main_sha,
                            "status": "completed",
                            "conclusion": "success",
                        }
                    ],
                },
            }
        )
        with tempfile.TemporaryDirectory() as raw, mock.patch.object(
            PROMOTION, "verify_main", return_value=result
        ), mock.patch.object(
            PROMOTION,
            "_main_ci_run",
            return_value={"id": 100, "run_attempt": 2, "head_sha": main_sha},
        ), mock.patch.object(
            PROMOTION, "_download_main_plan_artifact", return_value=plan
        ):
            consumed, closed_plan = PROMOTION.consume_main_receipt(
                api, REPOSITORY, main_sha, Path(raw)
            )
        self.assertEqual(consumed.state, "validated")
        self.assertEqual(consumed.main_run_attempt, 2)
        self.assertEqual(closed_plan, plan)

    def test_main_receipt_rejects_failed_detect_changes_origin(self) -> None:
        main_sha = "3" * 40
        artifact = {
            "id": 60,
            "name": "main-release-promotion-receipt-100-1",
            "workflow_run": {"id": 100},
        }
        plan = {
            "schema_version": 1,
            "main_sha": main_sha,
            "main_tree_sha": "4" * 40,
            "main_run": {
                "run_id": 100,
                "run_attempt": 1,
                "workflow_path": PROMOTION.CI_WORKFLOW_PATH,
            },
            "receipt": {},
            "receipt_artifact": {},
        }
        api = FakeApi(
            {
                (
                    f"/repos/{REPOSITORY}/actions/runs/100/attempts/1/jobs",
                    (("page", 1), ("per_page", 100)),
                ): {
                    "total_count": 1,
                    "jobs": [
                        {
                            "id": 70,
                            "name": "detect-changes",
                            "run_id": 100,
                            "run_attempt": 1,
                            "head_sha": main_sha,
                            "status": "completed",
                            "conclusion": "failure",
                        }
                    ],
                }
            }
        )
        with tempfile.TemporaryDirectory() as raw, mock.patch.object(
            PROMOTION, "_main_receipt_artifacts", return_value=[artifact]
        ), mock.patch.object(
            PROMOTION, "_download_main_plan_artifact", return_value=plan
        ):
            with self.assertRaisesRegex(PROMOTION.PromotionError, "not an exact success"):
                PROMOTION._download_main_plan(
                    api,
                    REPOSITORY,
                    2,
                    {"id": 100, "run_attempt": 2, "head_sha": main_sha},
                    Path(raw),
                )

    def test_main_receipt_reruns_must_have_consistent_semantics(self) -> None:
        main_sha = "3" * 40
        receipt = {
            "assets": [{"name": "a", "sha256": "1" * 64}],
            "check_suite_id": 11,
            "head_sha": HEAD_SHA,
            "paths": ["version.txt"],
            "pr_number": 4,
            "repository": REPOSITORY,
            "repository_id": 2,
            "run_attempt": 1,
            "run_id": 7,
            "tree_sha": "2" * 40,
            "version": "0.15.4",
            "workflow_id": 9,
        }
        artifacts = [
            {
                "id": attempt,
                "name": f"main-release-promotion-receipt-100-{attempt}",
                "workflow_run": {"id": 100},
            }
            for attempt in (1, 2)
        ]
        plans = [
            {
                "schema_version": 1,
                "main_sha": main_sha,
                "main_tree_sha": digit * 40,
                "main_run": {
                    "run_id": 100,
                    "run_attempt": attempt,
                    "workflow_path": PROMOTION.CI_WORKFLOW_PATH,
                },
                "receipt": {**receipt, "observer": {"run_id": attempt}},
                "receipt_artifact": {"id": attempt},
            }
            for attempt, digit in ((1, "4"), (2, "5"))
        ]
        responses: dict[tuple[str, tuple], object] = {}
        for attempt in (1, 2):
            responses[
                (
                    f"/repos/{REPOSITORY}/actions/runs/100/attempts/{attempt}/jobs",
                    (("page", 1), ("per_page", 100)),
                )
            ] = {
                "total_count": 1,
                "jobs": [
                    {
                        "id": 70 + attempt,
                        "name": "detect-changes",
                        "run_id": 100,
                        "run_attempt": attempt,
                        "head_sha": main_sha,
                        "status": "completed",
                        "conclusion": "success",
                    }
                ],
            }
        api = FakeApi(responses)
        with tempfile.TemporaryDirectory() as raw, mock.patch.object(
            PROMOTION, "_main_receipt_artifacts", return_value=artifacts
        ), mock.patch.object(
            PROMOTION, "_download_main_plan_artifact", side_effect=plans
        ):
            with self.assertRaisesRegex(PROMOTION.PromotionError, "conflicting semantics"):
                PROMOTION._download_main_plan(
                    api,
                    REPOSITORY,
                    2,
                    {"id": 100, "run_attempt": 2, "head_sha": main_sha},
                    Path(raw),
                )

    def test_main_receipt_selects_newest_consistent_origin(self) -> None:
        main_sha = "3" * 40
        receipt = {
            "assets": [{"name": "a", "sha256": "1" * 64}],
            "check_suite_id": 11,
            "head_sha": HEAD_SHA,
            "paths": ["version.txt"],
            "pr_number": 4,
            "repository": REPOSITORY,
            "repository_id": 2,
            "run_attempt": 1,
            "run_id": 7,
            "tree_sha": "2" * 40,
            "version": "0.15.4",
            "workflow_id": 9,
        }
        artifacts = [
            {
                "id": attempt,
                "name": f"main-release-promotion-receipt-100-{attempt}",
                "workflow_run": {"id": 100},
            }
            for attempt in (1, 2)
        ]
        plans = [
            {
                "schema_version": 1,
                "main_sha": main_sha,
                "main_tree_sha": "4" * 40,
                "main_run": {
                    "run_id": 100,
                    "run_attempt": attempt,
                    "workflow_path": PROMOTION.CI_WORKFLOW_PATH,
                },
                "receipt": {**receipt, "observer": {"run_id": attempt}},
                "receipt_artifact": {"id": attempt},
            }
            for attempt in (1, 2)
        ]
        responses: dict[tuple[str, tuple], object] = {}
        for attempt in (1, 2):
            responses[
                (
                    f"/repos/{REPOSITORY}/actions/runs/100/attempts/{attempt}/jobs",
                    (("page", 1), ("per_page", 100)),
                )
            ] = {
                "total_count": 1,
                "jobs": [
                    {
                        "id": 70 + attempt,
                        "name": "detect-changes",
                        "run_id": 100,
                        "run_attempt": attempt,
                        "head_sha": main_sha,
                        "status": "completed",
                        "conclusion": "success",
                    }
                ],
            }
        api = FakeApi(responses)
        with tempfile.TemporaryDirectory() as raw, mock.patch.object(
            PROMOTION, "_main_receipt_artifacts", return_value=artifacts
        ), mock.patch.object(
            PROMOTION, "_download_main_plan_artifact", side_effect=plans
        ):
            selected = PROMOTION._download_main_plan(
                api,
                REPOSITORY,
                2,
                {"id": 100, "run_attempt": 2, "head_sha": main_sha},
                Path(raw),
            )
        self.assertEqual(selected["main_run"]["run_attempt"], 2)

    def test_main_receipt_future_attempt_and_malformed_name_fail_closed(self) -> None:
        main_sha = "3" * 40
        future_artifact = {
            "id": 60,
            "name": "main-release-promotion-receipt-100-3",
            "workflow_run": {"id": 100},
        }
        future_plan = {
            "schema_version": 1,
            "main_sha": main_sha,
            "main_tree_sha": "4" * 40,
            "main_run": {
                "run_id": 100,
                "run_attempt": 3,
                "workflow_path": PROMOTION.CI_WORKFLOW_PATH,
            },
            "receipt": {},
            "receipt_artifact": {},
        }
        with tempfile.TemporaryDirectory() as raw, mock.patch.object(
            PROMOTION, "_main_receipt_artifacts", return_value=[future_artifact]
        ), mock.patch.object(
            PROMOTION, "_download_main_plan_artifact", return_value=future_plan
        ):
            with self.assertRaisesRegex(PROMOTION.PromotionError, "future run attempt"):
                PROMOTION._download_main_plan(
                    mock.Mock(),
                    REPOSITORY,
                    2,
                    {"id": 100, "run_attempt": 2, "head_sha": main_sha},
                    Path(raw),
                )

        artifacts_path = f"/repos/{REPOSITORY}/actions/runs/100/artifacts"
        malformed_api = FakeApi(
            {
                (
                    artifacts_path,
                    (("page", 1), ("per_page", 100)),
                ): {
                    "total_count": 1,
                    "artifacts": [
                        {
                            "id": 61,
                            "name": "main-release-promotion-receipt-100-bogus",
                        }
                    ],
                }
            }
        )
        with self.assertRaisesRegex(PROMOTION.PromotionError, "name is malformed"):
            PROMOTION._main_receipt_artifacts(malformed_api, REPOSITORY, 100)

    def test_main_receipt_and_job_pagination_fail_closed_on_short_pages(self) -> None:
        artifacts_path = f"/repos/{REPOSITORY}/actions/runs/100/artifacts"
        jobs_path = f"/repos/{REPOSITORY}/actions/runs/100/attempts/1/jobs"
        params = (("page", 1), ("per_page", 100))
        api = FakeApi(
            {
                (artifacts_path, params): {
                    "total_count": 2,
                    "artifacts": [
                        {
                            "id": 1,
                            "name": "main-release-promotion-receipt-100-1",
                        }
                    ],
                },
                (jobs_path, params): {
                    "total_count": 2,
                    "jobs": [
                        {
                            "id": 2,
                            "name": "detect-changes",
                            "run_id": 100,
                            "run_attempt": 1,
                            "head_sha": "3" * 40,
                            "status": "completed",
                            "conclusion": "success",
                        }
                    ],
                },
            }
        )
        with self.assertRaisesRegex(PROMOTION.PromotionError, "does not match pages"):
            PROMOTION._main_receipt_artifacts(api, REPOSITORY, 100)
        with self.assertRaisesRegex(PROMOTION.PromotionError, "does not match pages"):
            PROMOTION._detect_changes_job(api, REPOSITORY, 100, 1, "3" * 40)

    def test_main_ci_run_requires_exact_attempt_and_control_plane(self) -> None:
        run = {
            "id": 100,
            "run_attempt": 3,
            "workflow_id": 9,
            "path": PROMOTION.CI_WORKFLOW_PATH,
            "event": "push",
            "status": "completed",
            "conclusion": "success",
            "head_branch": "main",
            "head_sha": "3" * 40,
            "repository": {"id": 2, "full_name": REPOSITORY},
            "head_repository": {"id": 2, "full_name": REPOSITORY},
        }
        api = FakeApi(
            {
                (f"/repos/{REPOSITORY}/actions/runs/100", ()): run,
                (f"/repos/{REPOSITORY}/actions/workflows/9", ()): {
                    "id": 9,
                    "path": PROMOTION.CI_WORKFLOW_PATH,
                    "state": "active",
                },
            }
        )
        resolved = PROMOTION._main_ci_run(
            api, REPOSITORY, 2, "3" * 40, 100, 3
        )
        self.assertEqual(resolved["id"], 100)
        with self.assertRaisesRegex(PROMOTION.PromotionError, "attempt differs"):
            PROMOTION._main_ci_run(api, REPOSITORY, 2, "3" * 40, 100, 2)

        list_path = (
            f"/repos/{REPOSITORY}/actions/workflows/"
            f"{PROMOTION.CI_WORKFLOW_FILE}/runs"
        )
        list_params = {"event": "push", "head_sha": "3" * 40, "per_page": 100}
        discovery_api = FakeApi(
            {
                (list_path, tuple(sorted(list_params.items()))): {
                    "workflow_runs": [run]
                },
                (f"/repos/{REPOSITORY}/actions/runs/100", ()): run,
                (f"/repos/{REPOSITORY}/actions/workflows/9", ()): {
                    "id": 9,
                    "path": PROMOTION.CI_WORKFLOW_PATH,
                    "state": "active",
                },
            }
        )
        discovered = PROMOTION._main_ci_run(
            discovery_api, REPOSITORY, 2, "3" * 40, None, None
        )
        self.assertEqual(discovered["run_attempt"], 3)

    def test_verify_main_ci_waits_for_the_exact_run_then_succeeds(self) -> None:
        main_sha = "3" * 40
        list_path = (
            f"/repos/{REPOSITORY}/actions/workflows/"
            f"{PROMOTION.CI_WORKFLOW_FILE}/runs"
        )
        run_path = f"/repos/{REPOSITORY}/actions/runs/100"
        workflow_path = f"/repos/{REPOSITORY}/actions/workflows/9"
        run = {
            "id": 100,
            "run_attempt": 2,
            "workflow_id": 9,
            "path": PROMOTION.CI_WORKFLOW_PATH,
            "event": "push",
            "status": "completed",
            "conclusion": "success",
            "head_branch": "main",
            "head_sha": main_sha,
            "repository": {"id": 2, "full_name": REPOSITORY},
            "head_repository": {
                "id": 2,
                "full_name": REPOSITORY,
                "fork": False,
            },
        }
        pending = {**run, "status": "in_progress", "conclusion": None}
        states = [pending, run]
        inventories = [
            {"total_count": 0, "workflow_runs": []},
            {"total_count": 1, "workflow_runs": [{"id": 100}]},
            {"total_count": 1, "workflow_runs": [{"id": 100}]},
        ]

        def get_json(path, *, params=None):
            if path == f"/repos/{REPOSITORY}":
                return {"id": 2, "full_name": REPOSITORY}
            if path == list_path:
                self.assertEqual(
                    params,
                    {
                        "event": "push",
                        "head_sha": main_sha,
                        "per_page": 100,
                    },
                )
                return inventories.pop(0)
            if path == run_path:
                return states.pop(0)
            if path == workflow_path:
                return {
                    "id": 9,
                    "path": PROMOTION.CI_WORKFLOW_PATH,
                    "state": "active",
                }
            self.fail(f"unexpected API path: {path}")

        api = mock.Mock()
        api.get_json.side_effect = get_json
        clock = [0.0]
        sleeps: list[float] = []

        def sleep(seconds: float) -> None:
            sleeps.append(seconds)
            clock[0] += seconds

        resolved = PROMOTION.verify_main_ci(
            api,
            REPOSITORY,
            main_sha,
            wait_seconds=45,
            sleep=sleep,
            monotonic=lambda: clock[0],
        )

        self.assertEqual(resolved["id"], 100)
        self.assertEqual(resolved["run_attempt"], 2)
        self.assertEqual(sleeps, [15, 15])

    def test_verify_main_ci_rejects_ambiguity_and_terminal_failure(self) -> None:
        main_sha = "3" * 40
        list_path = (
            f"/repos/{REPOSITORY}/actions/workflows/"
            f"{PROMOTION.CI_WORKFLOW_FILE}/runs"
        )
        params = {"event": "push", "head_sha": main_sha, "per_page": 100}
        ambiguous = FakeApi(
            {
                (f"/repos/{REPOSITORY}", ()): {
                    "id": 2,
                    "full_name": REPOSITORY,
                },
                (list_path, tuple(sorted(params.items()))): {
                    "total_count": 2,
                    "workflow_runs": [{"id": 100}, {"id": 101}],
                },
            }
        )
        with self.assertRaisesRegex(PROMOTION.PromotionError, "exactly one"):
            PROMOTION.verify_main_ci(ambiguous, REPOSITORY, main_sha)

        failed_run = {
            "id": 100,
            "run_attempt": 1,
            "workflow_id": 9,
            "path": PROMOTION.CI_WORKFLOW_PATH,
            "event": "push",
            "status": "completed",
            "conclusion": "failure",
            "head_branch": "main",
            "head_sha": main_sha,
            "repository": {"id": 2, "full_name": REPOSITORY},
            "head_repository": {
                "id": 2,
                "full_name": REPOSITORY,
                "fork": False,
            },
        }
        terminal = FakeApi(
            {
                (f"/repos/{REPOSITORY}", ()): {
                    "id": 2,
                    "full_name": REPOSITORY,
                },
                (list_path, tuple(sorted(params.items()))): {
                    "total_count": 1,
                    "workflow_runs": [{"id": 100}],
                },
                (f"/repos/{REPOSITORY}/actions/runs/100", ()): failed_run,
                (f"/repos/{REPOSITORY}/actions/workflows/9", ()): {
                    "id": 9,
                    "path": PROMOTION.CI_WORKFLOW_PATH,
                    "state": "active",
                },
            }
        )
        with self.assertRaisesRegex(PROMOTION.PromotionError, "conclusion 'failure'"):
            PROMOTION.verify_main_ci(terminal, REPOSITORY, main_sha, wait_seconds=30)

    def test_verify_main_ci_rejects_repository_head_and_workflow_drift(self) -> None:
        main_sha = "3" * 40
        list_path = (
            f"/repos/{REPOSITORY}/actions/workflows/"
            f"{PROMOTION.CI_WORKFLOW_FILE}/runs"
        )
        params = {"event": "push", "head_sha": main_sha, "per_page": 100}
        run = {
            "id": 100,
            "run_attempt": 1,
            "workflow_id": 9,
            "path": PROMOTION.CI_WORKFLOW_PATH,
            "event": "push",
            "status": "completed",
            "conclusion": "success",
            "head_branch": "main",
            "head_sha": main_sha,
            "repository": {"id": 2, "full_name": REPOSITORY},
            "head_repository": {
                "id": 2,
                "full_name": REPOSITORY,
                "fork": False,
            },
        }
        cases = [
            (
                "repository",
                {**run, "repository": {"id": 3, "full_name": REPOSITORY}},
                {"id": 9, "path": PROMOTION.CI_WORKFLOW_PATH, "state": "active"},
                "main CI repository differs",
            ),
            (
                "head repository",
                {**run, "head_repository": {"id": 3, "full_name": REPOSITORY, "fork": False}},
                {"id": 9, "path": PROMOTION.CI_WORKFLOW_PATH, "state": "active"},
                "main CI head repository differs",
            ),
            (
                "workflow",
                run,
                {"id": 9, "path": PROMOTION.CI_WORKFLOW_PATH, "state": "disabled_manually"},
                "control-plane identity mismatch",
            ),
        ]
        for label, candidate, workflow, expected in cases:
            with self.subTest(label=label):
                api = FakeApi(
                    {
                        (f"/repos/{REPOSITORY}", ()): {
                            "id": 2,
                            "full_name": REPOSITORY,
                        },
                        (list_path, tuple(sorted(params.items()))): {
                            "total_count": 1,
                            "workflow_runs": [{"id": 100}],
                        },
                        (f"/repos/{REPOSITORY}/actions/runs/100", ()): candidate,
                        (f"/repos/{REPOSITORY}/actions/workflows/9", ()): workflow,
                    }
                )
                with self.assertRaisesRegex(PROMOTION.PromotionError, expected):
                    PROMOTION.verify_main_ci(api, REPOSITORY, main_sha)

    def test_verify_main_ci_cli_prints_validated_run_identity(self) -> None:
        run = {
            "id": 100,
            "run_attempt": 3,
            "head_sha": "3" * 40,
        }
        with mock.patch.object(PROMOTION.VALIDATOR, "GitHubApi"), mock.patch.object(
            PROMOTION, "verify_main_ci", return_value=run
        ), mock.patch("sys.stdout", new_callable=io.StringIO) as stdout:
            result = PROMOTION._main(
                [
                    "verify-main-ci",
                    "--repository",
                    REPOSITORY,
                    "--sha",
                    "3" * 40,
                    "--wait-seconds",
                    "1200",
                ]
            )
        self.assertEqual(result, 0)
        self.assertEqual(
            json.loads(stdout.getvalue()),
            {
                "head_sha": "3" * 40,
                "run_attempt": 3,
                "run_id": 100,
                "status": "validated",
                "workflow_path": PROMOTION.CI_WORKFLOW_PATH,
            },
        )


if __name__ == "__main__":
    unittest.main(verbosity=2)
