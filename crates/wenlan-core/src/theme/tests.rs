// SPDX-License-Identifier: Apache-2.0
use super::*;

const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0];

fn write_theme(root: &Path, id: &str, json: &str) -> PathBuf {
    let dir = root.join(id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("theme.json"), json).unwrap();
    dir
}

fn parse(json: &str) -> ThemeFile {
    serde_json::from_str(json).unwrap()
}

#[test]
fn built_in_slot_values_meet_their_own_contrast_bar() {
    for base in [ThemeBase::Dark, ThemeBase::Light] {
        for (fg, bg, min) in CONTRAST_PAIRS {
            let value = |name: &str| {
                let slot = slot(name).unwrap();
                parse_hex6(if base == ThemeBase::Dark {
                    slot.dark
                } else {
                    slot.light
                })
                .unwrap()
            };
            let ratio = contrast_ratio(value(fg), value(bg));
            assert!(
                ratio >= *min,
                "{base:?} {fg} on {bg} = {ratio:.2}, below {min}"
            );
        }
    }
}

#[test]
fn built_in_slot_values_match_index_css() {
    let css = include_str!("../../../../src/index.css");
    // The block that opens with `start` and defines --mem-bg (light mode has
    // more than one `html[data-theme="light"]` block).
    let block = |start: &str| {
        css.match_indices(start)
            .map(|(at, _)| {
                let body = &css[at..];
                body[..body.find("\n}").unwrap()].to_string()
            })
            .find(|body| body.contains("--mem-bg:"))
            .unwrap_or_else(|| panic!("missing {start}"))
    };
    let dark = block(":root,\nhtml[data-theme=\"dark\"] {");
    let light = block("html[data-theme=\"light\"] {");
    let value = |body: &str, token: &str| {
        let at = body
            .find(&format!("{token}:"))
            .unwrap_or_else(|| panic!("missing {token}"));
        body[at + token.len() + 1..]
            .split(';')
            .next()
            .unwrap()
            .trim()
            .to_string()
    };
    for slot in SLOTS {
        let token = slot.tokens[0];
        assert_eq!(value(&dark, token), slot.dark, "dark {token}");
        assert_eq!(value(&light, token), slot.light, "light {token}");
    }
}

#[test]
fn contrast_ratio_matches_wcag_examples() {
    let black = Rgb(0, 0, 0);
    let white = Rgb(255, 255, 255);
    assert!((contrast_ratio(black, white) - 21.0).abs() < 1e-6);
    assert!((contrast_ratio(white, white) - 1.0).abs() < 1e-6);
}

#[test]
fn good_theme_passes_and_becomes_tokens() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = write_theme(
        tmp.path(),
        "night-bunny",
        r##"{"schema":1,"name":"Night Bunny","base":"dark","author":"Claude Code",
            "colors":{"background":"#15101E","surface":"#1F1729","text":"#F3ECF8","accent":"#F29AC9"},
            "fonts":{"heading":"Fraunces"},"radius":1.4}"##,
    );
    let (theme, _) = read_theme_file(&dir).unwrap();
    assert_eq!(check_theme(&theme, &dir), vec![]);
    let tokens = theme_tokens(&theme);
    assert_eq!(tokens["--mem-bg"], "#15101E");
    assert_eq!(tokens["--mem-detail-surface"], "#1F1729");
    assert_eq!(tokens["--mem-accent-indigo"], "#F29AC9");
    assert_eq!(tokens["--mem-focus-ring"], "rgba(242, 154, 201, 0.55)");
    assert_eq!(tokens["--mem-font-heading"], "Fraunces, Georgia, serif");
    assert_eq!(tokens["--mem-radius-scale"], "1.4");
    assert!(
        !tokens.contains_key("--mem-link"),
        "unset slots keep the built-in value"
    );
}

