// SPDX-License-Identifier: AGPL-3.0-only
//! App-local config file I/O. Reads the shared Wenlan config path, with legacy
//! Origin path fallback, during app startup so local sensors have bootstrap
//! state before the UI talks to the daemon. Settings writes that affect daemon
//! config should go through the daemon HTTP client, then mirror successful
//! values into app-local runtime state when the process needs them immediately.
//! Remaining local compatibility writes preserve daemon-only JSON fields.
//!
//! Copied from origin-core::config; uses AppError instead of OriginError.
use crate::error::AppError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use wenlan_types::sources::{Source, SourceType, SyncStatus};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    /// Legacy field — kept for backward compat with old config files.
    /// Use `sources` instead. Migrated to Source structs by `migrate()`.
    #[serde(default)]
    pub watch_paths: Vec<PathBuf>,
    #[serde(default)]
    pub sources: Vec<Source>,
    #[serde(default)]
    pub knowledge_path: Option<PathBuf>,
    #[serde(default)]
    pub setup_completed: bool,
    #[serde(default)]
    pub anthropic_api_key: Option<String>,
    #[serde(default)]
    pub routine_model: Option<String>,
    #[serde(default)]
    pub synthesis_model: Option<String>,
    #[serde(default)]
    pub remote_access_enabled: bool,
    #[serde(default)]
    pub on_device_model: Option<String>,
    #[serde(default)]
    pub external_llm_endpoint: Option<String>,
    #[serde(default)]
    pub external_llm_model: Option<String>,
}

/// Generate a source ID slug from a directory path (last component, lowercased, sanitized).
fn slug_from_path(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_else(|| "dir".to_string())
        .replace(|c: char| !c.is_alphanumeric() && c != '-', "-")
}

impl Config {
    /// Migrate legacy `watch_paths` entries into `sources` vec.
    /// Idempotent — only converts paths not already represented in `sources`.
    pub fn migrate(&mut self) {
        if self.watch_paths.is_empty() {
            return;
        }
        let existing_paths: std::collections::HashSet<PathBuf> =
            self.sources.iter().map(|s| s.path.clone()).collect();

        for path in &self.watch_paths {
            if existing_paths.contains(path) {
                continue;
            }
            let slug = slug_from_path(path);
            self.sources.push(Source {
                id: format!("dir-{}", slug),
                source_type: SourceType::Directory,
                path: path.clone(),
                status: SyncStatus::Active,
                last_sync: None,
                file_count: 0,
                memory_count: 0,
                last_sync_errors: 0,
                last_sync_error_detail: None,
                space: None,
                queued_files: 0,
                waiting_files: 0,
            });
        }
        // Clear legacy field so it doesn't re-migrate on next load
        self.watch_paths.clear();
    }

    /// Returns the configured knowledge path, or the product default.
    /// Existing `~/Origin/knowledge` directories remain readable during rename.
    pub fn knowledge_path_or_default(&self) -> PathBuf {
        // Resolved lazily: a configured path answers the question without ever
        // asking the OS where home is, so the only callers that can be stopped
        // by an unmeasurable profile are the ones that actually need it.
        if let Some(path) = self.knowledge_path.clone() {
            return path;
        }
        self.knowledge_path_or_default_for_home(&crate::identity_paths::home_base())
    }

    fn knowledge_path_or_default_for_home(&self, home: &std::path::Path) -> PathBuf {
        if let Some(path) = self.knowledge_path.clone() {
            return path;
        }
        let current = home.join("Wenlan").join("knowledge");
        let legacy = home.join("Origin").join("knowledge");
        if !current.exists() && legacy.exists() {
            return legacy;
        }
        current
    }

    /// Returns paths for all active Directory-type sources (for indexer compat).
    pub fn directory_source_paths(&self) -> Vec<PathBuf> {
        self.sources
            .iter()
            .filter(|s| s.source_type == SourceType::Directory)
            .filter(|s| matches!(s.status, SyncStatus::Active))
            .map(|s| s.path.clone())
            .collect()
    }
}

fn config_path() -> PathBuf {
    crate::identity_paths::app_data_dir().join("config.json")
}

