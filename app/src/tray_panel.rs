// SPDX-License-Identifier: AGPL-3.0-only
//! The small panel under the menu bar (tray) icon: what needs the user, the
//! pages Wenlan wrote lately, and the ways in. Clicking the icon toggles it;
//! it hides when it loses focus. The panel's content lives in the frontend
//! (`#tray-panel`); this module places, shows and hides the window and hands
//! the panel's actions to the main window.

use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, WebviewWindow};

pub const LABEL: &str = "tray-panel";
pub const WIDTH: f64 = 320.0;
pub const INITIAL_HEIGHT: f64 = 360.0;
pub const MAX_HEIGHT: f64 = 560.0;
/// Space between the icon and the panel.
const GAP: f64 = 6.0;
/// The panel never touches the screen edge.
const EDGE: f64 = 8.0;
/// Clicking the icon while the panel is open first takes focus from the
/// panel, which hides it, and only then delivers the click. A click this soon
/// after a focus-loss hide is that same click, so it must not reopen.
const REOPEN_GUARD: Duration = Duration::from_millis(350);

/// Sent to the panel each time it opens, so it rereads its data.
pub const OPENED_EVENT: &str = "tray-panel-opened";
/// Sent to the main window with a [`TrayPanelTarget`].
pub const NAVIGATE_EVENT: &str = "tray-panel-navigate";

/// A rectangle in physical pixels, origin top-left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

#[derive(Default)]
struct Inner {
    /// Where the icon was at the last click; the panel hangs from it.
    anchor: Option<Rect>,
    /// Content height the panel asked for, in logical pixels.
    height: Option<f64>,
    hidden_on_blur_at: Option<Instant>,
}

#[derive(Default)]
pub struct TrayPanelState(Mutex<Inner>);

impl TrayPanelState {
    fn with<T>(&self, f: impl FnOnce(&mut Inner) -> T) -> T {
        let mut guard = self.0.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut guard)
    }
}

/// Where the main window should go when the panel hands over.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum TrayPanelTarget {
    Home,
    Page {
        #[serde(rename = "pageId")]
        page_id: String,
    },
    Review,
    Search,
}

/// The daemon as the tray icon sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TrayDaemonState {
    Starting,
    Up,
    Down,
}

/// Top-left corner for a `w` x `h` panel hanging from `icon`, kept inside
/// `area`. A menu bar icon sits at the top of the screen, so the panel opens
/// below it; a taskbar icon at the bottom opens the panel above it.
pub fn panel_origin(icon: Rect, area: Rect, w: f64, h: f64, gap: f64, edge: f64) -> (f64, f64) {
    let min_x = area.x + edge;
    let max_x = (area.x + area.w - w - edge).max(min_x);
    let x = (icon.x + icon.w / 2.0 - w / 2.0).clamp(min_x, max_x);

    let opens_below = icon.y + icon.h / 2.0 < area.y + area.h / 2.0;
    let y = if opens_below {
        icon.y + icon.h + gap
    } else {
        icon.y - gap - h
    };
    let min_y = area.y;
    let max_y = (area.y + area.h - h - edge).max(min_y);
    (x, y.clamp(min_y, max_y))
}

fn to_physical(position: tauri::Position, size: tauri::Size, scale: f64) -> Rect {
    let p = position.to_physical::<f64>(scale);
    let s = size.to_physical::<f64>(scale);
    Rect {
        x: p.x,
        y: p.y,
        w: s.width,
        h: s.height,
    }
}

/// The icon's rectangle from a tray click.
pub fn icon_rect(app: &AppHandle, rect: &tauri::Rect) -> Rect {
    let scale = app
        .primary_monitor()
        .ok()
        .flatten()
        .map(|m| m.scale_factor())
        .unwrap_or(1.0);
    to_physical(rect.position, rect.size, scale)
}

