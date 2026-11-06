// SPDX-License-Identifier: Apache-2.0
//! Custom themes: where theme files live, the checks every interface shares,
//! and turning a theme into the CSS custom properties the app applies.
//!
//! Layout: `<data root>/themes/<id>/theme.json` plus any images it names, and
//! `<data root>/themes/active.json` holding the chosen id. The data root
//! follows `WENLAN_DATA_DIR`, so a scratch daemon never touches real themes.
//!
//! The app never trusts a theme file directly. It asks the daemon for the
//! resolved token map, and the daemon only returns values that passed a strict
//! grammar (hex colors, allowlisted fonts, numbers in range), so a theme
//! cannot smuggle CSS such as `url(...)` into the page.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use wenlan_types::theme::{
    ActiveThemeResponse, ResolvedThemeBackground, ResolvedThemeSticker, ThemeBase, ThemeDetail,
    ThemeFile, ThemeFonts, ThemeImages, ThemeListResponse, ThemeProblem, ThemeSummary,
    THEME_SCHEMA_VERSION,
};

/// Token names a theme's `advanced` block may set, generated from
/// `src/index.css` by `scripts/gen-theme-tokens.mjs`.
const KNOWN_TOKENS: &str = include_str!("known_tokens.txt");

const THEME_FILE: &str = "theme.json";
const ACTIVE_FILE: &str = "active.json";
const MAX_THEME_FILE_BYTES: u64 = 64 * 1024;
const MAX_BACKGROUND_BYTES: u64 = 5 * 1024 * 1024;
const MAX_STICKER_BYTES: u64 = 1024 * 1024;
const MAX_NAME_CHARS: usize = 60;

/// Fonts a theme may name. The app bundles the first three; the rest ship
/// with macOS, Windows or common Linux desktops, or are CSS generic families.
/// Nothing is fetched from the network.
pub const ALLOWED_FONTS: &[&str] = &[
    "Fraunces",
    "Instrument Sans",
    "JetBrains Mono",
    "Georgia",
    "Times New Roman",
    "Palatino",
    "Helvetica Neue",
    "Arial",
    "Verdana",
    "Menlo",
    "Courier New",
    "PingFang TC",
    "PingFang SC",
    "Noto Sans CJK TC",
    "Noto Serif CJK TC",
    "serif",
    "sans-serif",
    "monospace",
    "system-ui",
];

/// A friendly color slot and the CSS tokens it sets.
struct Slot {
    name: &'static str,
    tokens: &'static [&'static str],
    /// Built-in values (dark, light) from `src/index.css`, used when the theme
    /// leaves the slot out and for contrast checks.
    dark: &'static str,
    light: &'static str,
}

