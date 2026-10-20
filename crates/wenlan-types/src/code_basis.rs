// SPDX-License-Identifier: Apache-2.0
//! Wire types for a note that relies on lines of code (its **code basis**).
//!
//! Mirrors [`crate::page_basis`]: one read shape, [`CodeBasisRef`], is used
//! everywhere a note's code bases are listed (read, list, recall, write
//! responses), and one write shape, [`CodeBasisInput`], rides the same request
//! that writes the note. A code basis sits beside `basis`, never inside it.

use serde::{Deserialize, Serialize};

/// Lines of a file a note relies on, as an agent states them when it writes
/// the note. `repo_root` is absolute; `path` is relative to it and `/`
/// separated; lines are 1-based and inclusive.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CodeBasisInput {
    pub repo_root: String,
    pub path: String,
    pub line_start: u32,
    pub line_end: u32,
}

/// One code basis of a note, in any state (`ok`, `moved`, `pending`,
/// `unavailable`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeBasisRef {
    pub repo_root: String,
    pub path: String,
    pub line_start: u32,
    pub line_end: u32,
    /// `ok`: the lines still hash the same. `moved`: same content at other
    /// line numbers (the span here is already the new one). `pending`: the
    /// content changed or can no longer be found. `unavailable`: the file or
    /// repository is gone.
    pub state: String,
    /// When the content was first seen changed. Only while `pending`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changed_at: Option<i64>,
    /// Why a code basis is `unavailable`: `file_missing`, `repo_missing`,
    /// `outside_repo`, `unreadable` or `too_large`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Body of `POST /api/pages/{id}/code-basis/confirm` and
/// `DELETE /api/pages/{id}/code-basis`: which row of the note is meant. The
/// span is the one the row has now, as a read returned it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodeBasisLocator {
    pub repo_root: String,
    pub path: String,
    pub line_start: u32,
    pub line_end: u32,
}

impl From<&CodeBasisRef> for CodeBasisLocator {
    fn from(row: &CodeBasisRef) -> Self {
        Self {
            repo_root: row.repo_root.clone(),
            path: row.path.clone(),
            line_start: row.line_start,
            line_end: row.line_end,
        }
    }
}

/// One note with code bases that need a look, from
/// `POST /api/pages/code-basis/recheck`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeBasisRecheckEntry {
    pub page_id: String,
    pub title: String,
    /// The note's `pending` and `unavailable` code bases.
    pub pending: Vec<CodeBasisRef>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::requests::{CreatePageWithBasisRequest, RefreshPageRequest};
    use serde_json::json;

    #[test]
    fn a_quiet_ref_leaves_out_changed_at_and_reason() {
        let row = CodeBasisRef {
            repo_root: "/repo".into(),
            path: "src/lib.rs".into(),
            line_start: 10,
            line_end: 20,
            state: "ok".into(),
            changed_at: None,
            reason: None,
        };
        let wire = serde_json::to_value(&row).unwrap();
        assert_eq!(
            wire,
            json!({
                "repo_root": "/repo",
                "path": "src/lib.rs",
                "line_start": 10,
                "line_end": 20,
                "state": "ok"
            })
        );
        assert_eq!(serde_json::from_value::<CodeBasisRef>(wire).unwrap(), row);
    }

    #[test]
    fn a_ref_keeps_changed_at_and_reason_when_present() {
        let wire = json!({
            "repo_root": "/repo",
            "path": "a.rs",
            "line_start": 1,
            "line_end": 2,
            "state": "unavailable",
            "changed_at": 1_700_000_000,
            "reason": "file_missing"
        });
        let row: CodeBasisRef = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(row.changed_at, Some(1_700_000_000));
        assert_eq!(row.reason.as_deref(), Some("file_missing"));
        assert_eq!(serde_json::to_value(&row).unwrap(), wire);
    }

    #[test]
    fn a_locator_takes_the_four_fields_and_nothing_else() {
        let body = json!({"repo_root": "/r", "path": "a.rs", "line_start": 1, "line_end": 2});
        assert!(serde_json::from_value::<CodeBasisLocator>(body.clone()).is_ok());
        let mut extra = body.clone();
        extra["force"] = json!(true);
        assert!(serde_json::from_value::<CodeBasisLocator>(extra).is_err());
        let mut missing = body;
        missing.as_object_mut().unwrap().remove("line_end");
        assert!(serde_json::from_value::<CodeBasisLocator>(missing).is_err());
    }

    #[test]
    fn a_negative_or_oversized_line_is_refused_at_parse_time() {
        for line in [json!(-1), json!(4_294_967_296_u64), json!("3")] {
            let body =
                json!({"repo_root": "/r", "path": "a.rs", "line_start": line, "line_end": 2});
            assert!(serde_json::from_value::<CodeBasisInput>(body).is_err());
        }
    }

    /// An unmodified client sends no `code_basis`; both write bodies parse and
    /// do not grow the key when they serialize again.
    #[test]
    fn old_write_bodies_parse_and_new_ones_carry_code_basis() {
        let old: RefreshPageRequest = serde_json::from_value(json!({"content": "x"})).unwrap();
        assert!(old.code_basis.is_empty());
        assert!(serde_json::to_value(&old)
            .unwrap()
            .get("code_basis")
            .is_none());

        let span =
            json!({"repo_root": "/r", "path": "src/lib.rs", "line_start": 10, "line_end": 20});
        let new: RefreshPageRequest =
            serde_json::from_value(json!({"content": "x", "code_basis": [span.clone()]})).unwrap();
        assert_eq!(new.code_basis.len(), 1);
        assert_eq!(new.code_basis[0].line_end, 20);

        let create_old: CreatePageWithBasisRequest =
            serde_json::from_value(json!({"title": "T", "content": "C"})).unwrap();
        assert!(create_old.code_basis.is_empty());
        let wire = serde_json::to_value(&create_old).unwrap();
        assert!(wire.get("code_basis").is_none(), "{wire}");

        let create_new: CreatePageWithBasisRequest = serde_json::from_value(
            json!({"title": "T", "content": "C", "basis": [], "code_basis": [span]}),
        )
        .unwrap();
        assert_eq!(create_new.code_basis[0].path, "src/lib.rs");
        assert!(create_new.basis.is_empty());
    }
}