#[test]
fn unreadable_text_is_refused_with_the_pair_named() {
    let theme = parse(r##"{"schema":1,"name":"Fog","base":"dark","colors":{"text":"#2A2A2A"}}"##);
    let problems = check_theme(&theme, Path::new("."));
    assert!(problems
        .iter()
        .any(|p| p.field == "colors.text" && p.message.contains("text on background")));
}

#[test]
fn css_injection_and_unknown_names_are_refused() {
    let theme = parse(
        r##"{"schema":1,"name":"Bad","base":"dark",
            "colors":{"accent":"red; background:url(http://x)","glow":"#FFFFFF"},
            "fonts":{"body":"x; background:url(http://x)"},
            "advanced":{"--mem-bg":"url(http://x)","--not-a-token":"#FFFFFF","--kg-person":"#FF00FF80"}}"##,
    );
    let fields: Vec<String> = check_theme(&theme, Path::new("."))
        .into_iter()
        .map(|p| p.field)
        .collect();
    for field in [
        "colors.accent",
        "colors.glow",
        "fonts.body",
        "advanced.--mem-bg",
        "advanced.--not-a-token",
        "advanced.--kg-person",
    ] {
        assert!(
            fields.iter().any(|f| f == field),
            "expected a problem for {field}: {fields:?}"
        );
    }
}

#[test]
fn radius_and_schema_are_range_checked() {
    let theme = parse(r##"{"schema":2,"name":"R","base":"light","radius":9}"##);
    let fields: Vec<String> = check_theme(&theme, Path::new("."))
        .into_iter()
        .map(|p| p.field)
        .collect();
    assert!(fields.contains(&"schema".to_string()));
    assert!(fields.contains(&"radius".to_string()));
}

#[test]
fn images_must_be_real_images_inside_the_theme_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = write_theme(tmp.path(), "pics", "{}");
    std::fs::write(dir.join("stars.png"), PNG).unwrap();
    std::fs::write(dir.join("fake.png"), b"<svg onload=alert(1)>").unwrap();
    std::fs::write(tmp.path().join("secret.png"), PNG).unwrap();

    assert!(check_image(&dir, "stars.png", MAX_STICKER_BYTES).is_ok());
    assert!(check_image(&dir, "fake.png", MAX_STICKER_BYTES)
        .unwrap_err()
        .contains("not a PNG"));
    assert!(check_image(&dir, "../secret.png", MAX_STICKER_BYTES).is_err());
    assert!(check_image(&dir, "/etc/passwd", MAX_STICKER_BYTES).is_err());
    assert!(check_image(&dir, "missing.png", MAX_STICKER_BYTES).is_err());
    assert!(check_image(&dir, "stars.png", 4)
        .unwrap_err()
        .contains("larger"));
}

#[test]
fn theme_ids_are_folder_safe() {
    assert!(valid_theme_id("night-bunny"));
    assert!(valid_theme_id("2026"));
    for bad in [
        "", "active", "check", "Night", "../x", "a/b", "-x", ".hidden", "x y",
    ] {
        assert!(!valid_theme_id(bad), "{bad}");
    }
}

#[test]
fn choosing_a_theme_refuses_broken_ones_and_reset_clears_it() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_theme(
        root,
        "ok",
        r##"{"schema":1,"name":"Ok","base":"light","colors":{"accent":"#3D5A99"}}"##,
    );
    write_theme(
        root,
        "broken",
        r#"{"schema":1,"name":"Broken","base":"dark","colors":{"text":"nope"}}"#,
    );

    assert!(set_active_id(root, Some("broken")).is_err());
    assert_eq!(read_active_id(root), None);
    assert_eq!(active_theme(root).revision, "builtin");

    set_active_id(root, Some("ok")).unwrap();
    let active = active_theme(root);
    assert_eq!(active.id.as_deref(), Some("ok"));
    assert_eq!(active.tokens["--mem-accent-indigo"], "#3D5A99");
    assert!(active.problems.is_empty());
    let first = active.revision.clone();

    // An AI tool edits the file: the revision changes so the app reloads.
    write_theme(
        root,
        "ok",
        r##"{"schema":1,"name":"Ok","base":"light","colors":{"accent":"#2F4A80"}}"##,
    );
    assert_ne!(active_theme(root).revision, first);

    // Broken after being chosen: nothing is applied and the problems say why.
    write_theme(
        root,
        "ok",
        r#"{"schema":1,"name":"Ok","base":"light","colors":{"accent":"blue"}}"#,
    );
    let broken = active_theme(root);
    assert!(broken.tokens.is_empty());
    assert!(!broken.problems.is_empty());

    set_active_id(root, None).unwrap();
    assert_eq!(active_theme(root).id, None);
}

