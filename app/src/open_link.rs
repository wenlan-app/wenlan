// SPDX-License-Identifier: AGPL-3.0-only
//! `wenlan://page/{id}` and `wenlan://memory/{id}` links: open that page or
//! memory in the app.
//!
//! AI tools print these next to a reminder or a cited page (the in-chat check,
//! the CLI), so the person can jump to where something was decided. A link
//! only raises the window and shows the item; it changes nothing.

use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime, Url};

/// Frontend event: a link is waiting in [`PendingOpenLink`].
pub const OPEN_LINK_EVENT: &str = "wenlan-open-link";

/// What a link opens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum OpenTarget {
    Page {
        #[serde(rename = "pageId")]
        page_id: String,
    },
    Memory {
        #[serde(rename = "memoryId")]
        memory_id: String,
    },
}

/// The most recent link, held until the main window takes it. A cold start
/// delivers the link before the frontend is listening, so the frontend pulls
/// it with [`take_open_link`] instead of relying on the event.
#[derive(Default)]
pub struct PendingOpenLink(Mutex<Option<OpenTarget>>);

impl PendingOpenLink {
    fn put(&self, target: OpenTarget) {
        *self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(target);
    }

    /// Hands the target out once, so a remount or a repeated event cannot
    /// navigate again.
    fn take(&self) -> Option<OpenTarget> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }
}

/// The target when `url` is exactly `wenlan://page/<id>` or
/// `wenlan://memory/<id>`. Anything else, including a query, is ignored.
pub fn open_target(url: &Url) -> Option<OpenTarget> {
    if url.scheme() != "wenlan"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let id = url.path().strip_prefix('/')?;
    if !valid_id(id) {
        return None;
    }
    match url.host_str()? {
        "page" => Some(OpenTarget::Page {
            page_id: id.to_string(),
        }),
        "memory" => Some(OpenTarget::Memory {
            memory_id: id.to_string(),
        }),
        _ => None,
    }
}

/// Page and memory ids are opaque but plain: 1 to 128 of `[A-Za-z0-9._-]`.
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// Handles URLs the OS opened the app with. The first page or memory link in
/// `urls` is kept (replacing one the window has not taken yet) and the window
/// is raised. Returns the URLs that are not page or memory links, for the
/// other link handlers.
pub fn accept_urls<R: Runtime>(app: &AppHandle<R>, urls: &[Url]) -> Vec<Url> {
    let mut target = None;
    let mut rest = Vec::new();
    for url in urls {
        match open_target(url) {
            Some(found) => {
                target.get_or_insert(found);
            }
            None if is_open_link_shape(url) => {
                log::warn!("[open-link] ignored a malformed page or memory link");
            }
            None => rest.push(url.clone()),
        }
    }
    let Some(target) = target else {
        return rest;
    };
    if let Some(pending) = app.try_state::<PendingOpenLink>() {
        pending.put(target);
    }
    crate::reveal_main_window_on_main_thread(app);
    if let Err(error) = app.emit_to("main", OPEN_LINK_EVENT, ()) {
        log::warn!("[open-link] could not notify the main window: {error}");
    }
    rest
}

/// A `wenlan://page…` or `wenlan://memory…` URL, valid or not: never handed to
/// the pairing handler.
fn is_open_link_shape(url: &Url) -> bool {
    url.scheme() == "wenlan" && matches!(url.host_str(), Some("page" | "memory"))
}

#[tauri::command]
pub fn take_open_link(pending: tauri::State<'_, PendingOpenLink>) -> Option<OpenTarget> {
    pending.take()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(raw: &str) -> Option<OpenTarget> {
        open_target(&Url::parse(raw).unwrap())
    }

    #[test]
    fn opens_page_and_memory_links() {
        assert_eq!(
            parse("wenlan://page/page_abc-1"),
            Some(OpenTarget::Page {
                page_id: "page_abc-1".into()
            })
        );
        assert_eq!(
            parse("wenlan://memory/mem_x.2"),
            Some(OpenTarget::Memory {
                memory_id: "mem_x.2".into()
            })
        );
    }

    #[test]
    fn serializes_for_the_frontend() {
        assert_eq!(
            serde_json::to_value(OpenTarget::Page {
                page_id: "p1".into()
            })
            .unwrap(),
            serde_json::json!({"kind": "page", "pageId": "p1"})
        );
        assert_eq!(
            serde_json::to_value(OpenTarget::Memory {
                memory_id: "m1".into()
            })
            .unwrap(),
            serde_json::json!({"kind": "memory", "memoryId": "m1"})
        );
    }

    #[test]
    fn rejects_every_other_shape() {
        for raw in [
            "https://page/page_abc",
            "wenlan://pages/page_abc",
            "wenlan://page",
            "wenlan://page/",
            "wenlan://page/a/b",
            "wenlan://page/page_abc?edit=1",
            "wenlan://page/page_abc#x",
            "wenlan://user@page/page_abc",
            "wenlan://page:9/page_abc",
            "wenlan://page/a%20b",
            "wenlan://memory/a:b",
        ] {
            assert_eq!(parse(raw), None, "{raw}");
        }
        let long = format!("wenlan://page/{}", "a".repeat(129));
        assert_eq!(parse(&long), None);
    }

    #[test]
    fn a_link_is_handed_out_once() {
        let pending = PendingOpenLink::default();
        assert_eq!(pending.take(), None);
        pending.put(OpenTarget::Page {
            page_id: "a".into(),
        });
        pending.put(OpenTarget::Memory {
            memory_id: "b".into(),
        });
        assert_eq!(
            pending.take(),
            Some(OpenTarget::Memory {
                memory_id: "b".into()
            })
        );
        assert_eq!(pending.take(), None);
    }

    #[test]
    fn pairing_links_are_left_for_the_pairing_handler() {
        let pair = Url::parse("wenlan://pair?code=x").unwrap();
        assert_eq!(open_target(&pair), None);
        assert!(!is_open_link_shape(&pair));
    }
}
