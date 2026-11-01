// SPDX-License-Identifier: Apache-2.0
use super::*;

// 2026-05-14 and 2026-11-01, unix seconds.
const MAY: i64 = 1_778_716_800;
const NOV: i64 = 1_793_491_200;

fn person() -> Actor {
    Actor::new(ActorKind::Person)
}

fn page_write(id: &str, version: i64, text: &str, ts: i64) -> NewEntry {
    let mut entry = NewEntry::new(Actor::new(ActorKind::Ai), "page.write");
    entry.ts = Some(ts);
    entry.target = Some(Target::new("page", id));
    entry.version = Some(version);
    entry.text = Some(text.to_string());
    entry
}

fn small() -> HistoryConfig {
    HistoryConfig {
        rotate_bytes: 600,
        blob_threshold: 64,
    }
}

fn log_files(root: &Path) -> Vec<PathBuf> {
    list_log_files(root)
        .unwrap()
        .into_iter()
        .map(|file| file.path)
        .collect()
}

#[test]
fn a_committed_group_replays_and_an_open_one_does_not() {
    let dir = tempfile::tempdir().unwrap();
    let (mut log, report) = HistoryLog::open(dir.path()).unwrap();
    assert_eq!(report.next_seq, 1);

    let mut tx = log.begin(person());
    let first = tx.append(page_write("page_a", 1, "One.", NOV)).unwrap();
    tx.append(page_write("page_b", 1, "Two.", NOV)).unwrap();
    assert_eq!(tx.commit().unwrap(), Some(first));

    // A crash before the commit: the entry is on disk but never counts.
    let mut open = log.begin(person());
    open.append(page_write("page_c", 1, "Three.", NOV)).unwrap();
    drop(open);

    let replay = log.replay().unwrap();
    let ids: Vec<_> = replay
        .entries
        .iter()
        .map(|entry| entry.target.as_ref().unwrap().id.as_str())
        .collect();
    assert_eq!(ids, ["page_a", "page_b"]);
    assert!(replay.entries.iter().all(|entry| entry.tx == first));
    assert_eq!(replay.report.uncommitted, [4]);
    assert!(replay.report.damaged.is_empty());
    assert!(replay.report.bad_lines.is_empty());

    // An empty group writes nothing.
    assert_eq!(log.begin(person()).commit().unwrap(), None);
    assert_eq!(log.next_seq(), 5);
}

#[test]
fn a_second_writer_is_refused_and_reopening_continues_the_seq() {
    let dir = tempfile::tempdir().unwrap();
    let (mut log, _) = HistoryLog::open(dir.path()).unwrap();
    log.write(page_write("page_a", 1, "One.", NOV)).unwrap();
    assert!(matches!(
        HistoryLog::open(dir.path()),
        Err(WenlanError::Conflict(_))
    ));
    drop(log);

    let (mut log, report) = HistoryLog::open(dir.path()).unwrap();
    assert_eq!(report.next_seq, 3);
    assert_eq!(report.repaired_tail_bytes, 0);
    assert_eq!(log.write(page_write("page_a", 2, "Two.", NOV)).unwrap(), 3);
    assert_eq!(log_files(dir.path()).len(), 1, "appends to the same file");
    let replay = log.replay().unwrap();
    assert_eq!(replay.entries.len(), 2);
    assert!(replay.report.is_clean());
}

#[test]
fn long_text_goes_to_one_blob_and_reads_back() {
    let dir = tempfile::tempdir().unwrap();
    let (mut log, _) = HistoryLog::open_with(dir.path(), small()).unwrap();
    let long = "A sentence that repeats. ".repeat(10);
    log.write(page_write("page_a", 1, &long, NOV)).unwrap();
    log.write(page_write("page_a", 2, &long, NOV)).unwrap();
    log.write(page_write("page_a", 3, "Short.", NOV)).unwrap();

    let replay = log.replay().unwrap();
    let digest = replay.entries[0].blob.clone().expect("stored as a blob");
    assert_eq!(replay.entries[0].text, None);
    assert_eq!(replay.entries[1].blob.as_deref(), Some(digest.as_str()));
    assert_eq!(replay.entries[2].blob, None);
    for entry in &replay.entries[..2] {
        assert_eq!(
            log.read_text(entry).unwrap().as_deref(),
            Some(long.as_str())
        );
    }
    let blobs: Vec<_> = fs::read_dir(dir.path().join(BLOBS_DIR))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(blobs.len(), 1, "the same text is stored once");

    // A blob that no longer matches its name is refused, not returned.
    fs::write(dir.path().join(BLOBS_DIR).join(&digest), "changed").unwrap();
    assert!(read_text(dir.path(), &replay.entries[0]).is_err());
    assert!(read_blob(dir.path(), "../state.json").is_err());
}

