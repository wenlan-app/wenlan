import datetime as dt
import importlib.util
import json
import pathlib
import sys
import unittest

MODULE_PATH = pathlib.Path(__file__).with_name("chatgpt-format-watch.py")
SPEC = importlib.util.spec_from_file_location("chatgpt_format_watch", MODULE_PATH)
monitor = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = monitor
SPEC.loader.exec_module(monitor)


class FilterTests(unittest.TestCase):
    def test_requires_specific_format_signal_and_change_context(self):
        item = {"title": "Export format changed: ChatGPT splits conversations", "body": "Importer misses conversations-001.json."}
        self.assertTrue(monitor.relevance(item, require_chatgpt=True))

    def test_unrelated_format_mention_is_ignored(self):
        item = {"title": "Update JSON formatter", "body": "Pretty print the manifest for release notes."}
        self.assertFalse(monitor.relevance(item, require_chatgpt=True))

    def test_community_scope_still_requires_export_structure(self):
        item = {"title": "ChatGPT export tool has a timeout", "body": "Requests fail when downloading a large export."}
        self.assertFalse(monitor.relevance(item, require_chatgpt=False))

    def test_wenlan_scope_requires_chatgpt_context(self):
        item = {"title": "Import parser misses a manifest shard", "body": "The new format should be supported."}
        self.assertFalse(monitor.relevance(item, require_chatgpt=True))