const SLOTS: &[Slot] = &[
    Slot {
        name: "background",
        tokens: &["--mem-bg"],
        dark: "#191C24",
        light: "#FCFCFB",
    },
    Slot {
        name: "surface",
        tokens: &["--mem-surface", "--mem-detail-surface", "--mem-popover"],
        dark: "#202430",
        light: "#FFFFFF",
    },
    Slot {
        name: "card",
        tokens: &["--mem-card-surface"],
        dark: "#20232A",
        light: "#F8F8F7",
    },
    Slot {
        name: "sidebar",
        tokens: &["--mem-sidebar"],
        dark: "#161920",
        light: "#FFFFFF",
    },
    Slot {
        name: "text",
        tokens: &["--mem-text", "--mem-brand-text"],
        dark: "#F1EFE8",
        light: "#1A1A2E",
    },
    Slot {
        name: "textSecondary",
        tokens: &["--mem-text-secondary"],
        dark: "#C9C8D1",
        light: "#586174",
    },
    Slot {
        name: "textTertiary",
        tokens: &["--mem-text-tertiary", "--mem-text-meta-aa"],
        dark: "#969BAD",
        light: "#687184",
    },
    Slot {
        name: "border",
        tokens: &["--mem-border"],
        dark: "#333A4B",
        light: "#E3E7EE",
    },
    Slot {
        name: "accent",
        tokens: &["--mem-accent-indigo", "--mem-accent-page"],
        dark: "#A9AEF2",
        light: "#5E58C8",
    },
    Slot {
        name: "textOnAccent",
        tokens: &["--mem-text-on-accent"],
        dark: "#11131A",
        light: "#FFFFFF",
    },
    Slot {
        name: "accentWarm",
        tokens: &["--mem-accent-warm"],
        dark: "#BB8967",
        light: "#B46A3A",
    },
    Slot {
        name: "link",
        tokens: &["--mem-link"],
        dark: "#9095D8",
        light: "#5E58C8",
    },
    Slot {
        name: "danger",
        tokens: &["--mem-status-danger-text", "--mem-danger"],
        dark: "#D98A78",
        light: "#A4442F",
    },
    Slot {
        name: "warning",
        tokens: &["--mem-status-warning-text"],
        dark: "#DCC36F",
        light: "#8C5F16",
    },
    Slot {
        name: "success",
        tokens: &["--mem-status-success-text"],
        dark: "#BBC7AE",
        light: "#4F7558",
    },
    Slot {
        name: "graphProject",
        tokens: &["--kg-project"],
        dark: "#E0AE57",
        light: "#C0851F",
    },
    Slot {
        name: "graphTool",
        tokens: &["--kg-tool"],
        dark: "#6DA9E7",
        light: "#2D78BD",
    },
    Slot {
        name: "graphOrg",
        tokens: &["--kg-org"],
        dark: "#E2805E",
        light: "#C95A37",
    },
    Slot {
        name: "graphPerson",
        tokens: &["--kg-person"],
        dark: "#B99BDE",
        light: "#8160B5",
    },
    Slot {
        name: "graphConcept",
        tokens: &["--kg-concept"],
        dark: "#7CBD89",
        light: "#4A925C",
    },
    Slot {
        name: "graphMemory",
        tokens: &["--kg-memory"],
        dark: "#D7C9AE",
        light: "#B9A987",
    },
    Slot {
        name: "graphPage",
        tokens: &["--kg-page"],
        dark: "#67A6A1",
        light: "#5E9C98",
    },
    Slot {
        name: "graphEdge",
        tokens: &["--kg-edge"],
        dark: "#444956",
        light: "#B9BDC5",
    },
];

/// Translucent tokens made from a slot when the theme sets that slot:
/// (token, slot, alpha on a dark base, alpha on a light base). The alphas are
/// the built-in themes' own.
const DERIVED: &[(&str, &str, f64, f64)] = &[
    ("--mem-indigo-bg", "accent", 0.13, 0.08),
    ("--mem-accent-indigo-border", "accent", 0.42, 0.34),
    ("--mem-focus-ring", "accent", 0.55, 0.45),
    ("--mem-distilled-border", "accent", 0.36, 0.28),
    ("--mem-distilled-bg", "accent", 0.09, 0.06),
    ("--mem-detail-divider", "text", 0.11, 0.10),
    ("--mem-hover", "textTertiary", 0.09, 0.08),
    ("--mem-hover-strong", "textTertiary", 0.15, 0.14),
];

/// Text/background pairs the checks hold to a minimum WCAG contrast ratio.
/// Text pairs use 4.5:1 (WCAG AA body text); tertiary meta text and
/// non-text accent marks use 3:1, the same bar the built-in themes meet.
const CONTRAST_PAIRS: &[(&str, &str, f64)] = &[
    ("text", "background", 4.5),
    ("text", "surface", 4.5),
    ("text", "card", 4.5),
    ("text", "sidebar", 4.5),
    ("textSecondary", "background", 4.5),
    ("textSecondary", "surface", 4.5),
    ("textTertiary", "background", 3.0),
    ("link", "background", 4.5),
    ("textOnAccent", "accent", 4.5),
    ("accent", "background", 3.0),
    ("danger", "background", 3.0),
];

/// `<data root>/themes`.
pub fn themes_dir() -> PathBuf {
    crate::config::data_root().join("themes")
}

/// Ids the API uses as path words, so no theme folder may take them.
const RESERVED_IDS: &[&str] = &["active", "check"];

