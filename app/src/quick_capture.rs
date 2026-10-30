// SPDX-License-Identifier: AGPL-3.0-only
//! Placement and show/hide plumbing for the quick-capture window.
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, WebviewWindow};

pub const QC_WIDTH: f64 = 560.0;
pub const QC_HEIGHT: f64 = 360.0;
pub const QC_CORNER_PADDING: f64 = 16.0;
pub const QC_OPENED_EVENT: &str = "quick-capture-opened";
pub const QC_CLOSED_EVENT: &str = "quick-capture-closed";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum QuickCapturePlacement {
    #[default]
    BottomRight,
    CenteredOverMain,
}

pub struct QuickCapturePlacementState(pub Mutex<QuickCapturePlacement>);

/// Top-left origin that centers a `w` x `h` window over a rectangle at (`x`, `y`) of `mw` x `mh`.
pub fn centered_origin(x: f64, y: f64, mw: f64, mh: f64, w: f64, h: f64) -> (f64, f64) {
    (x + (mw - w) / 2.0, y + (mh - h) / 2.0)
}

pub fn set_placement(app: &AppHandle, placement: QuickCapturePlacement) {
    if let Some(state) = app.try_state::<QuickCapturePlacementState>() {
        let mut guard = state.0.lock().unwrap_or_else(|e| e.into_inner());
        *guard = placement;
    }
}

pub fn placement(app: &AppHandle) -> QuickCapturePlacement {
    app.try_state::<QuickCapturePlacementState>()
        .map(|state| *state.0.lock().unwrap_or_else(|e| e.into_inner()))
        .unwrap_or_default()
}

pub fn apply_placement(app: &AppHandle) -> Result<(), String> {
    match placement(app) {
        QuickCapturePlacement::CenteredOverMain => {
            let win = app
                .get_webview_window("quick-capture")
                .ok_or("quick-capture window not found")?;
            let Some(main) = app.get_webview_window("main") else {
                return position_bottom_right(&win);
            };
            let main_visible = main.is_visible().unwrap_or(false);
            let main_minimized = main.is_minimized().unwrap_or(false);
            if !main_visible || main_minimized {
                return position_bottom_right(&win);
            }
            // Physical pixels end to end: `outer_position`/`outer_size` are
            // physical, and `set_position(LogicalPosition)` would reconvert
            // with the capture window's scale, which differs on mixed-scale
            // monitors.
            let pos = main.outer_position().map_err(|e| e.to_string())?;
            let size = main.outer_size().map_err(|e| e.to_string())?;
            let scale = main.scale_factor().map_err(|e| e.to_string())?;
            let (qw, qh) = (QC_WIDTH * scale, QC_HEIGHT * scale);
            let (x, y) = centered_origin(
                pos.x as f64,
                pos.y as f64,
                size.width as f64,
                size.height as f64,
                qw,
                qh,
            );
            win.set_size(tauri::PhysicalSize::new(
                qw.round() as u32,
                qh.round() as u32,
            ))
            .map_err(|e| e.to_string())?;
            win.set_position(tauri::PhysicalPosition::new(
                x.round() as i32,
                y.round() as i32,
            ))
            .map_err(|e| e.to_string())?;
            Ok(())
        }
        QuickCapturePlacement::BottomRight => {
            let win = app
                .get_webview_window("quick-capture")
                .ok_or("quick-capture window not found")?;
            position_bottom_right(&win)
        }
    }
}

fn position_bottom_right(win: &WebviewWindow) -> Result<(), String> {
    #[cfg(not(target_os = "macos"))]
    {
        let monitor = win
            .current_monitor()
            .map_err(|e| e.to_string())?
            .or_else(|| win.primary_monitor().ok().flatten())
            .ok_or("no monitor available")?;
        let scale = monitor.scale_factor();
        let (qw, qh, pad) = (
            QC_WIDTH * scale,
            QC_HEIGHT * scale,
            QC_CORNER_PADDING * scale,
        );
        let area = monitor.work_area();
        let x = area.position.x as f64 + area.size.width as f64 - qw - pad;
        let y = area.position.y as f64 + area.size.height as f64 - qh - pad;
        win.set_size(tauri::PhysicalSize::new(
            qw.round() as u32,
            qh.round() as u32,
        ))
        .map_err(|e| e.to_string())?;
        win.set_position(tauri::PhysicalPosition::new(
            x.round() as i32,
            y.round() as i32,
        ))
        .map_err(|e| e.to_string())?;
    }

    #[cfg(target_os = "macos")]
    #[allow(deprecated)]
    {
        use cocoa::base::id;
        use cocoa::foundation::NSRect;
        use raw_window_handle::HasWindowHandle;
        use tauri::{LogicalPosition, LogicalSize};

        let raw_handle = win.window_handle().map_err(|e| e.to_string())?;
        if let raw_window_handle::RawWindowHandle::AppKit(appkit) = raw_handle.as_raw() {
            let ns_view = appkit.ns_view.as_ptr() as id;

            let (visible, screen_h) = unsafe {
                let ns_win: id = objc::msg_send![ns_view, window];
                if ns_win.is_null() {
                    return Err("NSWindow not attached".into());
                }
                let screen: id = objc::msg_send![ns_win, screen];
                if screen.is_null() {
                    return Err("NSScreen not available".into());
                }
                let visible: NSRect = objc::msg_send![screen, visibleFrame];
                let frame: NSRect = objc::msg_send![screen, frame];
                (visible, frame.size.height)
            };

            let width = QC_WIDTH;
            let height = QC_HEIGHT;
            let padding = QC_CORNER_PADDING;

            win.set_size(LogicalSize::new(width, height))
                .map_err(|e| e.to_string())?;

            let x = visible.origin.x + visible.size.width - width - padding;
            let y = screen_h - visible.origin.y - padding - height;

            log::debug!("[qc-pos] visible=({:.0},{:.0} {:.0}x{:.0}) screen_h={:.0} → size=({:.0},{:.0}) pos=({:.0},{:.0})",
                visible.origin.x, visible.origin.y, visible.size.width, visible.size.height,
                screen_h, width, height, x, y);

            win.set_position(LogicalPosition::new(x, y))
                .map_err(|e| e.to_string())?;
        }
    }

    Ok(())
}

