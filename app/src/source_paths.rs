// SPDX-License-Identifier: AGPL-3.0-only
use std::path::Path;
use wenlan_types::sources::Source;

pub fn same_path(left: &Path, right: &Path) -> bool {
    left == right
        || std::fs::canonicalize(left)
            .ok()
            .zip(std::fs::canonicalize(right).ok())
            .is_some_and(|(left, right)| left == right)
}

/// Resolve aliases without discarding the registered spelling (also needed
/// when a disconnected directory no longer exists). Never disambiguate an old
/// shared ID by deleting only one of its registrations.
pub fn source_for_path<'a>(
    sources: &'a [Source],
    path: &Path,
) -> Result<Option<&'a Source>, String> {
    let matches: Vec<_> = sources
        .iter()
        .filter(|source| same_path(&source.path, path))
        .collect();
    if matches.len() > 1 {
        return Err(
            "Multiple sources refer to this folder; repair their registrations first".into(),
        );
    }
    let Some(source) = matches.first().copied() else {
        return Ok(None);
    };
    if sources.iter().filter(|other| other.id == source.id).count() > 1 {
        return Err(format!(
            "Source ID {} belongs to multiple folders; repair their registrations first",
            source.id
        ));
    }
    Ok(Some(source))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn source(path: &Path, id: &str) -> Source {
        serde_json::from_value(serde_json::json!({"id":id,"source_type":"directory","path":path,"status":"Active","last_sync":null,"file_count":0,"memory_count":0,"last_sync_errors":0})).unwrap()
    }
    #[test]
    fn removal_resolves_native_canonical_and_alias_paths_and_keeps_registered_spelling() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("中文 Notes");
        std::fs::create_dir(&path).unwrap();
        let sources = vec![source(&path, "unique")];
        for alias in [
            path.clone(),
            std::fs::canonicalize(&path).unwrap(),
            path.join("."),
        ] {
            assert_eq!(
                source_for_path(&sources, &alias).unwrap().unwrap().path,
                path
            );
        }
        std::fs::remove_dir(&path).unwrap();
        assert!(source_for_path(&sources, &path).unwrap().is_some());
    }
    #[test]
    fn path_removal_cannot_turn_a_legacy_collision_into_an_apparently_unique_source() {
        let root = tempfile::tempdir().unwrap();
        let sources = vec![
            source(&root.path().join("one/Notes"), "directory-notes"),
            source(&root.path().join("two/Notes"), "directory-notes"),
        ];
        assert!(source_for_path(&sources, &sources[0].path).is_err());
        assert_eq!(sources.len(), 2);
    }
}
