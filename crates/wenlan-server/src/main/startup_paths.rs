// SPDX-License-Identifier: Apache-2.0
use std::path::Path;

pub fn prepare_default_home(
    repair_pending: bool,
    selected: &Path,
    data_local: Option<&Path>,
) -> std::io::Result<bool> {
    if repair_pending {
        return Ok(false);
    }
    std::fs::create_dir_all(selected)?;
    Ok(may_use_default_home(false, selected, data_local))
}

/// Desktop sidecars explicitly pass the default root too. Decide from the
/// selected filesystem identity, not from the presence of an environment flag.
pub fn may_use_default_home(
    repair_pending: bool,
    selected: &Path,
    data_local: Option<&Path>,
) -> bool {
    if repair_pending {
        return false;
    }
    let Some(base) = data_local else {
        return false;
    };
    let Ok(selected) = std::fs::canonicalize(selected) else {
        return false;
    };
    ["wenlan", "origin"]
        .iter()
        .any(|name| std::fs::canonicalize(base.join(name)).is_ok_and(|default| selected == default))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn first_run_default_initializes_home_but_repair_and_scratch_cannot() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("data");
        let legacy_home = dir.path().join("home/.origin");
        std::fs::create_dir_all(&legacy_home).unwrap();
        let selected = base.join("wenlan");
        assert!(!selected.exists());
        assert!(prepare_default_home(false, &selected, Some(&base)).unwrap());
        assert!(selected.is_dir());
        assert!(legacy_home.is_dir());
        let repair = base.join("origin");
        assert!(!prepare_default_home(true, &repair, Some(&base)).unwrap());
        assert!(!repair.exists());
        assert!(!prepare_default_home(false, &dir.path().join("scratch"), Some(&base)).unwrap());
    }
    #[test]
    fn desktop_default_and_legacy_roots_keep_initialization_but_scratch_cannot_touch_home() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["wenlan", "origin", "scratch"] {
            std::fs::create_dir(dir.path().join(name)).unwrap();
        }
        assert!(may_use_default_home(
            false,
            &dir.path().join("wenlan"),
            Some(dir.path())
        ));
        assert!(may_use_default_home(
            false,
            &dir.path().join("wenlan/."),
            Some(dir.path())
        ));
        assert!(may_use_default_home(
            false,
            &dir.path().join("origin"),
            Some(dir.path())
        ));
        assert!(!may_use_default_home(
            false,
            &dir.path().join("scratch"),
            Some(dir.path())
        ));
        assert!(!may_use_default_home(
            true,
            &dir.path().join("wenlan"),
            Some(dir.path())
        ));
    }
}