/// A theme id is its folder name: lowercase letters, digits and dashes.
pub fn valid_theme_id(id: &str) -> bool {
    !id.is_empty()
        && !RESERVED_IDS.contains(&id)
        && id.len() <= 64
        && id.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn problem(field: impl Into<String>, message: impl Into<String>) -> ThemeProblem {
    ThemeProblem {
        field: field.into(),
        message: message.into(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Rgb(u8, u8, u8);

/// Parse `#RRGGBB`; anything else (names, `rgb()`, `url()`) is refused.
fn parse_hex6(value: &str) -> Option<Rgb> {
    let hex = value.strip_prefix('#')?;
    if hex.len() != 6 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some(Rgb(byte(0)?, byte(2)?, byte(4)?))
}

/// `#RRGGBB` or `#RRGGBBAA`.
fn valid_hex_color(value: &str) -> bool {
    let Some(hex) = value.strip_prefix('#') else {
        return false;
    };
    (hex.len() == 6 || hex.len() == 8) && hex.chars().all(|c| c.is_ascii_hexdigit())
}

fn relative_luminance(Rgb(r, g, b): Rgb) -> f64 {
    let channel = |v: u8| {
        let c = f64::from(v) / 255.0;
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
}

/// WCAG 2 contrast ratio between two opaque colors (1.0 to 21.0).
fn contrast_ratio(a: Rgb, b: Rgb) -> f64 {
    let (la, lb) = (relative_luminance(a), relative_luminance(b));
    let (hi, lo) = if la >= lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

fn slot(name: &str) -> Option<&'static Slot> {
    SLOTS.iter().find(|slot| slot.name == name)
}

fn known_tokens() -> impl Iterator<Item = &'static str> {
    KNOWN_TOKENS
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
}

/// The value a slot ends up with: the theme's, or the base theme's built-in.
fn effective_slot(theme: &ThemeFile, slot: &Slot) -> Option<Rgb> {
    match theme.colors.get(slot.name) {
        Some(value) => parse_hex6(value),
        None => parse_hex6(match theme.base {
            ThemeBase::Dark => slot.dark,
            ThemeBase::Light => slot.light,
        }),
    }
}

/// Every check a theme must pass before the app may apply it. `dir` is the
/// theme's own folder, used to check the images it names.
pub fn check_theme(theme: &ThemeFile, dir: &Path) -> Vec<ThemeProblem> {
    let mut problems = Vec::new();
    if theme.schema != THEME_SCHEMA_VERSION {
        problems.push(problem(
            "schema",
            format!(
                "This Wenlan reads theme schema {THEME_SCHEMA_VERSION}; the file says {}.",
                theme.schema
            ),
        ));
    }
    let name = theme.name.trim();
    if name.is_empty()
        || name.chars().count() > MAX_NAME_CHARS
        || name.chars().any(char::is_control)
    {
        problems.push(problem(
            "name",
            format!("Give the theme a name of 1 to {MAX_NAME_CHARS} characters."),
        ));
    }
    if let Some(author) = &theme.author {
        if author.chars().count() > MAX_NAME_CHARS || author.chars().any(char::is_control) {
            problems.push(problem(
                "author",
                format!("Keep the author to {MAX_NAME_CHARS} characters on one line."),
            ));
        }
    }

    let mut colors_ok = true;
    for (key, value) in &theme.colors {
        if slot(key).is_none() {
            let known: Vec<&str> = SLOTS.iter().map(|slot| slot.name).collect();
            problems.push(problem(
                format!("colors.{key}"),
                format!("Unknown color slot. Use one of: {}.", known.join(", ")),
            ));
            colors_ok = false;
        } else if parse_hex6(value).is_none() {
            problems.push(problem(
                format!("colors.{key}"),
                "Write the color as #RRGGBB, for example #E58BC0.",
            ));
            colors_ok = false;
        }
    }
    if colors_ok {
        for (fg, bg, min) in CONTRAST_PAIRS {
            let (Some(fg_slot), Some(bg_slot)) = (slot(fg), slot(bg)) else {
                continue;
            };
            if !theme.colors.contains_key(*fg) && !theme.colors.contains_key(*bg) {
                continue; // both built-in: already meets the bar
            }
            let (Some(a), Some(b)) = (
                effective_slot(theme, fg_slot),
                effective_slot(theme, bg_slot),
            ) else {
                continue;
            };
            let ratio = contrast_ratio(a, b);
            if ratio + 1e-9 < *min {
                problems.push(problem(
                    format!("colors.{fg}"),
                    format!("{fg} on {bg} has contrast {ratio:.1}:1; it needs at least {min}:1 to stay readable."),
                ));
            }
        }
    }

    for (field, font) in [
        ("fonts.heading", &theme.fonts.heading),
        ("fonts.body", &theme.fonts.body),
        ("fonts.mono", &theme.fonts.mono),
    ] {
        if let Some(font) = font {
            if !ALLOWED_FONTS.contains(&font.as_str()) {
                problems.push(problem(
                    field,
                    format!("Use one of these fonts: {}.", ALLOWED_FONTS.join(", ")),
                ));
            }
        }
    }

    if let Some(radius) = theme.radius {
        if !radius.is_finite() || !(0.0..=2.5).contains(&radius) {
            problems.push(problem(
                "radius",
                "Use a corner roundness from 0 (square) to 2.5; 1 is Wenlan's default.",
            ));
        }
    }

    for (key, value) in &theme.advanced {
        if !known_tokens().any(|token| token == key) {
            problems.push(problem(
                format!("advanced.{key}"),
                "Wenlan has no token with this name.",
            ));
        } else if !valid_hex_color(value) {
            problems.push(problem(
                format!("advanced.{key}"),
                "Write the value as #RRGGBB or #RRGGBBAA.",
            ));
        } else if key.starts_with("--kg-") && parse_hex6(value).is_none() {
            problems.push(problem(
                format!("advanced.{key}"),
                "Map colors must be opaque: write #RRGGBB.",
            ));
        }
    }

    if let Some(background) = &theme.images.background {
        if !background.opacity.is_finite() || !(0.0..=1.0).contains(&background.opacity) {
            problems.push(problem(
                "images.background.opacity",
                "Use an opacity from 0 to 1.",
            ));
        }
        if let Err(message) = check_image(dir, &background.file, MAX_BACKGROUND_BYTES) {
            problems.push(problem("images.background.file", message));
        }
    }
    if let Some(sticker) = &theme.images.sticker {
        if !(48..=160).contains(&sticker.size) {
            problems.push(problem(
                "images.sticker.size",
                "Use a sticker size from 48 to 160 pixels.",
            ));
        }
        if let Err(message) = check_image(dir, &sticker.file, MAX_STICKER_BYTES) {
            problems.push(problem("images.sticker.file", message));
        }
    }
    problems
}

/// An image must be a plain file name inside the theme folder (no paths, no
/// links), within the size limit, and really a PNG, JPEG or WebP by its first
/// bytes. SVG is refused because it can carry script.
fn check_image(dir: &Path, file: &str, max_bytes: u64) -> Result<PathBuf, String> {
    let plain = !file.is_empty()
        && !file.starts_with('.')
        && !file.contains(['/', '\\', ':', '\0'])
        && Path::new(file).file_name().and_then(|name| name.to_str()) == Some(file);
    if !plain {
        return Err(
            "Name a file inside the theme folder, like stars.png, with no folder path.".into(),
        );
    }
    let path = dir.join(file);
    let meta = std::fs::symlink_metadata(&path)
        .map_err(|_| format!("{file} is not in the theme folder."))?;
    if !meta.file_type().is_file() {
        return Err(format!(
            "{file} must be a regular file, not a link or folder."
        ));
    }
    if meta.len() > max_bytes {
        return Err(format!(
            "{file} is larger than {} MB.",
            max_bytes / (1024 * 1024)
        ));
    }
    let canonical_dir = std::fs::canonicalize(dir).map_err(|e| e.to_string())?;
    let canonical = std::fs::canonicalize(&path).map_err(|e| e.to_string())?;
    if canonical.parent() != Some(canonical_dir.as_path()) {
        return Err(format!("{file} must sit directly in the theme folder."));
    }
    let mut head = [0u8; 12];
    let read = std::fs::File::open(&canonical)
        .and_then(|mut f| f.read(&mut head))
        .map_err(|_| format!("{file} could not be read."))?;
    let head = &head[..read];
    let is_png = head.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
    let is_jpeg = head.starts_with(&[0xFF, 0xD8, 0xFF]);
    let is_webp = head.len() >= 12 && &head[0..4] == b"RIFF" && &head[8..12] == b"WEBP";
    if !(is_png || is_jpeg || is_webp) {
        return Err(format!("{file} is not a PNG, JPEG or WebP image."));
    }
    Ok(canonical)
}

fn font_stack(font: &str, fallback: &str) -> String {
    let quoted = if font.contains(' ') {
        format!("'{font}'")
    } else {
        font.to_string()
    };
    format!("{quoted}, {fallback}")
}

fn rgba(Rgb(r, g, b): Rgb, alpha: f64) -> String {
    format!("rgba({r}, {g}, {b}, {alpha})")
}

/// CSS custom properties for a theme that passed [`check_theme`]. Only the
/// tokens the theme changes are returned; the app leaves the rest at the base
/// theme's built-in values.
pub fn theme_tokens(theme: &ThemeFile) -> BTreeMap<String, String> {
    let mut tokens = BTreeMap::new();
    let dark = theme.base == ThemeBase::Dark;
    for slot in SLOTS {
        if let Some(value) = theme.colors.get(slot.name) {
            let value = value.to_ascii_uppercase();
            for token in slot.tokens {
                tokens.insert((*token).to_string(), value.clone());
            }
        }
    }
    for (token, slot_name, dark_alpha, light_alpha) in DERIVED {
        if let Some(rgb) = theme.colors.get(*slot_name).and_then(|v| parse_hex6(v)) {
            tokens.insert(
                (*token).to_string(),
                rgba(rgb, if dark { *dark_alpha } else { *light_alpha }),
            );
        }
    }
    if let Some(font) = &theme.fonts.heading {
        tokens.insert(
            "--mem-font-heading".into(),
            font_stack(font, "Georgia, serif"),
        );
    }
    if let Some(font) = &theme.fonts.body {
        tokens.insert(
            "--mem-font-body".into(),
            font_stack(font, "-apple-system, sans-serif"),
        );
    }
    if let Some(font) = &theme.fonts.mono {
        tokens.insert("--mem-font-mono".into(), font_stack(font, "monospace"));
    }
    if let Some(radius) = theme.radius {
        tokens.insert(
            "--mem-radius-scale".into(),
            format!("{}", (radius * 100.0).round() / 100.0),
        );
    }
    for (token, value) in &theme.advanced {
        tokens.insert(token.clone(), value.to_ascii_uppercase());
    }
    tokens
}

/// Read and parse `<dir>/theme.json`.
pub fn read_theme_file(dir: &Path) -> Result<(ThemeFile, Vec<u8>), ThemeProblem> {
    let path = dir.join(THEME_FILE);
    let meta = std::fs::symlink_metadata(&path)
        .map_err(|_| problem("theme.json", "theme.json is missing."))?;
    if !meta.file_type().is_file() {
        return Err(problem("theme.json", "theme.json must be a regular file."));
    }
    if meta.len() > MAX_THEME_FILE_BYTES {
        return Err(problem("theme.json", "theme.json is larger than 64 KB."));
    }
    let bytes = std::fs::read(&path)
        .map_err(|e| problem("theme.json", format!("theme.json could not be read: {e}")))?;
    let theme = serde_json::from_slice::<ThemeFile>(&bytes).map_err(|e| {
        problem(
            "theme.json",
            format!("theme.json is not a valid theme: {e}"),
        )
    })?;
    Ok((theme, bytes))
}

/// Check one theme folder by id under `root`.
pub fn check_theme_id(root: &Path, id: &str) -> Vec<ThemeProblem> {
    if !valid_theme_id(id) {
        return vec![problem(
            "id",
            "A theme id uses lowercase letters, digits and dashes, like night-bunny.",
        )];
    }
    let dir = root.join(id);
    match read_theme_file(&dir) {
        Ok((theme, _)) => check_theme(&theme, &dir),
        Err(problem) => vec![problem],
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ActiveFile {
    id: Option<String>,
}

/// The chosen theme id, if any.
pub fn read_active_id(root: &Path) -> Option<String> {
    let bytes = std::fs::read(root.join(ACTIVE_FILE)).ok()?;
    serde_json::from_slice::<ActiveFile>(&bytes)
        .ok()?
        .id
        .filter(|id| valid_theme_id(id))
}

/// Choose a theme (or `None` for the built-in themes). Refuses a theme that
/// does not pass its checks, so the app never switches to a broken one.
pub fn set_active_id(root: &Path, id: Option<&str>) -> Result<(), Vec<ThemeProblem>> {
    if let Some(id) = id {
        let problems = check_theme_id(root, id);
        if !problems.is_empty() {
            return Err(problems);
        }
    }
    std::fs::create_dir_all(root).map_err(|e| {
        vec![problem(
            "themes",
            format!("Could not create the themes folder: {e}"),
        )]
    })?;
    let body = serde_json::to_vec_pretty(&ActiveFile {
        id: id.map(str::to_string),
    })
    .expect("serialize active theme");
    let tmp = root.join(".active.json.tmp");
    std::fs::write(&tmp, body)
        .and_then(|()| std::fs::rename(&tmp, root.join(ACTIVE_FILE)))
        .map_err(|e| {
            vec![problem(
                "themes",
                format!("Could not save the chosen theme: {e}"),
            )]
        })
}

/// Every theme folder under `root`, with its check result.
pub fn list_themes(root: &Path) -> ThemeListResponse {
    let mut themes = Vec::new();
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let Some(id) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if !file_type.is_dir() || !valid_theme_id(&id) {
                continue;
            }
            let dir = entry.path();
            themes.push(match read_theme_file(&dir) {
                Ok((theme, _)) => ThemeSummary {
                    problems: check_theme(&theme, &dir),
                    id,
                    name: Some(theme.name),
                    base: Some(theme.base),
                    author: theme.author,
                },
                Err(problem) => ThemeSummary {
                    id,
                    name: None,
                    base: None,
                    author: None,
                    problems: vec![problem],
                },
            });
        }
    }
    themes.sort_by(|a, b| a.id.cmp(&b.id));
    ThemeListResponse {
        themes_dir: root.display().to_string(),
        active: read_active_id(root),
        themes,
    }
}

/// What the app should apply right now.
pub fn active_theme(root: &Path) -> ActiveThemeResponse {
    let none =
        |revision: String, problems: Vec<ThemeProblem>, id: Option<String>| ActiveThemeResponse {
            id,
            name: None,
            base: None,
            revision,
            tokens: BTreeMap::new(),
            background: None,
            sticker: None,
            problems,
        };
    let Some(id) = read_active_id(root) else {
        return none("builtin".into(), Vec::new(), None);
    };
    let dir = root.join(&id);
    let (theme, bytes) = match read_theme_file(&dir) {
        Ok(read) => read,
        Err(problem) => return none(format!("broken-{id}"), vec![problem], Some(id)),
    };
    let problems = check_theme(&theme, &dir);
    let mut hasher = Sha256::new();
    hasher.update(id.as_bytes());
    hasher.update(&bytes);
    let mut image = |file: &str| -> Option<String> {
        let path = check_image(&dir, file, MAX_BACKGROUND_BYTES).ok()?;
        if let Ok(meta) = std::fs::metadata(&path) {
            hasher.update(meta.len().to_le_bytes());
            if let Ok(modified) = meta.modified() {
                hasher.update(format!("{modified:?}").as_bytes());
            }
        }
        Some(path.display().to_string())
    };
    let background = theme.images.background.as_ref().and_then(|bg| {
        image(&bg.file).map(|path| ResolvedThemeBackground {
            path,
            placement: bg.placement,
            opacity: bg.opacity,
        })
    });
    let sticker = theme.images.sticker.as_ref().and_then(|st| {
        image(&st.file).map(|path| ResolvedThemeSticker {
            path,
            corner: st.corner,
            size: st.size,
        })
    });
    let revision = format!("{:x}", hasher.finalize())[..16].to_string();
    if !problems.is_empty() {
        return none(revision, problems, Some(id));
    }
    ActiveThemeResponse {
        tokens: theme_tokens(&theme),
        id: Some(id),
        name: Some(theme.name),
        base: Some(theme.base),
        revision,
        background,
        sticker,
        problems,
    }
}

#[cfg(test)]
mod tests;

/// A new theme with every friendly color slot filled with the built-in value
/// for `base`, so the editor starts from what the user sees today.
pub fn template(name: &str, base: ThemeBase) -> ThemeFile {
    let dark = base == ThemeBase::Dark;
    ThemeFile {
        schema: THEME_SCHEMA_VERSION,
        name: name.to_string(),
        base,
        author: None,
        colors: SLOTS
            .iter()
            .map(|slot| {
                (
                    slot.name.to_string(),
                    (if dark { slot.dark } else { slot.light }).to_string(),
                )
            })
            .collect(),
        fonts: ThemeFonts::default(),
        radius: Some(1.0),
        images: ThemeImages::default(),
        advanced: BTreeMap::new(),
    }
}

fn invalid_id_problem() -> ThemeProblem {
    problem(
        "id",
        "A theme id uses lowercase letters, digits and dashes, like night-bunny.",
    )
}

/// One theme file for the editor, with its check result.
pub fn read_theme(root: &Path, id: &str) -> Result<ThemeDetail, Vec<ThemeProblem>> {
    if !valid_theme_id(id) {
        return Err(vec![invalid_id_problem()]);
    }
    let dir = root.join(id);
    let (theme, _) = read_theme_file(&dir).map_err(|p| vec![p])?;
    Ok(ThemeDetail {
        problems: check_theme(&theme, &dir),
        id: id.to_string(),
        theme,
    })
}

/// Check an unsaved theme as if it were saved in the `id` folder, and the
/// tokens it would set when it passes.
pub fn check_unsaved(
    root: &Path,
    id: &str,
    theme: &ThemeFile,
) -> (Vec<ThemeProblem>, BTreeMap<String, String>) {
    if !valid_theme_id(id) {
        return (vec![invalid_id_problem()], BTreeMap::new());
    }
    let problems = check_theme(theme, &root.join(id));
    let tokens = if problems.is_empty() {
        theme_tokens(theme)
    } else {
        BTreeMap::new()
    };
    (problems, tokens)
}

/// Save a theme that passes every check. A failing theme is refused, so a
/// saved file is always one the app can apply.
pub fn save_theme(
    root: &Path,
    id: &str,
    theme: &ThemeFile,
) -> Result<ThemeDetail, Vec<ThemeProblem>> {
    if !valid_theme_id(id) {
        return Err(vec![invalid_id_problem()]);
    }
    let dir = root.join(id);
    let problems = check_theme(theme, &dir);
    if !problems.is_empty() {
        return Err(problems);
    }
    let write_failed = |e: std::io::Error| {
        vec![problem(
            "theme.json",
            format!("Could not save the theme: {e}"),
        )]
    };
    std::fs::create_dir_all(&dir).map_err(write_failed)?;
    let body = serde_json::to_vec_pretty(theme).expect("serialize theme");
    let tmp = dir.join(".theme.json.tmp");
    std::fs::write(&tmp, body)
        .and_then(|()| std::fs::rename(&tmp, dir.join(THEME_FILE)))
        .map_err(write_failed)?;
    Ok(ThemeDetail {
        id: id.to_string(),
        theme: theme.clone(),
        problems: Vec::new(),
    })
}

/// A folder name for a new theme: the name in lowercase with dashes, or
/// `theme` when the name has no ASCII letters, plus `-2`, `-3`... when taken.
fn new_theme_id(root: &Path, name: &str) -> String {
    let mut slug = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let mut slug = slug.trim_end_matches('-').to_string();
    slug.truncate(40);
    let slug = slug.trim_end_matches('-');
    let base = if slug.is_empty() || !valid_theme_id(slug) {
        "theme".to_string()
    } else {
        slug.to_string()
    };
    let mut id = base.clone();
    let mut n = 2;
    while root.join(&id).exists() {
        id = format!("{base}-{n}");
        n += 1;
    }
    id
}

/// Start a new theme from the built-in colors of `base`.
pub fn create_theme(
    root: &Path,
    name: &str,
    base: ThemeBase,
) -> Result<ThemeDetail, Vec<ThemeProblem>> {
    let name = name.trim();
    let id = new_theme_id(root, name);
    save_theme(root, &id, &template(name, base))
}
