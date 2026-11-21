// SPDX-License-Identifier: Apache-2.0
//! Explicit installation of Wenlan's Codex SessionStart and Stop hooks.

use anyhow::{bail, Context};
use serde_json::{json, Map, Value};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

const BRIEF_CONTEXT: &str = include_str!("../../../../plugin-codex/hooks/brief-context.sh");
const HANDOFF_REMINDER: &str = include_str!("../../../../plugin-codex/hooks/handoff-reminder.sh");
const RESOLVE_SPACE: &str = include_str!("../../../../plugin-codex/bin/resolve-space.sh");
const INSTALL_DIR: &str = "wenlan-hooks";

pub(super) struct HookPlan {
    pub(super) codex_home: PathBuf,
    pub(super) merged_json: String,
    pub(super) needs_install: bool,
    original_exists: bool,
    original_json: Vec<u8>,
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\"'\"'"))
}

fn command(path: &Path) -> String {
    format!("WENLAN_HOOK_HOST=codex bash {}", shell_quote(path))
}

fn hook_group(command: &str) -> Value {
    json!({
        "hooks": [{
            "type": "command",
            "command": command,
            "timeout": 5
        }]
    })
}

fn event_has_command(event: &[Value], wanted: &str) -> bool {
    event.iter().any(|group| {
        group
            .get("hooks")
            .and_then(Value::as_array)
            .is_some_and(|hooks| {
                hooks
                    .iter()
                    .any(|hook| hook.get("command").and_then(Value::as_str) == Some(wanted))
            })
    })
}

fn add_hook(root: &mut Value, event_name: &str, command: &str) -> anyhow::Result<bool> {
    let root = root
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("Codex hooks.json must contain a JSON object"))?;
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("Codex hooks.json `hooks` must be a JSON object"))?;
    let event = hooks
        .entry(event_name)
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or_else(|| {
            anyhow::anyhow!("Codex hooks.json `hooks.{event_name}` must be a JSON array")
        })?;
    if event_has_command(event, command) {
        return Ok(false);
    }
    event.push(hook_group(command));
    Ok(true)
}

fn file_differs(path: &Path, expected: &str) -> bool {
    fs::read_to_string(path).map_or(true, |current| current != expected)
}

pub(super) fn plan(codex_home: &Path) -> anyhow::Result<HookPlan> {
    let hooks_path = codex_home.join("hooks.json");
    let (original_exists, original_json) = match fs::read(&hooks_path) {
        Ok(raw) => (true, raw),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (false, Vec::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("read {}", hooks_path.display()));
        }
    };
    let original = String::from_utf8(original_json.clone())
        .with_context(|| format!("parse {} as UTF-8", hooks_path.display()))?;
    let mut root = if original.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str::<Value>(&original)
            .with_context(|| format!("parse {}", hooks_path.display()))?
    };
    if !root.is_object() {
        bail!("Codex hooks.json must contain a JSON object");
    }

    let install_dir = codex_home.join(INSTALL_DIR);
    let brief_command = command(&install_dir.join("brief-context.sh"));
    let reminder_command = command(&install_dir.join("handoff-reminder.sh"));
    let changed = add_hook(&mut root, "SessionStart", &brief_command)?
        | add_hook(&mut root, "Stop", &reminder_command)?;
    let merged_json = if changed || original.is_empty() {
        format!("{}\n", serde_json::to_string_pretty(&root)?)
    } else {
        original
    };
    let scripts_differ = file_differs(&install_dir.join("brief-context.sh"), BRIEF_CONTEXT)
        || file_differs(&install_dir.join("handoff-reminder.sh"), HANDOFF_REMINDER)
        || file_differs(&install_dir.join("resolve-space.sh"), RESOLVE_SPACE);

    Ok(HookPlan {
        codex_home: codex_home.to_path_buf(),
        needs_install: changed || scripts_differ,
        merged_json,
        original_exists,
        original_json,
    })
}

fn write_atomic(path: &Path, contents: &str) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("{} has no parent directory", path.display()))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("create temporary file beside {}", path.display()))?;
    temporary
        .write_all(contents.as_bytes())
        .with_context(|| format!("write temporary file beside {}", path.display()))?;
    temporary
        .as_file_mut()
        .sync_all()
        .with_context(|| format!("sync temporary file beside {}", path.display()))?;
    temporary
        .persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("replace {}", path.display()))?;
    Ok(())
}

fn write_if_changed(path: &Path, contents: &str) -> anyhow::Result<()> {
    if !file_differs(path, contents) {
        return Ok(());
    }
    write_atomic(path, contents)
}

