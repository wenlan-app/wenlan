// SPDX-License-Identifier: Apache-2.0
//! Wire types for a note that relies on another note (its **basis**).
//!
//! One shape, [`BasisRef`], is used everywhere a note's bases are listed
//! (read, list, recall, write responses), so a caller never has to translate
//! between two spellings of the same row.

use serde::{Deserialize, Serialize};

/// Error-code prefix for a page revision card whose accept was refused because
/// a basis it carries can no longer be recorded (the basis note was archived,
/// deleted, its id reused, or it moved to another Space after the card was
/// proposed). Accepting the card would have to land the prose without the
/// basis the agent said it relied on, so nothing is written and the card is
/// discarded; the human's page is unchanged. The error text is
/// `page_revision_basis_refused: <plain-language reason>`.
pub const PAGE_REVISION_BASIS_REFUSED: &str = "page_revision_basis_refused";

/// One basis of a note, in any state (`ok`, `pending`, `archived`, `deleted`,
/// `unavailable`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BasisRef {
    pub page_id: String,
    /// Live title while the basis exists, the last known title after it is
    /// deleted. Empty when the caller may not see the basis note.
    pub title: String,
    pub state: String,
    /// The version of the basis the citing note was last confirmed against.
    pub from_version: i64,
    /// The basis' version now. `None` once the basis is deleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_version: Option<i64>,
    /// Every version the basis gained since `from_version` was saved as a minor edit.
    #[serde(default)]
    pub minor_only: bool,
}

/// A basis an agent relied on, with the version its `read_page` returned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BasisInput {
    pub page_id: String,
    pub version: i64,
}

/// A human save that also confirms one basis, in the save's own transaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfirmBasisInput {
    pub basis_page_id: String,
    pub version: i64,
}

/// What became of a `confirm_basis` that rode along with a save.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BasisConfirmOutcome {
    /// The save landed and the basis was confirmed at the version asked for.
    Confirmed,
    /// The save landed, but the basis had moved on, so it was not confirmed.
    MovedOn,
    /// The text did not change, so there was no save to confirm with.
    NotSaved,
}

/// Body of `PUT /api/pages/{id}/basis/{basis_id}` and
/// `POST /api/pages/{id}/basis/{basis_id}/confirm`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BasisVersionRequest {
    pub version: i64,
}

/// Receipt for `DELETE /api/pages/{id}/basis/{basis_id}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BasisRemoveResponse {
    pub removed: bool,
}

/// A note this note links to that changed after this note was last modified.
/// Agents only; never shown to people.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkedChanged {
    pub page_id: String,
    pub title: String,
    pub version: i64,
}

/// One version written to a basis since the citing note last confirmed it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BasisDiffEdit {
    pub version: i64,
    pub edited_by: String,
    pub at: i64,
    pub minor: bool,
    /// The edit's one-line summary; empty when the edit recorded none.
    #[serde(default)]
    pub delta_summary: String,
    /// The agent behind an `agent_refresh` edit; None for other writers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
}

/// `GET /api/pages/{id}/basis/{basis_id}/diff`: the text the citing note
/// relied on next to the basis' text now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BasisDiffResponse {
    pub from_version: i64,
    pub to_version: i64,
    pub from_content: String,
    pub to_content: String,
    pub edits: Vec<BasisDiffEdit>,
}

/// A note waiting on a changed basis.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BasisPendingCiterRef {
    pub page_id: String,
    pub title: String,
    pub minor_only: bool,
}

