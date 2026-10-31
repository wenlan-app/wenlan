// SPDX-License-Identifier: AGPL-3.0-only
//! `wenlan://page/<page id>` links, which AI tools print beside a page's mind
//! map when they can't show it (docs/ux/spec.md, UX-CHAT-4).
//!
//! A link only raises the window and opens that page. It never changes
//! anything, so a link someone else sends can do no more than a click in the
//! sidebar.

use std::sync::Mutex;

use tauri::{AppHandle, Emitter, Manager, Runtime, Url};

/// Frontend event: a page link is waiting in [`PendingPageLink`].
pub const PAGE_LINK_EVENT: &str = "page-link";

/// The most recent page link, held until the main window takes it. A cold
/// start delivers the link before the frontend is listening, so the frontend
/// pulls it with [`take_page_link`] instead of relying on the event.
#[derive(Default)]
pub struct PendingPageLink(Mutex<Option<String>>);

impl PendingPageLink {
    fn put(&self, page_id: String) {
        *self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(page_id);
    }

    /// Hands the page out once, so a remount or a repeated event cannot reopen it.
    fn take(&self) -> Option<String> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }
}

/// Returns the page id when `url` is exactly `wenlan://page/<id>`, with an id
/// of the same opaque shape the MCP tools accept. Anything else is ignored.
pub fn page_id(url: &Url) -> Option<String> {
    if url.scheme() != "wenlan"
        || url.host_str() != Some("page")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let id = url.path().strip_prefix('/')?;
    let opaque = !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'));
    opaque.then(|| id.to_string())
}

/// Takes the first page link in `urls` and returns the rest for the other
/// link handlers.
pub fn accept_urls<R: Runtime>(app: &AppHandle<R>, urls: Vec<Url>) -> Vec<Url> {
    let (pages, rest): (Vec<Url>, Vec<Url>) = urls
        .into_iter()
        .partition(|url| url.scheme() == "wenlan" && url.host_str() == Some("page"));
    let Some(page_id) = pages.iter().find_map(page_id) else {
        if !pages.is_empty() {
            log::warn!("[page-link] ignored {} malformed page link(s)", pages.len());
        }
        return rest;
    };
    if let Some(pending) = app.try_state::<PendingPageLink>() {
        pending.put(page_id);
    }
    crate::reveal_main_window_on_main_thread(app);
    if let Err(error) = app.emit_to("main", PAGE_LINK_EVENT, ()) {
        log::warn!("[page-link] could not notify the main window: {error}");
    }
    rest
}

#[tauri::command]
pub fn take_page_link(pending: tauri::State<'_, PendingPageLink>) -> Option<String> {
    pending.take()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(raw: &str) -> Option<String> {
        page_id(&Url::parse(raw).unwrap())
    }

    #[test]
    fn ux_chat_4_opens_the_page_a_tool_links_to() {
        assert_eq!(
            parse("wenlan://page/page_abc-123").as_deref(),
            Some("page_abc-123")
        );
        let pending = PendingPageLink::default();
        pending.put("page_a".to_string());
        pending.put("page_b".to_string());
        assert_eq!(pending.take().as_deref(), Some("page_b"));
        assert_eq!(pending.take(), None);
    }

    #[test]
    fn rejects_every_other_shape() {
        for raw in [
            "https://page/page_a",
            "wenlan://pages/page_a",
            "wenlan://page/",
            "wenlan://page",
            "wenlan://page/page_a/edit",
            "wenlan://page/page_a?delete=1",
            "wenlan://page/page_a#x",
            "wenlan://user@page/page_a",
            "wenlan://page:9/page_a",
            "wenlan://page/..%2Fx",
        ] {
            assert_eq!(parse(raw), None, "{raw}");
        }
    }
}
