// SPDX-License-Identifier: AGPL-3.0-only
//! Route macOS Edit-menu history actions to the focused CodeMirror, with a
//! same-turn WebKit input fallback and Cocoa handling for native controls.
use objc::{class, msg_send, sel, sel_impl};
use tauri::{
    menu::{Menu, MenuItem, MenuItemKind},
    AppHandle, Manager, Runtime,
};

const UNDO: &str = "wenlan-edit-undo";
const REDO: &str = "wenlan-edit-redo";

pub fn install<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let menu = Menu::default(app)?;
    let edit = menu
        .items()?
        .into_iter()
        .find_map(|item| match item {
            MenuItemKind::Submenu(submenu) if submenu.text().ok().as_deref() == Some("Edit") => {
                Some(submenu)
            }
            _ => None,
        })
        .ok_or_else(|| tauri::Error::Anyhow(anyhow::anyhow!("default Edit menu missing")))?;
    let undo = MenuItem::with_id(app, UNDO, "Undo", true, Some("CmdOrCtrl+Z"))?;
    let redo = MenuItem::with_id(app, REDO, "Redo", true, Some("CmdOrCtrl+Shift+Z"))?;
    edit.remove_at(1)?;
    edit.remove_at(0)?;
    edit.insert(&undo, 0)?;
    edit.insert(&redo, 1)?;
    app.set_menu(menu)?;
    app.on_menu_event(|app, event| {
        let direction = match event.id().as_ref() {
            UNDO => "undo",
            REDO => "redo",
            _ => return,
        };
        let app = app.clone();
        let dispatch_app = app.clone();
        let _ = app.run_on_main_thread(move || dispatch(&dispatch_app, direction));
    });
    Ok(())
}

fn dispatch<R: Runtime>(app: &AppHandle<R>, direction: &'static str) {
    // SAFETY: this function and the fallback run on the Cocoa main thread.
    let (key_window, responder): (usize, usize) = unsafe {
        let ns_app: *mut objc::runtime::Object =
            msg_send![class!(NSApplication), sharedApplication];
        let window: *mut objc::runtime::Object = msg_send![ns_app, keyWindow];
        if window.is_null() {
            return;
        }
        let responder: *mut objc::runtime::Object = msg_send![window, firstResponder];
        (window as usize, responder as usize)
    };
    let Some(window) = app.webview_windows().into_values().find(|window| {
        window.is_focused().unwrap_or(false)
            && window.ns_window().ok().map(|p| p as usize) == Some(key_window)
    }) else {
        // A native window/control outside our webviews retains Cocoa history.
        unsafe {
            let ns_app: *mut objc::runtime::Object =
                msg_send![class!(NSApplication), sharedApplication];
            let action = if direction == "undo" {
                sel!(undo:)
            } else {
                sel!(redo:)
            };
            let _: bool = msg_send![ns_app, sendAction:action to:std::ptr::null::<objc::runtime::Object>() from:std::ptr::null::<objc::runtime::Object>()];
        }
        return;
    };
    let target = window.clone();
    let _ = window.with_webview(move |webview| {
        // The focused responder must still be the one observed by the menu.
        // WebKit's content view is a descendant of its WKWebView.
        unsafe {
            let ns_app: *mut objc::runtime::Object = msg_send![class!(NSApplication), sharedApplication];
            let current: *mut objc::runtime::Object = msg_send![ns_app, keyWindow];
            if current.is_null() || current as usize != key_window { return; }
            let current_responder: *mut objc::runtime::Object = msg_send![current, firstResponder];
            if current_responder.is_null() || current_responder as usize != responder { return; }
            let is_view: bool = msg_send![current_responder, isKindOfClass:class!(NSView)];
            let webkit = webview.inner() as *mut objc::runtime::Object;
            let belongs: bool = if is_view && !webkit.is_null() {
                msg_send![current_responder, isDescendantOf:webkit]
            } else { false };
            if !belongs {
                let action = if direction == "undo" { sel!(undo:) } else { sel!(redo:) };
                let _: bool = msg_send![ns_app, sendAction:action to:std::ptr::null::<objc::runtime::Object>() from:std::ptr::null::<objc::runtime::Object>()];
                return;
            }
        }
        // The fallback executes within the same JS turn, on the same focused
        // DOM element; there is no delayed Cocoa action after this evaluation.
        let script = format!("(()=>{{if(!document.hasFocus())return;if(window.dispatchEvent(new CustomEvent('wenlan-native-history',{{cancelable:true,detail:'{direction}'}}))){{const e=document.activeElement;if(e&&!e.closest('.cm-content')&&(e.isContentEditable||e instanceof HTMLInputElement||e instanceof HTMLTextAreaElement)) document.execCommand('{direction}');}}}})()");
        let _ = target.eval(script);
    });
}