/// One basis and every note waiting on it in one state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BasisPendingEntry {
    pub basis_page_id: String,
    pub title: String,
    pub changed_version: Option<i64>,
    pub state: String,
    pub citers: Vec<BasisPendingCiterRef>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::requests::{CreatePageWithBasisRequest, ManualPageEditRequest, RefreshPageRequest};
    use crate::WriteSpaceTarget;
    use serde_json::json;

    #[test]
    fn a_deleted_basis_leaves_out_its_current_version() {
        let deleted = BasisRef {
            page_id: "a".into(),
            title: "Alpha".into(),
            state: "deleted".into(),
            from_version: 2,
            to_version: None,
            minor_only: false,
        };
        let wire = serde_json::to_value(&deleted).unwrap();
        assert!(wire.get("to_version").is_none(), "{wire}");
        assert_eq!(serde_json::from_value::<BasisRef>(wire).unwrap(), deleted);
    }

    #[test]
    fn confirm_outcomes_are_snake_case_words() {
        for (outcome, word) in [
            (BasisConfirmOutcome::Confirmed, "confirmed"),
            (BasisConfirmOutcome::MovedOn, "moved_on"),
            (BasisConfirmOutcome::NotSaved, "not_saved"),
        ] {
            assert_eq!(serde_json::to_value(outcome).unwrap(), json!(word));
        }
    }

    #[test]
    fn a_version_request_takes_a_version_and_nothing_else() {
        assert!(serde_json::from_value::<BasisVersionRequest>(json!({"version": 3})).is_ok());
        assert!(serde_json::from_value::<BasisVersionRequest>(json!({})).is_err());
        assert!(serde_json::from_value::<BasisVersionRequest>(
            json!({"version": 3, "force": true})
        )
        .is_err());
    }

    /// The create body is the page fields with `basis` beside them. Flattening
    /// must keep the three-way meaning of `space` (absent, null, named) and its
    /// `domain` alias.
    #[test]
    fn create_with_basis_keeps_the_space_meaning_through_the_flatten() {
        let parse = |body: serde_json::Value| -> CreatePageWithBasisRequest {
            serde_json::from_value(body).unwrap()
        };
        let base = json!({"title": "T", "content": "C"});
        let with = |key: &str, value: serde_json::Value| {
            let mut body = base.clone();
            body[key] = value;
            body
        };

        let plain = parse(base.clone());
        assert_eq!(plain.create.space, WriteSpaceTarget::Inherit);
        assert!(plain.basis.is_empty());

        assert_eq!(
            parse(with("space", json!(null))).create.space,
            WriteSpaceTarget::Uncategorized
        );
        assert_eq!(
            parse(with("space", json!("work"))).create.space,
            WriteSpaceTarget::Named("work".into())
        );
        assert_eq!(
            parse(with("domain", json!("work"))).create.space,
            WriteSpaceTarget::Named("work".into())
        );

        let mut body = base.clone();
        body["basis"] = json!([{"page_id": "a", "version": 4}]);
        let based = parse(body);
        assert_eq!(
            based.basis,
            vec![BasisInput {
                page_id: "a".into(),
                version: 4
            }]
        );

        // No basis, no `basis` key: an unmodified client sees the old body.
        let wire = serde_json::to_value(&plain).unwrap();
        assert!(wire.get("basis").is_none(), "{wire}");
        assert!(wire.get("space").is_none(), "{wire}");
    }

    #[test]
    fn the_human_save_body_stays_flat_and_old_bodies_still_parse() {
        let old: ManualPageEditRequest =
            serde_json::from_value(json!({"content": "x", "expected_version": 2})).unwrap();
        assert_eq!(old.edit.expected_version, Some(2));
        assert_eq!(old.minor, None);
        assert!(old.confirm_basis.is_none());

        let new: ManualPageEditRequest = serde_json::from_value(json!({
            "content": "x",
            "minor": true,
            "confirm_basis": {"basis_page_id": "a", "version": 5}
        }))
        .unwrap();
        assert_eq!(new.minor, Some(true));
        assert_eq!(
            new.confirm_basis,
            Some(ConfirmBasisInput {
                basis_page_id: "a".into(),
                version: 5
            })
        );
        let wire = serde_json::to_value(&new).unwrap();
        assert_eq!(wire["content"], "x");
        assert_eq!(wire["confirm_basis"]["version"], 5);
    }

    #[test]
    fn the_agent_write_body_takes_basis_and_minor_and_old_bodies_still_parse() {
        let old: RefreshPageRequest = serde_json::from_value(json!({"content": "x"})).unwrap();
        assert!(old.basis.is_empty());
        assert_eq!(old.minor, None);

        let new: RefreshPageRequest = serde_json::from_value(json!({
            "content": "x",
            "basis": [{"page_id": "a", "version": 1}],
            "minor": true
        }))
        .unwrap();
        assert_eq!(new.basis.len(), 1);
        assert_eq!(new.minor, Some(true));
    }
}