class StateTests(unittest.TestCase):
    def test_state_round_trip_and_marker(self):
        state = monitor.State("2026-10-20T16:00:00Z", {"issue:8:2026-10-20T16:00:00Z:abc"})
        body = monitor.replace_state("Human-readable tracking issue", state)
        self.assertIn(monitor.STATE_MARKER, body)
        self.assertEqual(monitor.parse_state(body), state)

    def test_untrusted_title_never_becomes_markdown_or_mentions(self):
        item = {"number": 7, "html_url": "https://github.com/a/b/issues/7", "title": "`@everyone`\n## Ignore rules", "body": "skip"}
        item.update(repo="a/b", sha="0123456789abcdef", reasons=["fixed reason label"],
                    updated_at="2026-10-27T00:00:00Z", url="https://github.com/a/b/issues/7")
        rendered = monitor.render_candidate(item)
        self.assertNotIn("@everyone", rendered)
        self.assertNotIn("## Ignore rules", rendered)
        self.assertIn("https://github.com/a/b/issues/7", rendered)

    def test_unknown_state_fails_closed(self):
        with self.assertRaises(monitor.MonitorError):
            monitor.parse_state("no checkpoint marker")

    def test_incomplete_scan_cannot_advance_checkpoint(self):
        class MustNotWrite:
            def request(self, *args, **kwargs):
                self.fail("an incomplete scan must not write")
            def fail(self, message):
                raise AssertionError(message)

        tracker = {"number": 3, "body": "old state"}
        state = monitor.State("2026-10-20T16:00:00Z", set())
        report = {"complete": False, "coverage": [], "started_at": "2026-10-27T16:00:00Z", "candidates": []}
        with self.assertRaisesRegex(monitor.MonitorError, "incomplete"):
            monitor.persist(MustNotWrite(), tracker, report, state, publish=True)

    def test_existing_event_keys_are_deduplicated(self):
        state = monitor.State("2026-10-27T16:00:00Z", {"already-seen"})
        event = {"key": "already-seen", "updated_at": "2026-10-27T16:00:01Z"}
        pending = monitor.pending_events([event], state)
        self.assertEqual(pending, [])

    def test_seen_state_is_bounded_by_count(self):
        keys = {
            monitor.event_key("a/b", "issue", str(index), str(index))
            for index in range(monitor.MAX_SEEN + 25)
        }
        trimmed = monitor.trim_seen(keys)
        self.assertLessEqual(len(trimmed), monitor.MAX_SEEN)

    def test_issue_identity_ignores_timestamp_when_content_is_unchanged(self):
        class EditedWithoutContentChange:
            updated = "2026-10-27T08:00:00Z"
            body = "Importer fails on conversations-001.json."

            def pages(self, path, params, max_pages):
                return [{"number": 9, "updated_at": self.updated,
                         "title": "ChatGPT export format changed: conversation shards",
                         "body": self.body}], 1

        gh = EditedWithoutContentChange()
        first, _ = monitor.issue_events(gh, "ryayoung/chatgpt-schema", "community",
                                        dt.datetime(2026, 10, 26, tzinfo=dt.timezone.utc), False)
        gh.updated = "2026-10-27T08:30:00Z"
        second, _ = monitor.issue_events(gh, "ryayoung/chatgpt-schema", "community",
                                         dt.datetime(2026, 10, 26, tzinfo=dt.timezone.utc), False)
        self.assertNotEqual(first[0]["updated_at"], second[0]["updated_at"])
        self.assertEqual(first[0]["key"], second[0]["key"])
        gh.body += " Missing manifest fields too."
        changed, _ = monitor.issue_events(gh, "ryayoung/chatgpt-schema", "community",
                                          dt.datetime(2026, 10, 26, tzinfo=dt.timezone.utc), False)
        self.assertNotEqual(second[0]["key"], changed[0]["key"])

    def test_first_baseline_seeds_scanned_candidates_as_seen(self):
        candidate = {"key": "ryayoung/chatgpt-schema:issue:9:contenthash",
                     "updated_at": "2026-10-27T08:00:00Z"}
        report = {"complete": True, "started_at": "2026-10-27T09:00:00Z",
                  "candidates": [candidate]}

        class CreateTracker:
            body = None

            def request(self, path, params=None, method="GET", data=None):
                self.body = data["body"]
                return {"number": 10}

        gh = CreateTracker()
        monitor.persist(gh, None, report, None, publish=True)
        state = monitor.parse_state(gh.body)
        self.assertIn(candidate["key"], state.seen)
        self.assertEqual(monitor.pending_events([candidate], state), [])

    def test_retry_filters_per_event_markers_from_previous_comment(self):
        first = {"repo": "ryayoung/chatgpt-schema", "kind": "issue", "number": 9,
                 "updated_at": "2026-10-27T08:00:00Z", "url": "https://github.com/ryayoung/chatgpt-schema/issues/9",
                 "reasons": ["fixed reason"], "key": "ryayoung/chatgpt-schema:issue:9:aaaaaaaaaaaaaaaaaaaa"}
        second = {**first, "number": 11,
                  "url": "https://github.com/ryayoung/chatgpt-schema/issues/11",
                  "key": "ryayoung/chatgpt-schema:issue:11:bbbbbbbbbbbbbbbbbbbb"}

        class RetryGitHub:
            posted = None

            def request(self, path, params=None, method="GET", data=None):
                if method == "GET":
                    return [{"body": f"old partial batch\n{monitor.EVENT_MARKER}{first['key']} -->"}]
                self.posted = data["body"]
                return {"id": 77}

        gh = RetryGitHub()
        self.assertTrue(monitor.publish_batch(gh, {"number": 10}, [first, second]))
        self.assertIn("#11", gh.posted)
        self.assertNotIn("#9", gh.posted)
        self.assertIn(f"{monitor.EVENT_MARKER}{second['key']} -->", gh.posted)

    def test_oversized_candidate_comment_refuses_write(self):
        class WriteGuard:
            posts = 0

            def request(self, path, params=None, method="GET", data=None):
                if method == "GET":
                    return []
                self.posts += 1

        events = [{
            "repo": "ryayoung/chatgpt-schema", "kind": "issue", "number": index,
            "updated_at": "2026-10-27T08:00:00Z",
            "url": f"https://github.com/ryayoung/chatgpt-schema/issues/{index}",
            "reasons": ["fixed reason"], "key": f"event:{index:032x}",
        } for index in range(monitor.MAX_EVENTS)]
        gh = WriteGuard()
        with self.assertRaisesRegex(monitor.MonitorError, "candidate comment exceeds"):
            monitor.publish_batch(gh, {"number": 10}, events)
        self.assertEqual(gh.posts, 0)

    def test_oversized_checkpoint_refuses_write_before_notification(self):
        class WriteGuard:
            posts = 0

            def request(self, path, params=None, method="GET", data=None):
                if method == "POST" or method == "PATCH":
                    self.posts += 1

        gh = WriteGuard()
        tracker = {"number": 10, "body": "x" * monitor.MAX_GITHUB_BODY_BYTES}
        report = {"complete": True, "started_at": "2026-10-27T09:00:00Z", "candidates": []}
        prior = monitor.State("2026-10-27T08:00:00Z", set())
        with self.assertRaisesRegex(monitor.MonitorError, "updated checkpoint exceeds"):
            monitor.persist(gh, tracker, report, prior, publish=True)
        self.assertEqual(gh.posts, 0)