#[test]
fn list_reports_each_theme_and_where_to_write() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_theme(root, "ok", r##"{"schema":1,"name":"Ok","base":"dark"}"##);
    write_theme(root, "garbled", "not json");
    std::fs::create_dir_all(root.join("Not-Valid")).unwrap();
    let list = list_themes(root);
    assert_eq!(list.themes_dir, root.display().to_string());
    let ids: Vec<&str> = list.themes.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids, ["garbled", "ok"]);
    assert!(list.themes[1].problems.is_empty());
    assert_eq!(list.themes[0].problems[0].field, "theme.json");
}

#[test]
fn known_tokens_cover_every_slot_and_derived_token() {
    let known: Vec<&str> = known_tokens().collect();
    for slot in SLOTS {
        for token in slot.tokens {
            assert!(known.contains(token), "{token}");
        }
    }
    for (token, slot_name, _, _) in DERIVED {
        assert!(known.contains(token), "{token}");
        assert!(slot(slot_name).is_some(), "{slot_name}");
    }
}

#[test]
fn new_themes_start_from_built_in_colors_and_get_free_folder_names() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let first = create_theme(root, "Night Bunny!", ThemeBase::Dark).unwrap();
    assert_eq!(first.id, "night-bunny");
    assert_eq!(first.theme.colors.len(), SLOTS.len());
    assert_eq!(first.theme.colors["background"], "#191C24");
    assert!(first.problems.is_empty());
    let second = create_theme(root, "Night Bunny", ThemeBase::Light).unwrap();
    assert_eq!(second.id, "night-bunny-2");
    assert_eq!(second.theme.colors["background"], "#FCFCFB");
    let unnamed_ascii = create_theme(root, "夜晚兔子", ThemeBase::Dark).unwrap();
    assert_eq!(unnamed_ascii.id, "theme");
    assert_eq!(unnamed_ascii.theme.name, "夜晚兔子");
    assert!(create_theme(root, "  ", ThemeBase::Dark).is_err());
    let read = read_theme(root, "night-bunny").unwrap();
    assert_eq!(read.theme, first.theme);
}

#[test]
fn saving_refuses_a_failing_theme_and_keeps_the_old_file() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mut detail = create_theme(root, "Paper", ThemeBase::Light).unwrap();
    let before = std::fs::read(root.join("paper/theme.json")).unwrap();
    detail.theme.colors.insert("text".into(), "#FCFCFB".into());
    let problems = save_theme(root, "paper", &detail.theme).unwrap_err();
    assert!(
        problems.iter().any(|p| p.field.contains("text")),
        "{problems:?}"
    );
    assert_eq!(
        std::fs::read(root.join("paper/theme.json")).unwrap(),
        before
    );

    let (problems, tokens) = check_unsaved(root, "paper", &detail.theme);
    assert!(!problems.is_empty());
    assert!(tokens.is_empty());

    detail
        .theme
        .colors
        .insert("accent".into(), "#4A3FB0".into());
    detail.theme.colors.insert("text".into(), "#1A1A2E".into());
    let (problems, tokens) = check_unsaved(root, "paper", &detail.theme);
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(tokens["--mem-accent-indigo"], "#4A3FB0");
    save_theme(root, "paper", &detail.theme).unwrap();
    assert_eq!(
        read_theme(root, "paper").unwrap().theme.colors["accent"],
        "#4A3FB0"
    );
    assert!(save_theme(root, "../x", &detail.theme).is_err());
}
