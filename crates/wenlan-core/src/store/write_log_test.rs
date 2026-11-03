// SPDX-License-Identifier: Apache-2.0
use super::*;

#[test]
fn savepoint_statements_are_read() {
    assert_eq!(
        savepoint_step(b"SAVEPOINT orphan_repair"),
        Some(SavepointStep::Open("orphan_repair".into()))
    );
    assert_eq!(
        savepoint_step(b"ROLLBACK TO orphan_repair"),
        Some(SavepointStep::RollbackTo("orphan_repair".into()))
    );
    assert_eq!(
        savepoint_step(b"rollback transaction to savepoint \"x\""),
        Some(SavepointStep::RollbackTo("x".into()))
    );
    assert_eq!(
        savepoint_step(b"RELEASE SAVEPOINT x;"),
        Some(SavepointStep::Release("x".into()))
    );
    assert_eq!(savepoint_step(b"ROLLBACK"), None);
    assert_eq!(savepoint_step(b"INSERT INTO t VALUES (1)"), None);
}

#[test]
fn a_rollback_to_a_savepoint_takes_back_its_changes() {
    let change = |table: &str| Change {
        table: table.into(),
        op: RowOp::Insert,
        key: Vec::new(),
        row: vec![("id".into(), Value::Integer(1))],
    };
    let mut pending = Pending::default();
    pending.changes.push(change("a"));
    apply_savepoint_step(&mut pending, SavepointStep::Open("s".into()));
    pending.changes.push(change("b"));
    apply_savepoint_step(&mut pending, SavepointStep::RollbackTo("s".into()));
    assert_eq!(pending.changes, vec![change("a")]);
    pending.changes.push(change("c"));
    apply_savepoint_step(&mut pending, SavepointStep::Release("s".into()));
    assert!(pending.savepoints.is_empty());
    assert_eq!(pending.changes, vec![change("a"), change("c")]);
}

#[test]
fn lines_round_trip_and_a_cut_tail_is_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let log = WriteLog::open(dir.path()).unwrap();
    log.arm(HashMap::new(), 7);
    let change = Change {
        table: "memories".into(),
        op: RowOp::Update,
        key: vec![("source_id".into(), Value::Text("m1".into()))],
        row: vec![
            ("source_id".into(), Value::Text("m1".into())),
            ("score".into(), Value::Real(0.5)),
            ("n".into(), Value::Integer(-3)),
            ("raw".into(), Value::Blob(vec![0, 1, 255])),
            ("gone".into(), Value::Null),
        ],
    };
    let first = log.append(std::slice::from_ref(&change)).unwrap();
    let second = log.append(&[change.clone(), change.clone()]).unwrap();
    assert_eq!(log.position().seq, second);
    // A write cut off mid-line: its transaction never committed.
    let segment = segments(dir.path()).unwrap().pop().unwrap().1;
    let mut file = OpenOptions::new().append(true).open(&segment).unwrap();
    file.write_all(b"{\"seq\":99,\"tx\":9,\"op\":\"insert\",\"tab")
        .unwrap();
    drop(file);
    drop(log);

    let (all, problems) = read_after(dir.path(), None).unwrap();
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].changes, vec![change.clone()]);
    assert_eq!(all[0].schema, 7);
    assert_eq!(all[1].changes.len(), 2);

    let reopened = WriteLog::open(dir.path()).unwrap();
    assert_eq!(reopened.position().seq, second);
    let epoch = reopened.epoch().to_string();
    let (after_first, problems) = read_after(
        dir.path(),
        Some(&LogPosition {
            epoch: epoch.clone(),
            seq: first,
        }),
    )
    .unwrap();
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(after_first.len(), 1);
    assert_eq!(after_first[0].seq, second);
    // A position from another log: everything.
    let (other, _) = read_after(
        dir.path(),
        Some(&LogPosition {
            epoch: "other".into(),
            seq: second,
        }),
    )
    .unwrap();
    assert_eq!(other.len(), 2);
    assert_eq!(last_committed(dir.path()).unwrap().unwrap().seq, second);
}