pub fn show_floating(window: &WebviewWindow) {
    #[cfg(target_os = "macos")]
    #[allow(deprecated)]
    {
        use cocoa::base::id;
        use raw_window_handle::HasWindowHandle;

        if let Ok(raw_handle) = window.window_handle() {
            if let raw_window_handle::RawWindowHandle::AppKit(appkit) = raw_handle.as_raw() {
                let ns_view = appkit.ns_view.as_ptr() as id;
                unsafe {
                    let ns_win: id = objc::msg_send![ns_view, window];
                    let _: () = objc::msg_send![ns_win, setLevel: 3_i64]; // NSFloatingWindowLevel
                    let _: () = objc::msg_send![ns_win, makeKeyAndOrderFront: ns_win];
                }
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// Hide without triggering macOS window promotion (which would show main).
/// `orderOut:` removes the window; `hide()` would auto-activate the next app window.
pub fn hide_quietly(window: &WebviewWindow) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    #[allow(deprecated)]
    {
        let qc_for_main_thread = window.clone();
        window
            .run_on_main_thread(move || {
                use cocoa::base::id;
                use raw_window_handle::HasWindowHandle;

                if let Ok(raw_handle) = qc_for_main_thread.window_handle() {
                    if let raw_window_handle::RawWindowHandle::AppKit(appkit) = raw_handle.as_raw()
                    {
                        let ns_view = appkit.ns_view.as_ptr() as id;
                        unsafe {
                            let ns_win: id = objc::msg_send![ns_view, window];
                            let _: () = objc::msg_send![ns_win, orderOut: ns_win];
                        }
                    }
                }
            })
            .map_err(|e| e.to_string())?;
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = window.hide();
    }
    Ok(())
}

pub fn notify_closed(app: &AppHandle) {
    set_placement(app, QuickCapturePlacement::BottomRight);
    let _ = app.emit_to("main", QC_CLOSED_EVENT, ());
}

#[tauri::command]
pub async fn open_quick_capture(
    app: AppHandle,
    placement: QuickCapturePlacement,
) -> Result<(), String> {
    let window = app
        .get_webview_window("quick-capture")
        .ok_or("quick-capture window not found")?;
    set_placement(&app, placement);
    if let Err(e) = apply_placement(&app) {
        set_placement(&app, QuickCapturePlacement::BottomRight);
        return Err(e);
    }
    // This command runs on a tokio thread; AppKit traps when a window's level
    // or ordering changes off the main thread, so hop over before showing.
    let to_show = window.clone();
    window
        .run_on_main_thread(move || {
            show_floating(&to_show);
        })
        .map_err(|e| e.to_string())?;
    app.emit_to("main", QC_OPENED_EVENT, placement)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centered_origin_centers_exactly() {
        assert_eq!(
            centered_origin(100.0, 50.0, 1200.0, 800.0, 400.0, 160.0),
            (500.0, 370.0)
        );
    }

    #[test]
    fn placement_serde_round_trip() {
        assert_eq!(
            serde_json::to_string(&QuickCapturePlacement::BottomRight).unwrap(),
            "\"bottom-right\""
        );
        assert_eq!(
            serde_json::to_string(&QuickCapturePlacement::CenteredOverMain).unwrap(),
            "\"centered-over-main\""
        );
        assert_eq!(
            serde_json::from_str::<QuickCapturePlacement>("\"bottom-right\"").unwrap(),
            QuickCapturePlacement::BottomRight
        );
        assert_eq!(
            serde_json::from_str::<QuickCapturePlacement>("\"centered-over-main\"").unwrap(),
            QuickCapturePlacement::CenteredOverMain
        );
    }

    #[test]
    fn default_placement_is_bottom_right() {
        assert_eq!(
            QuickCapturePlacement::default(),
            QuickCapturePlacement::BottomRight
        );
    }
}