#[test]
fn files_rotate_by_size_and_month_and_replay_in_seq_order() {
    let dir = tempfile::tempdir().unwrap();
    let (mut log, _) = HistoryLog::open_with(dir.path(), small()).unwrap();
    // Seeded history keeps its own month, commit included.
    for version in 1..=6 {
        log.write(page_write("page_a", version, "Old text.", MAY))
            .unwrap();
    }
    for version in 7..=12 {
        log.write(page_write("page_a", version, "New text.", NOV))
            .unwrap();
    }
    let files = log_files(dir.path());
    assert!(files.len() >= 4, "rotated by size: {files:?}");
    let months: Vec<_> = files
        .iter()
        .map(|path| path.parent().unwrap().file_name().unwrap().to_owned())
        .collect();
    assert!(months.iter().any(|month| month == "2026-05"));
    assert!(months.iter().any(|month| month == "2026-11"));
    for path in &files {
        let month = path.parent().unwrap().file_name().unwrap();
        for line in fs::read_to_string(path).unwrap().lines() {
            let entry = decode_line(line.as_bytes()).unwrap();
            assert_eq!(month_of(entry.ts), month.to_string_lossy());
        }
    }

    drop(log);
    let (log, report) = HistoryLog::open_with(dir.path(), small()).unwrap();
    assert_eq!(report.next_seq, 25);
    let replay = log.replay().unwrap();
    let versions: Vec<_> = replay.entries.iter().map(|e| e.version.unwrap()).collect();
    assert_eq!(versions, (1..=12).collect::<Vec<_>>());
    assert!(replay.report.is_clean());
}

/// Crash injection: cut the file at every byte of the last group. Whatever
/// survives, opening repairs the tail, the earlier group still replays, the
/// cut group never half-counts, and the log keeps working.
#[test]
fn a_crash_at_any_byte_of_a_write_never_half_counts_it() {
    let dir = tempfile::tempdir().unwrap();
    let (mut log, _) = HistoryLog::open(dir.path()).unwrap();
    log.write(page_write("page_a", 1, "Kept.", NOV)).unwrap();
    let path = log_files(dir.path()).remove(0);
    let before = fs::metadata(&path).unwrap().len() as usize;
    let mut tx = log.begin(person());
    tx.append(page_write("page_a", 2, "Cut one.", NOV)).unwrap();
    tx.append(page_write("page_b", 1, "Cut two.", NOV)).unwrap();
    tx.commit().unwrap();
    drop(log);
    let full = fs::read(&path).unwrap();

    for cut in before..full.len() {
        let scratch = tempfile::tempdir().unwrap();
        let copy = scratch.path().join("2026-11");
        fs::create_dir_all(&copy).unwrap();
        fs::write(copy.join(path.file_name().unwrap()), &full[..cut]).unwrap();

        let (mut log, report) = HistoryLog::open(scratch.path()).unwrap();
        let replay = log.replay().unwrap();
        let versions: Vec<_> = replay
            .entries
            .iter()
            .map(|entry| (entry.target.clone().unwrap().id, entry.version))
            .collect();
        assert_eq!(
            versions,
            [("page_a".to_string(), Some(1))],
            "cut at {cut}: only the first write counts"
        );
        assert!(replay.report.bad_lines.is_empty(), "cut at {cut}");
        assert!(replay.report.damaged.is_empty(), "cut at {cut}");
        let whole_lines = full[..cut].iter().filter(|&&b| b == b'\n').count();
        assert_eq!(
            report.repaired_tail_bytes as usize,
            cut - full[..cut]
                .iter()
                .rposition(|&b| b == b'\n')
                .map_or(0, |p| p + 1),
            "cut at {cut}: only the torn line is removed"
        );

        log.write(page_write("page_c", 1, "After.", NOV)).unwrap();
        let replay = log.replay().unwrap();
        assert_eq!(replay.entries.len(), 2, "cut at {cut}");
        assert_eq!(
            replay.entries[1].seq as usize,
            whole_lines + 1,
            "cut at {cut}: seqs continue after the last whole line"
        );
    }
}