pub(super) fn apply(plan: &HookPlan) -> anyhow::Result<()> {
    let hooks_path = plan.codex_home.join("hooks.json");
    let (current_exists, current_json) = match fs::read(&hooks_path) {
        Ok(raw) => (true, raw),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (false, Vec::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("read {}", hooks_path.display()));
        }
    };
    if current_exists != plan.original_exists || current_json != plan.original_json {
        bail!("Codex hooks.json changed after the preview; nothing was written; run setup again");
    }

    let install_dir = plan.codex_home.join(INSTALL_DIR);
    fs::create_dir_all(&install_dir)
        .with_context(|| format!("create {}", install_dir.display()))?;
    write_if_changed(&install_dir.join("brief-context.sh"), BRIEF_CONTEXT)?;
    write_if_changed(&install_dir.join("handoff-reminder.sh"), HANDOFF_REMINDER)?;
    write_if_changed(&install_dir.join("resolve-space.sh"), RESOLVE_SPACE)?;
    write_if_changed(&hooks_path, &plan.merged_json)
}

pub(super) fn apply_if_approved(plan: &HookPlan, approved: bool) -> anyhow::Result<bool> {
    if !approved || !plan.needs_install {
        return Ok(false);
    }
    apply(plan)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn merge_preserves_existing_hooks_and_is_idempotent() {
        let temp = tempfile::tempdir().unwrap();
        let codex_home = temp.path().join("codex-home");
        fs::create_dir_all(&codex_home).unwrap();
        fs::write(
            codex_home.join("hooks.json"),
            r#"{"hooks":{"Stop":[{"matcher":"keep","hooks":[{"type":"command","command":"existing-stop"}]}]},"custom":"untouched"}"#,
        )
        .unwrap();

        let first = plan(&codex_home).unwrap();
        assert!(first.needs_install);
        assert!(first.merged_json.contains("existing-stop"));
        assert!(first.merged_json.contains("brief-context.sh"));
        assert!(first.merged_json.contains("handoff-reminder.sh"));
        assert!(first.merged_json.contains("WENLAN_HOOK_HOST=codex"));
        apply(&first).unwrap();
        assert_eq!(
            fs::read_to_string(codex_home.join(INSTALL_DIR).join("brief-context.sh")).unwrap(),
            BRIEF_CONTEXT
        );
        assert_eq!(
            fs::read_to_string(codex_home.join(INSTALL_DIR).join("handoff-reminder.sh")).unwrap(),
            HANDOFF_REMINDER
        );
        assert_eq!(
            fs::read_to_string(codex_home.join(INSTALL_DIR).join("resolve-space.sh")).unwrap(),
            RESOLVE_SPACE
        );

        let saved = fs::read_to_string(codex_home.join("hooks.json")).unwrap();
        let second = plan(&codex_home).unwrap();
        assert!(!second.needs_install);
        assert_eq!(second.merged_json, saved);
        assert_eq!(
            fs::read_to_string(codex_home.join("hooks.json")).unwrap(),
            saved
        );
    }

    #[test]
    fn preview_without_approval_does_not_touch_scratch_codex_home() {
        let temp = tempfile::tempdir().unwrap();
        let codex_home = temp.path().join("codex-home");
        fs::create_dir_all(&codex_home).unwrap();
        let original = r#"{"hooks":{"SessionStart":[]},"keep":true}"#;
        fs::write(codex_home.join("hooks.json"), original).unwrap();

        let hook_plan = plan(&codex_home).unwrap();
        assert!(hook_plan.merged_json.contains("brief-context.sh"));
        assert!(!apply_if_approved(&hook_plan, false).unwrap());
        assert_eq!(
            fs::read_to_string(codex_home.join("hooks.json")).unwrap(),
            original
        );
        assert!(!codex_home.join(INSTALL_DIR).exists());
    }

    #[test]
    fn malformed_existing_hooks_are_refused_without_changes() {
        let temp = tempfile::tempdir().unwrap();
        let codex_home = temp.path().join("codex-home");
        fs::create_dir_all(&codex_home).unwrap();
        let path = codex_home.join("hooks.json");
        fs::write(&path, "{not json").unwrap();

        let error = plan(&codex_home).err().expect("malformed JSON must fail");
        assert!(error.to_string().contains("parse"), "{error:#}");
        assert_eq!(fs::read_to_string(path).unwrap(), "{not json");
        assert!(!codex_home.join(INSTALL_DIR).exists());
    }

    #[test]
    fn apply_refuses_hook_file_changed_after_preview() {
        let temp = tempfile::tempdir().unwrap();
        let codex_home = temp.path().join("codex-home");
        fs::create_dir_all(&codex_home).unwrap();
        let path = codex_home.join("hooks.json");
        fs::write(&path, r#"{"hooks":{}}"#).unwrap();
        let hook_plan = plan(&codex_home).unwrap();

        let changed = r#"{"hooks":{},"changed_while_waiting":true}"#;
        fs::write(&path, changed).unwrap();
        let error = apply(&hook_plan).expect_err("stale preview must not overwrite hooks.json");

        assert!(error.to_string().contains("run setup again"), "{error:#}");
        assert_eq!(fs::read_to_string(path).unwrap(), changed);
        assert!(!codex_home.join(INSTALL_DIR).exists());
    }
}