// The home root goes through `identity_paths::home_base()`, never
// `dirs::home_dir()` directly: this root is where the user's *pages* are
// written, and a local `unwrap_or(".")` both creates `./Wenlan/knowledge` in
// whatever directory the app was launched from and bypasses the `cfg(test)`
// guard, so a unit test reaching it writes into the real home directory.

pub fn load_config() -> Config {
    let path = config_path();
    let mut config = match std::fs::read_to_string(&path) {
        Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
        Err(_) => Config::default(),
    };
    config.migrate();
    config
}

pub fn save_config(config: &Config) -> Result<(), AppError> {
    save_config_at(&config_path(), config)
}

fn save_config_at(path: &std::path::Path, config: &Config) -> Result<(), AppError> {
    use std::io::Write;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Share the daemon/CLI lock before reading any daemon-owned JSON fields.
    // Otherwise a successful AI Off could race this compatibility save.
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path.with_extension("lock"))?;
    fs2::FileExt::lock_exclusive(&lock)?;
    let value = merge_with_existing_json(path, serde_json::to_value(config)?)?;
    // NamedTempFile creates a private file; write credentials only there and
    // replace the complete document, never expose partial JSON to workers.
    let mut temporary = tempfile::NamedTempFile::new_in(
        path.parent().unwrap_or_else(|| std::path::Path::new(".")),
    )?;
    temporary.write_all(serde_json::to_string_pretty(&value)?.as_bytes())?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

