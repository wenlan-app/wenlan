// SPDX-License-Identifier: Apache-2.0
//! Pruning of the pre-migration snapshots `backup_before_migration` leaves
//! beside the database as `pre_migration_<n>_backup.db`.
//!
//! Each snapshot is a full copy of the database (about 700 MB on a real
//! library), and nothing removed them, so they piled up release after release.
//! After a migration chain that took a backup finishes, only the newest few
//! are kept. Only files whose whole name is the pattern this code writes are
//! ever touched; every other file in the directory (hand-made backups, the
//! database itself, `.tmp` staging files) is left alone.

use std::path::{Path, PathBuf};

/// How many pre-migration snapshots stay after a migration chain. The newest
/// is the restore point for the upgrade that just ran; two more cover a
/// problem that only shows a release or two later. Each is a full copy of the
/// database, so this keeps the folder at a few database sizes.
pub(crate) const MIGRATION_BACKUPS_KEPT: usize = 3;

/// The migration number of a `pre_migration_<n>_backup.db` file name, or
/// `None` for any other name.
fn backup_migration_number(name: &str) -> Option<i64> {
    let digits = name
        .strip_prefix("pre_migration_")?
        .strip_suffix("_backup.db")?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// The `app_metadata` key holding the `user_version` an upgrade started
/// from. It is written when the upgrade takes its first snapshot and cleared
/// when the upgrade finishes, so an upgrade that failed partway and is
/// retried on the next start still knows which snapshots are its own.
pub(crate) const MIGRATION_CHAIN_FROM_VERSION_KEY: &str = "migration_chain_from_version";

/// Delete old pre-migration snapshots in `dir`. A snapshot numbered above
/// `protect_above` (the version the running upgrade started from) is never
/// removed; of the rest, the newest are kept until `keep_at_least` remain in
/// all. Newest means the highest migration number. A snapshot's `-wal`/`-shm`
/// sidecars, if any, go with it.
///
/// Each removal is logged as it happens. A file that cannot be removed is
/// logged and skipped, so one failure never hides the removals before it or
/// stops the ones after it. Returns the snapshot files removed; an error only
/// when the directory cannot be listed.
pub(crate) fn prune_migration_backups(
    dir: &Path,
    keep_at_least: usize,
    protect_above: Option<i64>,
) -> std::io::Result<Vec<PathBuf>> {
    prune_migration_backups_with(dir, keep_at_least, protect_above, |path| {
        std::fs::remove_file(path)
    })
}

fn prune_migration_backups_with(
    dir: &Path,
    keep_at_least: usize,
    protect_above: Option<i64>,
    mut remove: impl FnMut(&Path) -> std::io::Result<()>,
) -> std::io::Result<Vec<PathBuf>> {
    let mut backups: Vec<(i64, PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let Some(number) = entry.file_name().to_str().and_then(backup_migration_number) else {
            continue;
        };
        backups.push((number, entry.path()));
    }
    backups.sort_by_key(|(number, _)| std::cmp::Reverse(*number));
    let protected = |number: i64| protect_above.is_some_and(|from| number > from);
    let keep = backups
        .iter()
        .filter(|(number, _)| protected(*number))
        .count()
        .max(keep_at_least);
    let mut removed = Vec::new();
    for (number, path) in backups.into_iter().skip(keep) {
        if protected(number) {
            continue;
        }
        if let Err(error) = remove(&path) {
            log::warn!(
                "[migration] could not remove old pre-migration backup {}: {error}",
                path.display()
            );
            continue;
        }
        log::info!(
            "[migration] removed old pre-migration backup {} (keeping the newest {keep})",
            path.display()
        );
        for suffix in ["-wal", "-shm"] {
            let mut sidecar = path.as_os_str().to_os_string();
            sidecar.push(suffix);
            let sidecar = PathBuf::from(sidecar);
            if sidecar.is_file() {
                if let Err(error) = remove(&sidecar) {
                    log::warn!(
                        "[migration] could not remove {}: {error}",
                        sidecar.display()
                    );
                }
            }
        }
        removed.push(path);
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(dir: &Path, name: &str) {
        std::fs::write(dir.join(name), b"x").unwrap();
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn keeps_the_newest_snapshots_by_migration_number() {
        let dir = tempfile::tempdir().unwrap();
        // 99 < 100: numeric order, not name order.
        for n in [99, 100, 152, 153, 157, 158] {
            touch(dir.path(), &format!("pre_migration_{n}_backup.db"));
        }
        touch(dir.path(), "pre_migration_152_backup.db-wal");
        let removed = prune_migration_backups(dir.path(), 3, None).unwrap();
        assert_eq!(removed.len(), 3);
        assert_eq!(
            names(dir.path()),
            vec![
                "pre_migration_153_backup.db",
                "pre_migration_157_backup.db",
                "pre_migration_158_backup.db",
            ]
        );
    }

    #[test]
    fn leaves_every_file_it_did_not_write_alone() {
        let dir = tempfile::tempdir().unwrap();
        for n in [150, 151, 152, 153] {
            touch(dir.path(), &format!("pre_migration_{n}_backup.db"));
        }
        let others = [
            "origin_memory.db",
            "origin_memory.db-wal",
            "origin_memory.db.backup-2026-08-04-edges-repair",
            "pre_migration_149_backup.db.tmp",
            "pre_migration_x_backup.db",
            "pre_migration__backup.db",
            "my_pre_migration_1_backup.db",
            "pre_migration_1_backup.db.old",
        ];
        for name in others {
            touch(dir.path(), name);
        }
        std::fs::create_dir(dir.path().join("pre_migration_1_backup.db")).unwrap();
        let removed = prune_migration_backups(dir.path(), 3, None).unwrap();
        assert_eq!(
            removed,
            vec![dir.path().join("pre_migration_150_backup.db")]
        );
        let left = names(dir.path());
        for name in others {
            assert!(left.contains(&name.to_string()), "{name} was removed");
        }
        assert!(dir.path().join("pre_migration_1_backup.db").is_dir());
    }

    /// Through the real chain: a migration that takes a snapshot prunes the
    /// older ones, keeps the newest three, and leaves other files alone.
    #[tokio::test]
    async fn a_migration_chain_that_took_a_backup_prunes_older_snapshots() {
        let (db, dir) = super::super::tests::test_db_at(157).await;
        for n in [100, 150, 155, 157] {
            touch(dir.path(), &format!("pre_migration_{n}_backup.db"));
        }
        touch(
            dir.path(),
            "origin_memory.db.backup-2026-08-04-edges-repair",
        );
        db.run_migrations(&crate::events::NoopEmitter)
            .await
            .unwrap();
        let backups: Vec<String> = names(dir.path())
            .into_iter()
            .filter(|name| name.starts_with("pre_migration_"))
            .collect();
        assert_eq!(
            backups,
            vec![
                "pre_migration_155_backup.db",
                "pre_migration_157_backup.db",
                "pre_migration_158_backup.db",
            ]
        );
        assert!(dir
            .path()
            .join("origin_memory.db.backup-2026-08-04-edges-repair")
            .exists());
        // A later open with nothing left to migrate takes no backup and
        // prunes nothing.
        touch(dir.path(), "pre_migration_101_backup.db");
        db.run_migrations(&crate::events::NoopEmitter)
            .await
            .unwrap();
        assert!(dir.path().join("pre_migration_101_backup.db").exists());
    }

    /// A chain of more migrations than are normally kept keeps every snapshot
    /// it took, the oldest of which is the state before the whole upgrade.
    #[tokio::test]
    async fn a_long_chain_keeps_every_snapshot_it_took() {
        let (db, dir) = super::super::tests::test_db_at(151).await;
        touch(dir.path(), "pre_migration_140_backup.db");
        db.run_migrations(&crate::events::NoopEmitter)
            .await
            .unwrap();
        let backups: Vec<String> = names(dir.path())
            .into_iter()
            .filter(|name| name.starts_with("pre_migration_"))
            .collect();
        assert!(!backups.contains(&"pre_migration_140_backup.db".to_string()));
        assert!(backups.contains(&"pre_migration_152_backup.db".to_string()));
        assert!(backups.contains(&"pre_migration_158_backup.db".to_string()));
    }

    /// An upgrade from 150 that stopped partway (its snapshots 151..156 on
    /// disk, `user_version` left at 155) and is retried on the next start
    /// keeps 151, the only state from before the whole upgrade, although the
    /// retried chain itself only reaches 156..158.
    #[tokio::test]
    async fn a_retried_upgrade_keeps_the_snapshot_from_before_it() {
        let (db, dir) = super::super::tests::test_db_at(150).await;
        db.run_migrations_up_to(&crate::events::NoopEmitter, 155)
            .await
            .unwrap();
        // The first attempt took 156's snapshot, then failed inside 156.
        touch(dir.path(), "pre_migration_156_backup.db");
        assert_eq!(
            db.get_app_metadata(MIGRATION_CHAIN_FROM_VERSION_KEY)
                .await
                .unwrap()
                .as_deref(),
            Some("150")
        );
        touch(dir.path(), "pre_migration_120_backup.db");

        db.run_migrations(&crate::events::NoopEmitter)
            .await
            .unwrap();
        let backups: Vec<String> = names(dir.path())
            .into_iter()
            .filter(|name| name.starts_with("pre_migration_"))
            .collect();
        let expected: Vec<String> = (151..=158)
            .map(|n| format!("pre_migration_{n}_backup.db"))
            .collect();
        assert_eq!(backups, expected, "every snapshot of the upgrade stays");
        assert_eq!(
            db.get_app_metadata(MIGRATION_CHAIN_FROM_VERSION_KEY)
                .await
                .unwrap(),
            None,
            "a finished upgrade forgets where it started"
        );
    }

    #[test]
    fn a_file_that_cannot_be_removed_does_not_hide_the_other_removals() {
        let dir = tempfile::tempdir().unwrap();
        for n in [150, 151, 152, 153, 154] {
            touch(dir.path(), &format!("pre_migration_{n}_backup.db"));
        }
        let logs = crate::test_log::LogCapture::start();
        let stuck = dir.path().join("pre_migration_151_backup.db");
        let removed = prune_migration_backups_with(dir.path(), 3, None, |path| {
            if path == stuck {
                Err(std::io::Error::other("busy"))
            } else {
                std::fs::remove_file(path)
            }
        })
        .unwrap();
        assert_eq!(
            removed,
            vec![dir.path().join("pre_migration_150_backup.db")]
        );
        assert!(stuck.exists());
        assert_eq!(
            logs.matching(
                "INFO",
                &["removed old pre-migration backup", "pre_migration_150"]
            )
            .len(),
            1,
            "{:?}",
            logs.lines()
        );
        assert_eq!(
            logs.matching("WARN", &["could not remove", "pre_migration_151"])
                .len(),
            1,
            "{:?}",
            logs.lines()
        );
    }

    #[test]
    fn fewer_snapshots_than_kept_removes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), "pre_migration_158_backup.db");
        assert!(prune_migration_backups(dir.path(), 3, None)
            .unwrap()
            .is_empty());
        assert_eq!(names(dir.path()), vec!["pre_migration_158_backup.db"]);
    }
}
