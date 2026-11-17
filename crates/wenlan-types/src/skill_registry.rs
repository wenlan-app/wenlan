// SPDX-License-Identifier: Apache-2.0
//! Wire types for finding, looking at and holding other people's agent
//! skills (SKILL.md folders). The desktop app, the daemon, the CLI and the
//! MCP server all read these shapes, so they keep the camelCase field names
//! the app's screens already use.

use serde::{Deserialize, Serialize};

// ── Search ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkillHit {
    /// `owner/repo`.
    pub source: String,
    /// The id skills.sh lists; not always the folder name in the repo.
    pub skill_id: String,
    pub name: String,
    pub installs: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SearchVia {
    #[serde(rename = "skills.sh")]
    SkillsSh,
    #[serde(rename = "wenlan.app")]
    WenlanApp,
}

// ── Safety checks ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Pass,
    /// At least one check warned, or no check has run yet.
    Warn,
    Fail,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AuditLine {
    pub provider: String,
    pub status: String,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SafetyReport {
    pub verdict: Verdict,
    pub audits: Vec<AuditLine>,
}

// ── Skills Wenlan holds ─────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Scope {
    /// Every Space.
    All,
    /// Only while working in this Space.
    Space { name: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InstalledSkill {
    /// Folder name, in the Wenlan folder and in each tool's folder.
    pub name: String,
    pub source: String,
    pub skill_id: String,
    pub repo_path: String,
    pub commit: String,
    pub hash: String,
    pub verdict: Verdict,
    pub scope: Scope,
    pub description: Option<String>,
    pub installed_at: i64,
    pub updated_at: i64,
}

/// One skill whose author published a change since it was installed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UpdateOutcome {
    pub name: String,
    /// "updated", "needs_look" (the new version did not pass every check),
    /// or "unchanged".
    pub outcome: String,
}

/// A skill found in an AI tool's own folder that Wenlan does not hold yet.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LocalSkill {
    pub name: String,
    /// The folder it was found in.
    pub path: String,
    pub description: Option<String>,
}

// ── Answers ─────────────────────────────────────────────────────────────

/// One search result with the outcome of its safety checks.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkillSearchRow {
    #[serde(flatten)]
    pub hit: SkillHit,
    pub verdict: Verdict,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkillSearchResponse {
    pub via: SearchVia,
    /// Failing skills are never included.
    pub rows: Vec<SkillSearchRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkillPreviewFile {
    pub path: String,
    /// `None` for a file that is not text.
    pub text: Option<String>,
    /// The file is longer than `text`: not all of it is shown.
    #[serde(default)]
    pub cut: bool,
}

/// What a person looks at before a skill is installed: every file, the
/// safety checks, and the exact commit an install would take.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkillPreview {
    pub source: String,
    pub skill_id: String,
    pub commit: String,
    pub repo_path: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub files: Vec<SkillPreviewFile>,
    /// Files that could run on this computer. Wenlan never runs them.
    pub runnable: Vec<String>,
    pub safety: SafetyReport,
    pub source_url: String,
    pub skills_sh_url: String,
    /// Already installed under this folder name.
    pub installed_as: Option<String>,
    /// Why Wenlan will refuse to install this skill, when it holds text
    /// that cannot be shown to the person in full.
    #[serde(default)]
    pub cannot_install: Option<String>,
}

/// A skill Wenlan holds, with what the person added to it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InstalledSkillRow {
    #[serde(flatten)]
    pub skill: InstalledSkill,
    pub additions: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InstalledSkillsResponse {
    pub skills: Vec<InstalledSkillRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LocalSkillsResponse {
    pub skills: Vec<LocalSkill>,
}

/// Where an installed skill was copied.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkillInstallResult {
    pub skill: InstalledSkill,
    /// The AI tool folders that now hold a copy.
    pub tools: Vec<String>,
    /// Same-named folders that belong to the person, left alone.
    pub skipped: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkillImportResult {
    pub imported: Vec<String>,
    /// Name and reason for each skill that could not be taken in.
    pub failed: Vec<(String, String)>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkillRemoveResponse {
    pub removed: String,
}

// ── Requests ────────────────────────────────────────────────────────────

/// Install exactly what a preview showed: its commit and folder.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillInstallRequest {
    pub source: String,
    pub skill_id: String,
    pub commit: String,
    pub repo_path: String,
    pub scope: Scope,
}

/// Names from the scan of this computer; never a path.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillImportRequest {
    pub names: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillRemoveRequest {
    pub name: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The app's screens read these exact field names.
    #[test]
    fn rows_keep_the_field_names_the_app_reads() {
        let row = InstalledSkillRow {
            skill: InstalledSkill {
                name: "meeting-notes".into(),
                source: "o/r".into(),
                skill_id: "meeting-notes".into(),
                repo_path: "skills/meeting-notes".into(),
                commit: "abc".into(),
                hash: "h".into(),
                verdict: Verdict::Pass,
                scope: Scope::Space {
                    name: "Work".into(),
                },
                description: None,
                installed_at: 1,
                updated_at: 2,
            },
            additions: "mine".into(),
        };
        let json = serde_json::to_value(&row).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "name": "meeting-notes", "source": "o/r", "skillId": "meeting-notes",
                "repoPath": "skills/meeting-notes", "commit": "abc", "hash": "h",
                "verdict": "pass", "scope": {"kind": "space", "name": "Work"},
                "description": null, "installedAt": 1, "updatedAt": 2,
                "additions": "mine",
            })
        );
        assert_eq!(
            serde_json::from_value::<InstalledSkillRow>(json).unwrap(),
            row
        );

        let search = SkillSearchResponse {
            via: SearchVia::WenlanApp,
            rows: vec![SkillSearchRow {
                hit: SkillHit {
                    source: "o/r".into(),
                    skill_id: "n".into(),
                    name: "N".into(),
                    installs: 3,
                },
                verdict: Verdict::Warn,
            }],
        };
        let json = serde_json::to_value(&search).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"via": "wenlan.app", "rows": [
                {"source": "o/r", "skillId": "n", "name": "N", "installs": 3, "verdict": "warn"}
            ]})
        );
        assert_eq!(
            serde_json::from_value::<SkillSearchResponse>(json).unwrap(),
            search
        );
    }

    #[test]
    fn requests_refuse_a_path_or_any_other_extra_field() {
        assert!(
            serde_json::from_str::<SkillImportRequest>(r#"{"names":["a"],"path":"/etc"}"#).is_err()
        );
        assert!(serde_json::from_str::<SkillRemoveRequest>(r#"{"name":"a","dir":"/"}"#).is_err());
        let install: SkillInstallRequest = serde_json::from_str(
            r#"{"source":"o/r","skillId":"n","commit":"c","repoPath":"","scope":{"kind":"all"}}"#,
        )
        .unwrap();
        assert_eq!(install.scope, Scope::All);
    }
}