fn merge_with_existing_json(path: &std::path::Path, next: Value) -> Result<Value, AppError> {
    let Value::Object(next) = next else {
        return Ok(next);
    };

    let mut merged = match std::fs::read_to_string(path) {
        Ok(contents) => serde_json::from_str::<serde_json::Map<String, Value>>(&contents)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // Creating config before the daemon starts is still a new install:
            // automatic steeping starts off, "Lock pages I've written" on.
            serde_json::Map::from_iter([
                ("background_ai_enabled".into(), Value::Bool(false)),
                ("lock_my_pages".into(), Value::Bool(true)),
            ])
        }
        Err(error) => return Err(error.into()),
    };

    for (key, value) in next {
        merged.insert(key, value);
    }

    Ok(Value::Object(merged))
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::test_env::EnvGuard;

    const CONFIG_ENV_KEYS: &[&str] = &["HOME", "WENLAN_DATA_DIR", "ORIGIN_DATA_DIR"];

    #[test]
    fn compatibility_save_waits_for_daemon_consent_write() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.json");
        std::fs::write(&path, r#"{"background_ai_enabled":true}"#).unwrap();
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path.with_extension("lock"))
            .unwrap();
        fs2::FileExt::lock_exclusive(&lock).unwrap();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let app_path = path.clone();
        let writer = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            let result = save_config_at(
                &app_path,
                &Config {
                    setup_completed: true,
                    ..Config::default()
                },
            );
            done_tx.send(result).unwrap();
        });
        started_rx.recv().unwrap();
        assert!(matches!(
            done_rx.recv_timeout(std::time::Duration::from_millis(100)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ));
        // Simulate the daemon's completed Off write under the shared lock.
        std::fs::write(&path, r#"{"background_ai_enabled":false,"future_flag":42}"#).unwrap();
        drop(lock);
        done_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()
            .unwrap();
        writer.join().unwrap();
        let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["background_ai_enabled"], false);
        assert_eq!(saved["future_flag"], 42);
        assert_eq!(saved["setup_completed"], true);
    }

    #[test]
    fn compatibility_save_creates_off_and_preserves_malformed_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.json");
        save_config_at(&path, &Config::default()).unwrap();
        let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["background_ai_enabled"], false);
        assert_eq!(
            saved["lock_my_pages"], true,
            "a new install locks pages the user wrote"
        );
        std::fs::write(&path, "{broken").unwrap();
        assert!(save_config_at(&path, &Config::default()).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{broken");
    }

    #[test]
    #[serial_test::serial]
    fn config_path_prefers_wenlan_data_dir() {
        let _env = EnvGuard::capture(CONFIG_ENV_KEYS);
        std::env::set_var("WENLAN_DATA_DIR", "/tmp/wenlan-config-test");
        std::env::set_var("ORIGIN_DATA_DIR", "/tmp/origin-config-test");

        assert_eq!(
            config_path(),
            PathBuf::from("/tmp/wenlan-config-test/config.json")
        );
    }

    #[test]
    #[serial_test::serial]
    fn config_path_falls_back_to_origin_data_dir() {
        let _env = EnvGuard::capture(CONFIG_ENV_KEYS);
        std::env::remove_var("WENLAN_DATA_DIR");
        std::env::set_var("ORIGIN_DATA_DIR", "/tmp/origin-config-test");

        assert_eq!(
            config_path(),
            PathBuf::from("/tmp/origin-config-test/config.json")
        );
    }

    #[test]
    fn test_config_default_values() {
        let config = Config::default();
        assert!(config.watch_paths.is_empty());
    }

    #[test]
    fn test_config_roundtrip_serde() {
        let mut config = Config {
            setup_completed: false,
            anthropic_api_key: None,
            remote_access_enabled: false,
            ..Config::default()
        };
        config.watch_paths = vec![PathBuf::from("/tmp/test")];
        let json = serde_json::to_string(&config).unwrap();
        let restored: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.watch_paths, config.watch_paths);
    }

    #[test]
    fn test_config_deserialize_missing_fields_uses_defaults() {
        let json = r#"{"watch_paths": ["/tmp/a"]}"#;
        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.watch_paths, vec![PathBuf::from("/tmp/a")]);
    }

    #[test]
    fn test_config_deserialize_empty_json() {
        let config: Config = serde_json::from_str("{}").unwrap();
        assert!(config.watch_paths.is_empty());
    }

    // --- save_config / load_config I/O roundtrip ---

    #[test]
    #[serial_test::serial]
    fn save_load_config_roundtrip() {
        let _env = EnvGuard::capture(CONFIG_ENV_KEYS);
        let tmp = tempfile::tempdir().unwrap();
        // Point config_path() at our temp dir via the env override.
        // Env mutation is process-wide, so this must not race with other tests
        // that read or write ORIGIN_DATA_DIR.
        std::env::remove_var("WENLAN_DATA_DIR");
        std::env::set_var("ORIGIN_DATA_DIR", tmp.path());
        let config = Config {
            watch_paths: vec![PathBuf::from("/test/path")],
            ..Config::default()
        };
        save_config(&config).unwrap();
        let loaded = load_config();
        // After load_config, migrate() runs: watch_paths -> sources, watch_paths cleared.
        assert_eq!(loaded.sources.len(), 1);
        assert_eq!(loaded.sources[0].path, PathBuf::from("/test/path"));
        assert!(loaded.watch_paths.is_empty());
    }

    #[test]
    #[serial_test::serial]
    fn save_config_preserves_daemon_only_fields() {
        let _env = EnvGuard::capture(CONFIG_ENV_KEYS);
        let tmp = tempfile::tempdir().unwrap();
        std::env::remove_var("WENLAN_DATA_DIR");
        std::env::set_var("ORIGIN_DATA_DIR", tmp.path());
        let path = config_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"{"private_browsing_detection":true,"reranker_mode":"hybrid","future_flag":{"enabled":true}}"#,
        )
        .unwrap();

        let mut config = load_config();
        config.setup_completed = true;
        save_config(&config).unwrap();

        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["private_browsing_detection"], true);
        assert_eq!(saved["reranker_mode"], "hybrid");
        assert_eq!(saved["future_flag"], serde_json::json!({ "enabled": true }));
        assert_eq!(saved["setup_completed"], true);
    }

    // --- setup_completed ---

    #[test]
    fn test_setup_completed_defaults_to_false() {
        let config = Config::default();
        assert!(!config.setup_completed);
    }

    #[test]
    fn test_setup_completed_roundtrip() {
        let config = Config {
            setup_completed: true,
            ..Config::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        let restored: Config = serde_json::from_str(&json).unwrap();
        assert!(restored.setup_completed);
    }

    #[test]
    fn test_setup_completed_missing_in_json_defaults_false() {
        let json = r#"{"clipboard_enabled": true}"#;
        let config: Config = serde_json::from_str(json).unwrap();
        assert!(!config.setup_completed);
    }

    // --- remote_access_enabled ---

    #[test]
    fn test_remote_access_enabled_defaults_to_false() {
        let config = Config::default();
        assert!(!config.remote_access_enabled);
    }

    #[test]
    fn test_remote_access_enabled_roundtrip() {
        let config = Config {
            remote_access_enabled: true,
            ..Config::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        let restored: Config = serde_json::from_str(&json).unwrap();
        assert!(restored.remote_access_enabled);
    }

    #[test]
    fn test_remote_access_enabled_missing_in_json_defaults_false() {
        let json = r#"{"clipboard_enabled": true}"#;
        let config: Config = serde_json::from_str(json).unwrap();
        assert!(!config.remote_access_enabled);
    }

    // --- migrate() / watch_paths / sources / knowledge_path ---

    #[test]
    fn test_config_defaults_empty_sources() {
        let config: Config = serde_json::from_str("{}").unwrap();
        let mut config = config;
        config.migrate();
        assert!(config.sources.is_empty());
        assert!(config.knowledge_path.is_none());
    }

    #[test]
    fn config_watch_paths_migration() {
        let old_json = r#"{
            "watch_paths": ["/Users/x/docs", "/Users/x/notes"],
            "clipboard_enabled": false
        }"#;
        let mut config: Config = serde_json::from_str(old_json).unwrap();
        config.migrate();
        assert_eq!(config.sources.len(), 2);
        assert_eq!(config.sources[0].source_type, SourceType::Directory);
        assert_eq!(config.sources[0].path, PathBuf::from("/Users/x/docs"));
        assert_eq!(config.sources[1].path, PathBuf::from("/Users/x/notes"));
        // Legacy field cleared after migration.
        assert!(config.watch_paths.is_empty());
    }

    #[test]
    fn config_knowledge_path_default_uses_wenlan_when_no_legacy_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let config: Config = serde_json::from_str("{}").unwrap();
        let default_path = tmp.path().join("Wenlan").join("knowledge");
        assert_eq!(
            config.knowledge_path_or_default_for_home(tmp.path()),
            default_path
        );
    }

    #[test]
    fn config_knowledge_path_default_uses_legacy_when_only_legacy_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let legacy = tmp.path().join("Origin").join("knowledge");
        std::fs::create_dir_all(&legacy).unwrap();
        let config: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(
            config.knowledge_path_or_default_for_home(tmp.path()),
            legacy
        );
    }

    // No `isolate_app_roots` here, deliberately: under `cfg(test)`
    // `identity_paths::home_base()` panics the moment it is reached without
    // one, so this test still passing is itself the assertion that a
    // configured path short-circuits before anything asks the OS where home
    // is. Make the resolution eager again and this test panics.
    #[test]
    fn config_knowledge_path_custom() {
        let json = r#"{"knowledge_path": "/my/custom/path"}"#;
        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(
            config.knowledge_path_or_default(),
            PathBuf::from("/my/custom/path")
        );
    }

    /// The default pages root must come from the isolated home, not from
    /// `dirs::home_dir()` and not from a `"."` fallback. Before this went
    /// through `identity_paths`, a unit test reaching it wrote into the
    /// developer's real home directory and a `dirs` that could not answer
    /// silently relocated the user's pages to the process's cwd.
    #[test]
    #[serial_test::serial]
    fn knowledge_path_default_resolves_under_the_isolated_home() {
        let tmp = tempfile::tempdir().unwrap();
        let _roots = crate::test_env::isolate_app_roots(tmp.path());
        let config: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(
            config.knowledge_path_or_default(),
            tmp.path().join("Wenlan").join("knowledge")
        );
    }

    #[test]
    fn directory_source_paths() {
        let json = r#"{"sources": [
            {"id": "d1", "source_type": "directory", "path": "/a", "status": "Active", "last_sync": null, "file_count": 0, "memory_count": 0},
            {"id": "o1", "source_type": "obsidian", "path": "/b", "status": "Active", "last_sync": null, "file_count": 0, "memory_count": 0}
        ]}"#;
        let config: Config = serde_json::from_str(json).unwrap();
        let paths = config.directory_source_paths();
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], PathBuf::from("/a"));
    }

    // --- unknown-field tolerance ---

    #[test]
    fn dwell_enabled_alias() {
        // dwell_enabled was removed with ambient capture; verify unknown fields are ignored.
        let json = r#"{"dwell_enabled": true}"#;
        let _config: Config = serde_json::from_str(json).unwrap();
    }
}
