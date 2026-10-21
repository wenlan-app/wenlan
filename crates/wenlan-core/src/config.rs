// SPDX-License-Identifier: Apache-2.0
use crate::error::WenlanError;
use crate::sources::{Source, SourceType, SyncStatus};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

fn default_true() -> bool {
    true
}

fn default_skip_apps() -> Vec<String> {
    vec![
        "Window Server".into(),
        "Dock".into(),
        "SystemUIServer".into(),
        "Control Center".into(),
        "Notification Center".into(),
        "loginwindow".into(),
        "Spotlight".into(),
        "Wenlan".into(),
        "1Password".into(),
        "Keychain Access".into(),
        "LastPass".into(),
        "Bitwarden".into(),
        "Dashlane".into(),
        "KeePass".into(),
    ]
}

fn default_skip_title_patterns() -> Vec<String> {
    vec![]
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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
    pub clipboard_enabled: bool,
    #[serde(default = "default_skip_apps")]
    pub skip_apps: Vec<String>,
    #[serde(default = "default_skip_title_patterns")]
    pub skip_title_patterns: Vec<String>,
    #[serde(default = "default_true")]
    pub private_browsing_detection: bool,
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
    pub screen_capture_enabled: bool,
    #[serde(default)]
    pub on_device_model: Option<String>,
    #[serde(default)]
    pub external_llm_endpoint: Option<String>,
    #[serde(default)]
    pub external_llm_model: Option<String>,
    /// Bearer key for the external OpenAI-compatible endpoint. Never returned
    /// by any API response — see the key-lifecycle contract in the design spec.
    #[serde(default)]
    pub external_llm_api_key: Option<String>,
    /// Persistent cross-encoder reranker mode (`off`/`lite`/`full`). Daemon-read
    /// at startup via `reranker_mode_resolved`; the `WENLAN_RERANKER_MODE` env
    /// var overrides it. Set with `wenlan models reranker <mode>`.
    #[serde(default)]
    pub reranker_mode: Option<String>,
    /// Optional per-job routing pin for everyday work (recap, extraction, bulk
    /// enrich): `"anthropic"` | `"external"` | `"on_device"`. Selects the SOURCE
    /// only; the model comes from that source's own knobs. Absent disables
    /// model-backed background work for this job class.
    #[serde(default)]
    pub everyday_source: Option<String>,
    /// Optional per-job routing pin for synthesis (page distillation):
    /// `"anthropic"` | `"external"` | `"on_device"`. Selects the SOURCE only.
    /// Absent disables model-backed background work for this job class.
    #[serde(default)]
    pub synthesis_source: Option<String>,
    /// Explicit background-AI preference. Missing in an existing config keeps
    /// its previous routing behavior; a new config is initialized to Off.
    #[serde(default)]
    pub background_ai_enabled: Option<bool>,
    /// Gates ONLY the proactive Page-Map suggestion phase in the refinery
    /// scheduler (`Phase::PageMaps`). The explicit `POST /api/pages/{id}/map/improve`
    /// route is NEVER gated by this flag.
    ///
    /// DEFAULT FALSE since 2026-07-19: a Canvas that fills itself with a dozen
    /// `suggested` boxes before the user has drawn anything reads as clutter to
    /// triage, not as help — the user asked to start from blank and reach for
    /// suggestions deliberately via the Improve button. Opt back in by setting
    /// `page_map_auto_suggest: true` in config.json (or via `PUT /api/config`).
    #[serde(default)]
    pub page_map_auto_suggest: bool,
}

/// Generate a source ID slug from a directory path (last component, lowercased, sanitized).
fn slug_from_path(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_else(|| "dir".to_string())
        .replace(|c: char| !c.is_alphanumeric() && c != '-', "-")
}

impl Config {
    pub fn background_ai_enabled(&self) -> bool {
        self.background_ai_enabled.unwrap_or(true)
    }

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