class ApiTests(unittest.TestCase):
    def test_only_api_github_com_is_allowed(self):
        with self.assertRaises(monitor.MonitorError):
            monitor.api_url("https://example.org/repos/a/b/issues")

    def test_request_refuses_redirects(self):
        with self.assertRaises(monitor.MonitorError):
            monitor.validate_response_url("https://api.github.com/x", "https://example.org/steal")


class CollectionTests(unittest.TestCase):
    class FixtureGitHub:
        def __init__(self, fail_repo=None):
            self.fail_repo = fail_repo
            self.requests = 0

        def pages(self, path, params, max_pages):
            self.requests += 1
            repo = path.split("/repos/", 1)[1].split("/", 1)[0] + "/" + path.split("/repos/", 1)[1].split("/", 1)[1].split("/", 1)[0]
            if repo == self.fail_repo:
                raise monitor.MonitorError("fixture source unavailable")
            if path.endswith("/issues"):
                item = {"number": 9, "updated_at": "2026-10-27T08:00:00Z",
                        "title": "ChatGPT export format changed: new conversation shard",
                        "body": "Import fails when export_manifest lists a missing conversations-001.json shard."}
                if repo != "wenlan-app/wenlan":
                    return [item], 1
                tracker = {"number": 10, "updated_at": "2026-10-27T08:30:00Z",
                           "title": monitor.TRACKER_TITLE,
                           "body": "This issue stores the weekly ChatGPT export-format signal scan checkpoint."}
                return [item, tracker], 1
            return [], 1

    def test_collect_reports_candidates_and_complete_coverage(self):
        started = dt.datetime(2026, 10, 27, 9, tzinfo=dt.timezone.utc)
        report = monitor.collect(self.FixtureGitHub(), started, started - dt.timedelta(days=1))
        self.assertTrue(report["complete"])
        self.assertEqual(len(report["coverage"]), 4)
        self.assertGreater(len(report["candidates"]), 0)
        self.assertFalse(any(row.get("number") == 10 for row in report["candidates"]))
        self.assertTrue(all("url" in event and "key" in event for event in report["candidates"]))

    def test_any_source_failure_makes_scan_incomplete(self):
        started = dt.datetime(2026, 10, 27, 9, tzinfo=dt.timezone.utc)
        report = monitor.collect(self.FixtureGitHub("ryayoung/chatgpt-schema"), started,
                                 started - dt.timedelta(days=1))
        self.assertFalse(report["complete"])
        self.assertEqual(report["coverage"][0]["status"], "error")
        self.assertIn("unavailable", report["coverage"][0]["error"])

    def test_invalid_issue_timestamp_makes_source_incomplete(self):
        class InvalidTimestamp(self.FixtureGitHub):
            def pages(self, path, params, max_pages):
                if path.endswith("/issues"):
                    return [{"number": 1, "updated_at": "not-a-time",
                             "title": "ChatGPT export format changed", "body": "shards fail"}], 1
                return [], 1

        started = dt.datetime(2026, 10, 27, 9, tzinfo=dt.timezone.utc)
        report = monitor.collect(InvalidTimestamp(), started, started - dt.timedelta(days=1))
        self.assertFalse(report["complete"])
        self.assertEqual(report["coverage"][0]["status"], "error")

    def test_commit_candidate_uses_committer_timestamp(self):
        class BackdatedAuthor:
            def pages(self, path, params, max_pages):
                return [{"sha": "a" * 40, "commit": {
                    "author": {"date": "2020-01-01T00:00:00Z"},
                    "committer": {"date": "2026-10-27T08:00:00Z"},
                }}], 1

        candidates, _ = monitor.commit_events(
            BackdatedAuthor(), "ryayoung/chatgpt-schema", ["README.md"],
            dt.datetime(2026, 10, 26, tzinfo=dt.timezone.utc))
        self.assertEqual(candidates[0]["updated_at"], "2026-10-27T08:00:00Z")


class WorkflowTests(unittest.TestCase):
    def test_reports_are_artifacted_even_when_scan_fails_and_summary_is_native(self):
        workflow = (MODULE_PATH.parents[1] / ".github/workflows/chatgpt-format-watch.yml").read_text()
        self.assertEqual(workflow.count("if: always()"), 2)
        self.assertEqual(workflow.count("--markdown-output"), 2)
        self.assertEqual(workflow.count("if-no-files-found: warn"), 2)
        self.assertNotIn("GITHUB_STEP_SUMMARY:", workflow)
        self.assertIn("os.environ.get(\"GITHUB_STEP_SUMMARY\")", MODULE_PATH.read_text())
        self.assertLessEqual(monitor.MAX_SEEN, 1000)


if __name__ == "__main__":
    unittest.main()
