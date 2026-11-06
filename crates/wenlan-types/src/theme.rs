// SPDX-License-Identifier: Apache-2.0
//! Custom theme file format and the daemon's theme API shapes.
//!
//! A theme lives in `<data root>/themes/<id>/theme.json`, next to any image
//! it uses. The daemon checks it and turns it into CSS custom properties; the
//! app only applies what the daemon returns, so every interface (app, CLI,
//! MCP) sees the same verdict.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The only theme file schema this version understands.
pub const THEME_SCHEMA_VERSION: u32 = 1;

/// Which built-in theme a custom theme starts from. Every color the theme
/// leaves out keeps this base's value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeBase {
    Dark,
    Light,
}

/// `theme.json` as written by a person or an AI tool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThemeFile {
    pub schema: u32,
    pub name: String,
    pub base: ThemeBase,
    /// Who made it, e.g. "Claude Code" or "Qi-Xuan". Display only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// Friendly color slots (`background`, `accent`, ...) to `#RRGGBB`.
    #[serde(default)]
    pub colors: BTreeMap<String, String>,
    #[serde(default)]
    pub fonts: ThemeFonts,
    /// Corner roundness multiplier: 0 = square, 1 = Wenlan's default, 2 = twice as round.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radius: Option<f64>,
    #[serde(default)]
    pub images: ThemeImages,
    /// Direct CSS token overrides (`--kg-person`: `#F2A7D0`) for anything the
    /// friendly slots do not cover.
    #[serde(default)]
    pub advanced: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThemeFonts {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heading: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mono: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThemeImages {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<ThemeBackground>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sticker: Option<ThemeSticker>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BackgroundPlacement {
    /// Behind the sidebar only; reading text stays on a plain surface.
    Sidebar,
    /// Behind the whole main window, dimmed under the content.
    Window,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThemeBackground {
    /// File name inside the theme's own folder (png, jpg or webp).
    pub file: String,
    #[serde(default = "default_background_placement")]
    pub placement: BackgroundPlacement,
    /// 0.0 to 1.0.
    #[serde(default = "default_background_opacity")]
    pub opacity: f64,
}

fn default_background_placement() -> BackgroundPlacement {
    BackgroundPlacement::Sidebar
}

fn default_background_opacity() -> f64 {
    0.35
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StickerCorner {
    BottomRight,
    BottomLeft,
    TopRight,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThemeSticker {
    /// File name inside the theme's own folder (png, jpg or webp).
    pub file: String,
    #[serde(default = "default_sticker_corner")]
    pub corner: StickerCorner,
    /// Longest side in CSS pixels, 48 to 160.
    #[serde(default = "default_sticker_size")]
    pub size: u32,
}

fn default_sticker_corner() -> StickerCorner {
    StickerCorner::BottomRight
}

fn default_sticker_size() -> u32 {
    88
}

/// One reason a theme cannot be applied, in words a person or an AI tool can
/// act on. `field` points into theme.json, e.g. `colors.text`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThemeProblem {
    pub field: String,
    pub message: String,
}

/// One theme folder as `GET /api/themes` lists it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThemeSummary {
    pub id: String,
    /// `None` when theme.json could not be read at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<ThemeBase>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// Empty when the theme passes every check.
    #[serde(default)]
    pub problems: Vec<ThemeProblem>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThemeListResponse {
    /// Absolute path of the themes folder, so an AI tool knows where to write.
    pub themes_dir: String,
    /// Id of the theme in use; `None` means the built-in light/dark/system choice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<String>,
    pub themes: Vec<ThemeSummary>,
}

/// An image the app may show, already checked by the daemon.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolvedThemeImage {
    /// Absolute path of the checked file.
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolvedThemeBackground {
    pub path: String,
    pub placement: BackgroundPlacement,
    pub opacity: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolvedThemeSticker {
    pub path: String,
    pub corner: StickerCorner,
    pub size: u32,
}

/// What the app applies: `GET /api/themes/active`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActiveThemeResponse {
    /// `None` = no custom theme; use the built-in light/dark/system choice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<ThemeBase>,
    /// Changes whenever the applied result would change (file edited, theme
    /// switched). The app polls this to pick up edits an AI tool made.
    pub revision: String,
    /// CSS custom property name to value, ready for `style.setProperty`.
    #[serde(default)]
    pub tokens: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<ResolvedThemeBackground>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sticker: Option<ResolvedThemeSticker>,
    /// Set when the chosen theme no longer passes its checks; the app then
    /// shows the built-in theme instead of a half-applied one.
    #[serde(default)]
    pub problems: Vec<ThemeProblem>,
}

/// `PUT /api/themes/active`. `id: null` goes back to the built-in themes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetActiveThemeRequest {
    pub id: Option<String>,
}

/// `POST /api/themes/check` request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckThemeRequest {
    pub id: String,
}

/// `POST /api/themes/check` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThemeCheckResponse {
    pub id: String,
    pub ok: bool,
    pub problems: Vec<ThemeProblem>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_theme_file_parses_with_defaults() {
        let theme: ThemeFile = serde_json::from_str(
            r##"{"schema":1,"name":"Night","base":"dark","colors":{"accent":"#E58BC0"},
                "images":{"sticker":{"file":"bunny.png"}}}"##,
        )
        .unwrap();
        assert_eq!(theme.base, ThemeBase::Dark);
        assert_eq!(theme.colors["accent"], "#E58BC0");
        let sticker = theme.images.sticker.unwrap();
        assert_eq!(sticker.corner, StickerCorner::BottomRight);
        assert_eq!(sticker.size, 88);
    }

    #[test]
    fn unknown_fields_are_refused() {
        let err = serde_json::from_str::<ThemeFile>(
            r#"{"schema":1,"name":"x","base":"dark","css":"body{}"}"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("unknown field"));
    }
}
