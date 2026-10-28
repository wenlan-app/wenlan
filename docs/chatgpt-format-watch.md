# ChatGPT export format signals

`.github/workflows/chatgpt-format-watch.yml` runs each Monday at 9:00 a.m.
America/Los_Angeles. GitHub Actions owns the schedule and stores candidate
reports as 14-day workflow artifacts. The scan uses only the repository's
`GITHUB_TOKEN` against `api.github.com`; it needs no ChatGPT login, export ZIP,
LLM, or additional secret.

The monitor watches issue and pull-request reports in three community parser or
schema projects and the Wenlan repository. It also watches selected parser,
schema, and fixture paths for commits. A match means only that a source may be
worth reviewing. It does not prove that OpenAI changed the personal export
format, that a synthetic fixture reflects a current export, or that Wenlan
cannot import a ZIP. A real export or representative sample is still required
to confirm compatibility. Coverage is limited to these curated public GitHub
sources; this is not an exhaustive scan of community discussions.

The first scheduled run establishes a checkpoint without sending a historic
notification flood. Later runs keep one open issue titled “Wenlan ChatGPT export
format watch checkpoint”. An HTML comment in that issue stores the scan
watermark and seen event keys. New candidates are comments on that issue; stable
content keys and per-event markers make retries safe, including when a comment
was posted but the checkpoint update failed. The initial baseline scans only
the prior seven days and marks those candidates seen without notifying; a
read-only preview defaults to 14 days. Issue identity uses its title and body,
so a comment-only update does not create a new signal: comments on watched
source issues are not inspected. The source issue title and body are not copied
into reports, and no people are mentioned.

Use **Actions → ChatGPT export format signals → Run workflow** with `publish`
left unchecked for a read-only preview of the last 14 days. Checking `publish`
lets it create or update the shared checkpoint and notify about new candidates.
The weekly schedule already uses that publishing path.

Every monitored repository must finish pagination. HTTP errors, redirects,
oversized responses, unsupported checkpoint data, and pagination beyond the
configured bound fail the run; they do not advance the checkpoint or report
“no change”. The JSON and Markdown artifacts are uploaded even when the scan
step fails after producing them, while the report is also appended to GitHub's
native job summary. GitHub can delay scheduled runs during high load. A clean
scan only means that this bounded source set returned no new matching signal.
Checkpoint and notification writes are preflighted against a conservative
60,000-byte issue-body limit; oversized writes fail before posting.

The scope is intentionally fixed in `scripts/chatgpt-format-watch.py` so the
workflow never executes code from watched repositories. Update the list and
tests when adding sources. Review each candidate manually, then verify against
an actual export before claiming the format changed or adjusting the parser.