fn place(app: &AppHandle, win: &WebviewWindow) -> Result<(), String> {
    let state = app.state::<TrayPanelState>();
    let (anchor, height) = state.with(|inner| (inner.anchor, inner.height));
    let height = height.unwrap_or(INITIAL_HEIGHT).clamp(80.0, MAX_HEIGHT);

    let monitor = match anchor {
        Some(icon) => app
            .monitor_from_point(icon.x + icon.w / 2.0, icon.y + icon.h / 2.0)
            .ok()
            .flatten(),
        None => None,
    }
    .or_else(|| app.primary_monitor().ok().flatten())
    .ok_or("no monitor available")?;
    let scale = monitor.scale_factor();
    let work = monitor.work_area();
    let area = Rect {
        x: work.position.x as f64,
        y: work.position.y as f64,
        w: work.size.width as f64,
        h: work.size.height as f64,
    };
    // Without a click to hang from (a keyboard open), use the top-right
    // corner, where the menu bar icon lives on macOS.
    let icon = anchor.unwrap_or(Rect {
        x: area.x + area.w - EDGE * scale,
        y: area.y - 1.0,
        w: 0.0,
        h: 0.0,
    });
    let (w, h) = (WIDTH * scale, height * scale);
    let (x, y) = panel_origin(icon, area, w, h, GAP * scale, EDGE * scale);
    win.set_size(tauri::PhysicalSize::new(w.round() as u32, h.round() as u32))
        .map_err(|e| e.to_string())?;
    win.set_position(tauri::PhysicalPosition::new(
        x.round() as i32,
        y.round() as i32,
    ))
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn show(win: &WebviewWindow) {
    crate::quick_capture::show_floating(win);
    // Activates the app too, so the panel gets focus and hides again when
    // the user clicks elsewhere.
    let _ = win.set_focus();
}

/// Hide without bringing the main window forward.
pub fn hide(win: &WebviewWindow) {
    let _ = crate::quick_capture::hide_quietly(win);
}

/// Open or close the panel from a click on the icon at `icon`.
pub fn toggle(app: &AppHandle, icon: Option<Rect>) {
    let Some(win) = app.get_webview_window(LABEL) else {
        return;
    };
    if win.is_visible().unwrap_or(false) {
        hide(&win);
        return;
    }
    let state = app.state::<TrayPanelState>();
    let same_click = state.with(|inner| {
        if let Some(icon) = icon {
            inner.anchor = Some(icon);
        }
        inner
            .hidden_on_blur_at
            .take()
            .is_some_and(|at| at.elapsed() < REOPEN_GUARD)
    });
    if same_click {
        return;
    }
    if let Err(e) = place(app, &win) {
        log::warn!("[tray-panel] could not place the panel: {e}");
    }
    show(&win);
    let _ = app.emit_to(LABEL, OPENED_EVENT, ());
}

/// Hide the panel when it loses focus.
pub fn on_focus_lost(app: &AppHandle) {
    if let Some(win) = app.get_webview_window(LABEL) {
        if win.is_visible().unwrap_or(false) {
            app.state::<TrayPanelState>()
                .with(|inner| inner.hidden_on_blur_at = Some(Instant::now()));
            hide(&win);
        }
    }
}

#[tauri::command]
pub async fn tray_panel_hide(app: AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window(LABEL) {
        hide(&win);
    }
    Ok(())
}

/// The panel grows with its content; it reports its height here.
#[tauri::command]
pub async fn tray_panel_set_height(app: AppHandle, height: f64) -> Result<(), String> {
    if !height.is_finite() || height <= 0.0 {
        return Err("height must be a positive number".into());
    }
    app.state::<TrayPanelState>()
        .with(|inner| inner.height = Some(height));
    let win = app
        .get_webview_window(LABEL)
        .ok_or("tray panel not found")?;
    place(&app, &win)
}

/// Hide the panel, bring the main window forward and send it to `target`.
#[tauri::command]
pub async fn tray_panel_open_main(app: AppHandle, target: TrayPanelTarget) -> Result<(), String> {
    if let Some(win) = app.get_webview_window(LABEL) {
        hide(&win);
    }
    let for_main = app.clone();
    app.run_on_main_thread(move || {
        crate::reveal_main_window(&for_main);
        let result = match &target {
            // Main's ⌘K listener opens the search dialog.
            TrayPanelTarget::Search => for_main.emit_to("main", "focus-search", ()),
            _ => for_main.emit_to("main", NAVIGATE_EVENT, &target),
        };
        if let Err(e) = result {
            log::warn!("[tray-panel] could not send the main window to {target:?}: {e}");
        }
    })
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn tray_panel_daemon_state(app: AppHandle) -> TrayDaemonState {
    match app.try_state::<crate::tray_health::HealthSignal>() {
        Some(signal) => match signal.current() {
            crate::tray_health::DaemonState::Up => TrayDaemonState::Up,
            crate::tray_health::DaemonState::Down => TrayDaemonState::Down,
            crate::tray_health::DaemonState::Starting => TrayDaemonState::Starting,
        },
        None => TrayDaemonState::Starting,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAC_SCREEN: Rect = Rect {
        x: 0.0,
        y: 50.0,
        w: 3024.0,
        h: 1914.0,
    };

    #[test]
    fn opens_below_a_menu_bar_icon_centred_on_it() {
        let icon = Rect {
            x: 2400.0,
            y: 0.0,
            w: 44.0,
            h: 48.0,
        };
        let (x, y) = panel_origin(icon, MAC_SCREEN, 640.0, 720.0, 12.0, 16.0);
        assert_eq!(x, 2422.0 - 320.0);
        assert_eq!(y, 60.0);
    }

    #[test]
    fn stays_on_screen_for_an_icon_at_the_right_edge() {
        let icon = Rect {
            x: 2990.0,
            y: 0.0,
            w: 34.0,
            h: 48.0,
        };
        let (x, _) = panel_origin(icon, MAC_SCREEN, 640.0, 720.0, 12.0, 16.0);
        assert_eq!(x, 3024.0 - 640.0 - 16.0);
    }

    #[test]
    fn opens_above_a_taskbar_icon() {
        let area = Rect {
            x: 0.0,
            y: 0.0,
            w: 1920.0,
            h: 1040.0,
        };
        let icon = Rect {
            x: 1700.0,
            y: 1048.0,
            w: 24.0,
            h: 24.0,
        };
        let (_, y) = panel_origin(icon, area, 320.0, 360.0, 6.0, 8.0);
        assert_eq!(y, 1040.0 - 360.0 - 8.0);
    }

    #[test]
    fn target_wire_shape_matches_the_frontend() {
        assert_eq!(
            serde_json::from_str::<TrayPanelTarget>(r#"{"kind":"page","pageId":"p1"}"#).unwrap(),
            TrayPanelTarget::Page {
                page_id: "p1".into()
            }
        );
        assert_eq!(
            serde_json::to_string(&TrayPanelTarget::Review).unwrap(),
            r#"{"kind":"review"}"#
        );
        assert_eq!(
            serde_json::to_string(&TrayDaemonState::Down).unwrap(),
            r#""down""#
        );
    }
}