    /// Returns the configured pages path, or [`default_pages_root`].
    /// Field name remains `knowledge_path` for back-compat with existing
    /// config files; the default folder is rebranded to `pages/` to match
    /// the user-facing `Page` concept and the consolidated `~/.wenlan/`
    /// data layout.
    pub fn knowledge_path_or_default(&self) -> PathBuf {
        self.knowledge_path
            .clone()
            .unwrap_or_else(default_pages_root)
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

/// The real config root: the platform data dir.
#[cfg(not(any(test, feature = "scratch-config")))]
fn default_config_root() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("wenlan")
}

/// A disposable per-process root, used when the crate is compiled for tests.
///
/// ~25 tests call `save_config`, and what keeps them off the developer's real
/// `config.json` today is a *process-global env var*: seven hand-rolled copies
/// of a `DataDirGuard` set `WENLAN_DATA_DIR` to a tempdir and remove it on drop.
/// That is a race, not a guarantee — the guard's `remove_var` on one thread can
/// land between another thread's `set_var` and its `save_config`, and then the
/// write goes to the real file. `Config::default()` carries no API key and no
/// reranker mode, so losing that race wipes both. One hole is still open:
/// `spaces.rs` sets the same var raw, which `temp_env`'s lock does not see.
///
/// Redirecting the *default* root closes it at the one place that resolves it,
/// so the env guards become a convenience rather than the safety mechanism.
/// `WENLAN_DATA_DIR` still wins when set; the rest share one temp dir per
/// process, which is the same save-then-load semantics they had before.
#[cfg(any(test, feature = "scratch-config"))]
fn default_config_root() -> PathBuf {
    static ROOT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    ROOT.get_or_init(|| {
        // Under `cfg(test)` this is the point. Reached from a *binary*, it means
        // the binary was built with the dev-dependency feature on (`cargo test`
        // leaves such a `target/debug/wenlan-server` behind), so say so loudly:
        // the config it reads is empty, not lost.
        #[cfg(not(test))]
        eprintln!(
            "[wenlan] WARNING: built with wenlan-core/scratch-config — config \
             reads and writes go to a temporary directory, not the real data \
             dir. This is a test-only feature; rebuild without it."
        );
        let root =
            std::env::temp_dir().join(format!("wenlan-scratch-config-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&root);
        root
    })
    .clone()
}

fn config_path() -> PathBuf {
    // Honor the `WENLAN_DATA_DIR` override so a scratch daemon (e.g.
    // `wenlan-server --data-dir /tmp/wenlan-demo`) reads and writes its own
    // config file rather than clobbering the user's real one.
    let root = crate::env_compat::var_compat("WENLAN_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(default_config_root);
    root.join("config.json")
}

/// Resolve the shared Wenlan data root used by the daemon and CLI durable
/// workflow artifacts.
pub fn data_root() -> PathBuf {
    crate::env_compat::var_compat("WENLAN_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::data_local_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("wenlan")
        })
}

/// Where pages go when the config names no folder: `~/.wenlan/pages`, or
/// `<data root>/pages` for an isolated run (see [`isolated_data_root`]).
///
/// An isolated run used to share the home default. A scratch daemon whose own
/// `config.json` had no `knowledge_path` (written after the first start, or
/// under a different `TMPDIR` than the one the daemon was given) then
/// projected into the user's real `~/.wenlan/pages` and rewrote its
/// `index.md` during startup reconcile.
pub fn default_pages_root() -> PathBuf {
    match isolated_data_root() {
        Some(root) => root.join("pages"),
        None => home_pages_root(),
    }
}

fn home_pages_root() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".wenlan/pages")
}

/// The installed app's own data roots, the ones an isolated run is not.
fn production_data_roots() -> Vec<PathBuf> {
    dirs::data_local_dir()
        .map(|base| vec![base.join("wenlan"), base.join("origin")])
        .unwrap_or_default()
}

/// `WENLAN_DATA_DIR` when it names a root other than the installed app's own:
/// a dev, test or demo run that must keep to itself. Being set is not enough,
/// because the desktop app's LaunchAgent sets it to the production root.
pub fn isolated_data_root() -> Option<PathBuf> {
    let root = PathBuf::from(crate::env_compat::var_compat("WENLAN_DATA_DIR")?);
    isolated_root_among(root, &production_data_roots())
}

fn isolated_root_among(root: PathBuf, production: &[PathBuf]) -> Option<PathBuf> {
    let is_production = production
        .iter()
        .any(|candidate| same_path(&root, candidate));
    (!is_production).then_some(root)
}

fn resolved(path: &std::path::Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn same_path(a: &std::path::Path, b: &std::path::Path) -> bool {
    a == b || resolved(a) == resolved(b)
}

fn overlaps(a: &std::path::Path, b: &std::path::Path) -> bool {
    let (ra, rb) = (resolved(a), resolved(b));
    a.starts_with(b) || b.starts_with(a) || ra.starts_with(&rb) || rb.starts_with(&ra)
}

/// Refuse an isolated run whose pages folder overlaps the user's real one.
///
/// The default can no longer land there, but an explicit `knowledge_path` can,
/// for example in a scratch config copied from the real one. Startup reconcile
/// would then repair the real folder against the scratch database. Production
/// runs are not checked.
pub fn ensure_isolated_page_root(page_root: &std::path::Path) -> Result<(), String> {
    let Some(data_root) = isolated_data_root() else {
        return Ok(());
    };
    // Read-only: the real configs are consulted only for a custom pages folder.
    let mut protected = vec![home_pages_root()];
    for root in production_data_roots() {
        let custom = std::fs::read_to_string(root.join("config.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<Config>(&text).ok())
            .and_then(|config| config.knowledge_path);
        protected.extend(custom);
    }
    check_isolated_page_root(&data_root, page_root, &protected)
}

fn check_isolated_page_root(
    data_root: &std::path::Path,
    page_root: &std::path::Path,
    protected: &[PathBuf],
) -> Result<(), String> {
    match protected.iter().find(|real| overlaps(page_root, real)) {
        Some(real) => Err(format!(
            "refusing to start: this daemon uses the isolated data root {} but its pages \
             folder {} overlaps the real pages folder {}. Remove `knowledge_path` from {} \
             (the default is {}) or point it at a scratch folder.",
            data_root.display(),
            page_root.display(),
            real.display(),
            data_root.join("config.json").display(),
            data_root.join("pages").display(),
        )),
        None => Ok(()),
    }
}

/// Directory containing durable CLI writes waiting for daemon replay.
pub fn outbox_dir() -> PathBuf {
    data_root().join("outbox")
}

/// The last reported configuration failure (path and parse error), so a
/// corrupt file is logged once rather than on every request and scheduler
/// tick. It is reported again when the error changes or after a successful
/// parse in between.
static LAST_CONFIG_WARNING: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Record the current parse outcome; true when a failure is new.
fn note_config_parse(failure: Option<&str>) -> bool {
    let mut last = LAST_CONFIG_WARNING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let is_new = failure.is_some() && last.as_deref() != failure;
    *last = failure.map(str::to_owned);
    is_new
}

pub fn load_config() -> Config {
    let path = config_path();
    let mut config = match std::fs::read_to_string(&path) {
        Ok(contents) => match serde_json::from_str(&contents) {
            Ok(config) => {
                note_config_parse(None);
                config
            }
            Err(error) => {
                if note_config_parse(Some(&format!("{}|{error}", path.display()))) {
                    log::warn!(
                        "[config] {} is not valid JSON ({error}); using defaults until it is fixed",
                        path.display()
                    );
                }
                new_install_config()
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => new_install_config(),
        Err(_) => new_install_config(),
    };
    config.migrate();
    config
}

fn new_install_config() -> Config {
    Config {
        background_ai_enabled: Some(false),
        ..Config::default()
    }
}

/// True when the config holds any credential — used to tighten file perms.
#[cfg_attr(not(unix), allow(dead_code))]
fn stores_secret(config: &Config) -> bool {
    config.anthropic_api_key.is_some() || config.external_llm_api_key.is_some()
}

/// Save ordinary settings without changing the latest persisted AI consent.
/// A source/key/model operation may hold a snapshot across an await; it must
/// never undo a later explicit On/Off decision.
pub fn save_config(config: &Config) -> Result<(), WenlanError> {
    save_config_inner(&config_path(), config, None).map(|_| ())
}

/// Persist an explicit user decision together with its settings. Returns the
/// committed snapshot, including the preference used by subsequent workers.
pub fn save_config_with_background_ai(
    config: &Config,
    enabled: bool,
) -> Result<Config, WenlanError> {
    save_config_inner(&config_path(), config, Some(enabled))
}

fn save_config_inner(
    path: &std::path::Path,
    config: &Config,
    consent: Option<bool>,
) -> Result<Config, WenlanError> {
    use std::io::Write;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // A separate, stable inode serializes daemon and CLI writers even though
    // config.json itself is replaced atomically below. Closing releases it.
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path.with_extension("lock"))?;
    fs2::FileExt::lock_exclusive(&lock)?;
    let current = match std::fs::read_to_string(path) {
        Ok(contents) => Some(serde_json::from_str::<Config>(&contents)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let mut saved = config.clone();
    saved.background_ai_enabled = match consent {
        Some(enabled) => Some(enabled),
        None => current
            .map(|cfg| cfg.background_ai_enabled)
            .unwrap_or(config.background_ai_enabled.or(Some(false))),
    };
    let mut temporary = tempfile::NamedTempFile::new_in(
        path.parent().unwrap_or_else(|| std::path::Path::new(".")),
    )?;
    // Set private permissions before any credential bytes reach disk.
    #[cfg(unix)]
    if stores_secret(&saved) {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    temporary.write_all(serde_json::to_string_pretty(&saved)?.as_bytes())?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(saved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_install_is_off_but_old_config_keeps_legacy_behavior() {
        assert_eq!(new_install_config().background_ai_enabled, Some(false));
        assert!(!new_install_config().background_ai_enabled());
        let legacy: Config = serde_json::from_str(r#"{"everyday_source":"anthropic"}"#).unwrap();
        assert_eq!(legacy.background_ai_enabled, None);
        assert!(legacy.background_ai_enabled());
        let opted_out: Config = serde_json::from_str(r#"{"background_ai_enabled":false}"#).unwrap();
        assert!(!opted_out.background_ai_enabled());
    }

    #[test]
    fn stale_settings_save_cannot_undo_explicit_background_ai_decision() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.json");
        for enabled in [true, false] {
            let old = save_config_inner(&path, &Config::default(), Some(!enabled)).unwrap();
            let mut delayed_settings = old.clone();
            delayed_settings.clipboard_enabled = true;
            delayed_settings.anthropic_api_key = Some("test-only-key".into());
            save_config_inner(&path, &old, Some(enabled)).unwrap();
            let committed = save_config_inner(&path, &delayed_settings, None).unwrap();
            let disk: Config = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            assert_eq!(committed.background_ai_enabled, Some(enabled));
            assert_eq!(disk.background_ai_enabled, Some(enabled));
            assert!(disk.clipboard_enabled);
        }
    }

    #[test]
    fn ordinary_save_preserves_legacy_consent_but_new_file_starts_off() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.json");
        assert!(!save_config_inner(&path, &Config::default(), None)
            .unwrap()
            .background_ai_enabled());
        std::fs::write(&path, r#"{"everyday_source":"anthropic"}"#).unwrap();
        let saved = save_config_inner(&path, &Config::default(), None).unwrap();
        assert_eq!(saved.background_ai_enabled, None);
        assert!(saved.background_ai_enabled());
    }

    #[test]
    fn malformed_config_is_not_overwritten_by_settings_or_consent_save() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.json");
        std::fs::write(&path, "{broken").unwrap();
        for consent in [None, Some(true), Some(false)] {
            assert!(save_config_inner(&path, &Config::default(), consent).is_err());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), "{broken");
        }
    }

    #[test]
    fn unreadable_or_malformed_config_keeps_background_ai_off() {
        let temp = tempfile::tempdir().unwrap();
        temp_env::with_var("WENLAN_DATA_DIR", Some(temp.path()), || {
            let path = config_path();
            std::fs::write(&path, "{broken").unwrap();
            assert!(!load_config().background_ai_enabled());
            std::fs::remove_file(&path).unwrap();
            std::fs::create_dir(&path).unwrap();
            assert!(!load_config().background_ai_enabled());
        });
    }

    /// A test binary may never resolve the developer's real `config.json`.
    /// ~25 test call sites reach `save_config`, which writes `Config::default()`
    /// — no API key, no reranker mode — so a test that lost the env-var race
    /// described on `default_config_root` wiped both. Unsets the legacy
    /// `ORIGIN_DATA_DIR` too: `var_compat` falls back to it, so leaving it set
    /// would mask the real default.
    #[test]
    fn config_path_never_resolves_the_real_data_dir_in_tests() {
        temp_env::with_vars(
            [
                ("WENLAN_DATA_DIR", None::<&str>),
                ("ORIGIN_DATA_DIR", None::<&str>),
            ],
            || {
                let real = dirs::data_local_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join("wenlan")
                    .join("config.json");
                assert_ne!(
                    config_path(),
                    real,
                    "test builds must not read or write the real config"
                );
            },
        );
    }

    #[test]
    fn test_config_default_values() {
        let config = Config::default();
        assert!(config.watch_paths.is_empty());
        assert!(!config.clipboard_enabled);
    }

    #[test]
    fn test_config_roundtrip_serde() {
        let mut config = Config {
            clipboard_enabled: true,
            skip_apps: vec!["TestApp".into()],
            skip_title_patterns: vec!["secret*".into()],
            private_browsing_detection: false,
            setup_completed: false,
            anthropic_api_key: None,
            remote_access_enabled: false,
            screen_capture_enabled: false,
            ..Config::default()
        };
        config.watch_paths = vec![PathBuf::from("/tmp/test")];
        let json = serde_json::to_string(&config).unwrap();
        let restored: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.watch_paths, config.watch_paths);
        assert!(restored.clipboard_enabled);
        assert_eq!(restored.skip_apps, vec!["TestApp".to_string()]);
        assert!(!restored.private_browsing_detection);
    }

    #[test]
    fn test_config_deserialize_missing_fields_uses_defaults() {
        let json = r#"{"watch_paths": ["/tmp/a"]}"#;
        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.watch_paths, vec![PathBuf::from("/tmp/a")]);
        assert!(config.private_browsing_detection);
        assert!(!config.skip_apps.is_empty());
    }

    #[test]
    fn test_config_deserialize_empty_json() {
        let config: Config = serde_json::from_str("{}").unwrap();
        assert!(config.watch_paths.is_empty());
    }

    #[test]
    fn test_save_load_config_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let config_file = tmp.path().join("config.json");
        let mut config = Config {
            clipboard_enabled: true,
            ..Config::default()
        };
        config.watch_paths = vec![PathBuf::from("/test/path")];
        let json = serde_json::to_string_pretty(&config).unwrap();
        std::fs::write(&config_file, &json).unwrap();
        let contents = std::fs::read_to_string(&config_file).unwrap();
        let restored: Config = serde_json::from_str(&contents).unwrap();
        assert_eq!(restored.watch_paths, vec![PathBuf::from("/test/path")]);
        assert!(restored.clipboard_enabled);
    }

    #[test]
    fn test_dwell_enabled_alias() {
        // dwell_enabled was removed with ambient capture; verify unknown fields are ignored
        let json = r#"{"dwell_enabled": true}"#;
        let _config: Config = serde_json::from_str(json).unwrap();
    }

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

    #[test]
    fn test_screen_capture_enabled_defaults_to_false() {
        let config = Config::default();
        assert!(!config.screen_capture_enabled);
    }

    #[test]
    fn test_screen_capture_enabled_roundtrip() {
        let config = Config {
            screen_capture_enabled: true,
            ..Config::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        let restored: Config = serde_json::from_str(&json).unwrap();
        assert!(restored.screen_capture_enabled);
    }

    #[test]
    fn test_screen_capture_enabled_missing_in_json_defaults_false() {
        let json = r#"{"clipboard_enabled": true}"#;
        let config: Config = serde_json::from_str(json).unwrap();
        assert!(!config.screen_capture_enabled);
    }

    #[test]
    fn config_model_choice_defaults_none() {
        let config = Config::default();
        assert!(config.routine_model.is_none());
        assert!(config.synthesis_model.is_none());
    }

    #[test]
    fn config_model_choice_roundtrip() {
        let json = r#"{
            "anthropic_api_key": "sk-test",
            "routine_model": "claude-haiku-4-5-20251001",
            "synthesis_model": "claude-opus-4-6"
        }"#;
        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(
            config.routine_model.as_deref(),
            Some("claude-haiku-4-5-20251001")
        );
        assert_eq!(config.synthesis_model.as_deref(), Some("claude-opus-4-6"));
    }

    #[test]
    fn config_backward_compat_without_model_fields() {
        let json = r#"{"anthropic_api_key": "sk-test"}"#;
        let config: Config = serde_json::from_str(json).unwrap();
        assert!(config.routine_model.is_none());
        assert!(config.synthesis_model.is_none());
    }

    #[test]
    fn config_reranker_mode_roundtrip_and_default_none() {
        // Persisted reranker_mode survives a serialize -> deserialize round-trip,
        // and an old config file without the field deserializes to None (serde default).
        let cfg = Config {
            reranker_mode: Some("full".to_string()),
            ..Default::default()
        };
        let back: Config = serde_json::from_str(&serde_json::to_string(&cfg).unwrap()).unwrap();
        assert_eq!(back.reranker_mode.as_deref(), Some("full"));

        let old: Config = serde_json::from_str(r#"{"anthropic_api_key": "sk-test"}"#).unwrap();
        assert_eq!(old.reranker_mode, None);
    }

    #[test]
    fn page_map_auto_suggest_defaults_false_on_missing_field() {
        // A config file without the field deserializes to false — a Canvas the
        // user has not asked to populate stays blank. This is the teeth on the
        // start-from-blank decision; flipping the serde default back to true
        // fails here.
        let config: Config = serde_json::from_str(r#"{"clipboard_enabled": true}"#).unwrap();
        assert!(!config.page_map_auto_suggest);
    }

    #[test]
    fn page_map_auto_suggest_roundtrip_true() {
        // Opting back in survives a serialize/deserialize cycle, so a user who
        // turns proactive suggestions on keeps them.
        let config = Config {
            page_map_auto_suggest: true,
            ..Config::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        let restored: Config = serde_json::from_str(&json).unwrap();
        assert!(restored.page_map_auto_suggest);
    }

    #[test]
    fn page_map_auto_suggest_tolerates_unknown_fields() {
        // Unknown/adjacent fields don't disturb the flag's serde default.
        let json = r#"{"page_map_auto_suggest": false, "some_future_field": 42}"#;
        let config: Config = serde_json::from_str(json).unwrap();
        assert!(!config.page_map_auto_suggest);
    }

    // --- New sources/knowledge_path tests ---

    #[test]
    fn test_config_defaults_empty_sources() {
        let config: Config = serde_json::from_str("{}").unwrap();
        let mut config = config;
        config.migrate();
        assert!(config.sources.is_empty());
        assert!(config.knowledge_path.is_none());
    }

    #[test]
    fn test_config_watch_paths_migration() {
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
    }

    #[test]
    fn test_config_knowledge_path_default() {
        temp_env::with_vars(
            [
                ("WENLAN_DATA_DIR", None::<&str>),
                ("ORIGIN_DATA_DIR", None::<&str>),
            ],
            || {
                let config: Config = serde_json::from_str("{}").unwrap();
                let default_path = dirs::home_dir().unwrap().join(".wenlan/pages");
                assert_eq!(config.knowledge_path_or_default(), default_path);
            },
        );
    }

    /// The four Mac test runs of 2026-10-20: a scratch daemon with no
    /// `knowledge_path` in its own config used the real `~/.wenlan/pages`.
    #[test]
    fn isolated_data_root_keeps_default_pages_inside_it() {
        let scratch = tempfile::tempdir().unwrap();
        temp_env::with_vars(
            [
                ("WENLAN_DATA_DIR", Some(scratch.path())),
                ("ORIGIN_DATA_DIR", None),
            ],
            || {
                let config: Config = serde_json::from_str("{}").unwrap();
                assert_eq!(
                    config.knowledge_path_or_default(),
                    scratch.path().join("pages")
                );
                assert_eq!(isolated_data_root().as_deref(), Some(scratch.path()));
            },
        );
    }

    #[test]
    fn the_production_root_named_explicitly_is_not_isolated() {
        let base = tempfile::tempdir().unwrap();
        let production = vec![base.path().join("wenlan"), base.path().join("origin")];
        for root in &production {
            assert_eq!(isolated_root_among(root.clone(), &production), None);
        }
        let mut with_slash = production[0].clone().into_os_string();
        with_slash.push("/");
        assert_eq!(
            isolated_root_among(PathBuf::from(with_slash), &production),
            None
        );
        let scratch = base.path().join("scratch");
        assert_eq!(
            isolated_root_among(scratch.clone(), &production),
            Some(scratch)
        );
    }

    #[test]
    fn isolated_page_root_may_not_overlap_the_real_pages_folder() {
        let home = tempfile::tempdir().unwrap();
        let real = home.path().join(".wenlan/pages");
        std::fs::create_dir_all(&real).unwrap();
        let scratch = home.path().join("scratch");
        let protected = vec![real.clone()];

        for page_root in [real.clone(), real.join("sub"), home.path().join(".wenlan")] {
            let error = check_isolated_page_root(&scratch, &page_root, &protected).unwrap_err();
            assert!(error.contains(&real.display().to_string()), "{error}");
        }
        assert!(check_isolated_page_root(&scratch, &scratch.join("pages"), &protected).is_ok());
        assert!(check_isolated_page_root(
            &scratch,
            &home.path().join(".wenlan/pages-copy"),
            &protected
        )
        .is_ok());
    }

    #[test]
    fn test_config_knowledge_path_custom() {
        let json = r#"{"knowledge_path": "/my/custom/path"}"#;
        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(
            config.knowledge_path_or_default(),
            PathBuf::from("/my/custom/path")
        );
    }

    #[test]
    fn test_directory_source_paths() {
        let json = r#"{"sources": [
            {"id": "d1", "source_type": "directory", "path": "/a", "status": "Active", "last_sync": null, "file_count": 0, "memory_count": 0},
            {"id": "o1", "source_type": "obsidian", "path": "/b", "status": "Active", "last_sync": null, "file_count": 0, "memory_count": 0}
        ]}"#;
        let config: Config = serde_json::from_str(json).unwrap();
        let paths = config.directory_source_paths();
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], PathBuf::from("/a"));
    }

    #[test]
    fn test_external_llm_api_key_roundtrip_and_default() {
        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert!(cfg.external_llm_api_key.is_none());
        let cfg = Config {
            external_llm_api_key: Some("sk-test".into()),
            ..Config::default()
        };
        let restored: Config = serde_json::from_str(&serde_json::to_string(&cfg).unwrap()).unwrap();
        assert_eq!(restored.external_llm_api_key.as_deref(), Some("sk-test"));
    }

    #[test]
    fn test_stores_secret_covers_both_keys() {
        assert!(!stores_secret(&Config::default()));
        assert!(stores_secret(&Config {
            anthropic_api_key: Some("k".into()),
            ..Config::default()
        }));
        assert!(stores_secret(&Config {
            external_llm_api_key: Some("k".into()),
            ..Config::default()
        }));
    }
}