#[test]
fn a_damaged_line_inside_a_committed_group_drops_the_group() {
    let dir = tempfile::tempdir().unwrap();
    let (mut log, _) = HistoryLog::open(dir.path()).unwrap();
    let mut tx = log.begin(person());
    tx.append(page_write("page_a", 1, "First sentence.", NOV))
        .unwrap();
    tx.append(page_write("page_b", 1, "Second sentence.", NOV))
        .unwrap();
    tx.commit().unwrap();
    log.write(page_write("page_c", 1, "Third sentence.", NOV))
        .unwrap();
    drop(log);

    let path = log_files(dir.path()).remove(0);
    let text = fs::read_to_string(&path).unwrap();
    fs::write(
        &path,
        text.replacen("First sentence.", "First sentencE.", 1),
    )
    .unwrap();

    let (log, report) = HistoryLog::open(dir.path()).unwrap();
    assert_eq!(
        report.repaired_tail_bytes, 0,
        "a bad line mid-file is not a torn tail"
    );
    assert_eq!(report.next_seq, 6);
    let replay = log.replay().unwrap();
    let ids: Vec<_> = replay
        .entries
        .iter()
        .map(|entry| entry.target.as_ref().unwrap().id.as_str())
        .collect();
    assert_eq!(ids, ["page_c"]);
    assert_eq!(replay.report.damaged, [1]);
    assert_eq!(replay.report.bad_lines.len(), 1);
    assert_eq!(replay.report.bad_lines[0].line, 1);
    assert_eq!(replay.report.bad_lines[0].reason, "crc mismatch");
}

#[test]
fn every_field_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let (mut log, _) = HistoryLog::open(dir.path()).unwrap();
    let mut entry = NewEntry::new(
        Actor {
            kind: ActorKind::Ai,
            name: Some("Claude Code".to_string()),
            agent: Some("claude-code".to_string()),
            session: Some("sess_1".to_string()),
        },
        "page.write",
    );
    entry.ts = Some(NOV);
    entry.target = Some(Target::new("page", "page_a"));
    entry.version = Some(3);
    entry.why = Some("source changed".to_string());
    entry.text = Some("Line one.\nLine \"two\".\t中文".to_string());
    entry.digest_before = Some("aa".to_string());
    entry.digest_after = Some("bb".to_string());
    entry.receipt = Some(Receipt {
        caller_id: "caller".to_string(),
        operation_id: "op_1".to_string(),
        request_digest: Some("cc".to_string()),
        response_digest: None,
    });
    entry.data = Some(serde_json::json!({"links": [{"text": "Lisbon", "id": "page_b"}]}));
    log.write(entry.clone()).unwrap();

    let line = fs::read_to_string(log_files(dir.path()).remove(0)).unwrap();
    let first = line.lines().next().unwrap();
    assert!(first.starts_with("{\"seq\":1,"));
    assert!(first.contains("\"target\":{\"type\":\"page\",\"id\":\"page_a\"}"));

    let replayed = &log.replay().unwrap().entries[0];
    assert_eq!(replayed.seq, 1);
    assert_eq!(replayed.tx, 1);
    assert_eq!(replayed.ts, NOV);
    assert_eq!(replayed.actor, entry.actor);
    assert_eq!(replayed.target, entry.target);
    assert_eq!(replayed.version, entry.version);
    assert_eq!(replayed.why, entry.why);
    assert_eq!(replayed.text, entry.text);
    assert_eq!(replayed.digest_before, entry.digest_before);
    assert_eq!(replayed.digest_after, entry.digest_after);
    assert_eq!(replayed.receipt, entry.receipt);
    assert_eq!(replayed.data, entry.data);
}

/// A line as a newer Wenlan would write it: it passes its check, but its
/// actor kind is one this build does not know.
fn newer_line(seq: u64) -> Vec<u8> {
    let json = serde_json::to_vec(&serde_json::json!({
        "seq": seq,
        "ts": NOV,
        "tx": seq,
        "actor": {"kind": "robot"},
        "op": "page.write",
    }))
    .unwrap();
    let mut line = json.clone();
    line.pop();
    line.extend_from_slice(CRC_PREFIX);
    line.extend_from_slice(crc_of(&json).as_bytes());
    line.extend_from_slice(b"\"}\n");
    line
}

#[test]
fn a_line_a_newer_build_wrote_is_kept_and_the_log_will_not_append() {
    let dir = tempfile::tempdir().unwrap();
    let (mut log, _) = HistoryLog::open(dir.path()).unwrap();
    log.write(page_write("page_a", 1, "First sentence.", NOV))
        .unwrap();
    drop(log);

    let path = log_files(dir.path()).remove(0);
    let mut bytes = fs::read(&path).unwrap();
    bytes.extend_from_slice(&newer_line(3));
    fs::write(&path, &bytes).unwrap();

    let refused = HistoryLog::open(dir.path()).err().expect("open is refused");
    assert!(
        matches!(refused, WenlanError::Conflict(ref message) if message.contains("newer Wenlan")),
        "{refused:?}"
    );
    assert_eq!(
        fs::read(&path).unwrap(),
        bytes,
        "the newer line is not cut off"
    );

    let replay = replay(dir.path()).unwrap();
    assert_eq!(replay.entries.len(), 1, "the older entry still replays");
    assert_eq!(replay.report.newer_lines.len(), 1);
    assert_eq!(replay.report.newer_lines[0].line, 3);
    assert!(replay.report.bad_lines.is_empty());
    assert!(!replay.report.is_clean());
}
