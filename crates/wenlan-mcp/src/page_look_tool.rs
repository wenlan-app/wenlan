// SPDX-License-Identifier: Apache-2.0
//! The `read_page_look` and `set_page_look` tools: an AI tool draws a page's
//! look (UX-LOOK-1..6). `read_page_look` hands the AI the real data the look
//! will get, the look the page has now, the last error the app saw, a few
//! rules and a short example; `set_page_look` saves the HTML. The rules live
//! here, not in the always-loaded tool descriptions. Local stdio only.

use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::ErrorData as McpError;
use serde::Deserialize;
use wenlan_types::page_look::{
    LookData, PageLookResponse, SetPageLookRequest, SetPageLookResponse,
};

use super::TransportMode;
use super::{is_opaque_page_id, tool_error, url_encode_simple, WenlanError, WenlanMcpServer};

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ReadPageLookParams {
    #[schemars(description = "The page's title as the person says it, or its page_id.")]
    pub page: String,
    #[schemars(
        description = "This conversation as <tool>:<id>, e.g. claude-code:<session id>. Defaults to WENLAN_SESSION."
    )]
    #[serde(default)]
    pub session: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SetPageLookParams {
    #[schemars(description = "The page's title as the person says it, or its page_id.")]
    pub page: String,
    #[schemars(description = "The look: one self-contained HTML document. Give this or path.")]
    #[serde(default)]
    pub html: Option<String>,
    #[schemars(
        description = "Absolute path of the .html file you wrote on this computer. Give this or html."
    )]
    #[serde(default)]
    pub path: Option<String>,
    #[schemars(
        description = "This conversation as <tool>:<id>, e.g. claude-code:<session id>. Defaults to WENLAN_SESSION."
    )]
    #[serde(default)]
    pub session: Option<String>,
}

/// How much of the page's own text the reply carries; the look reads all of
/// it from `wenlan.page` at run time.
const TEXT_CHARS: usize = 1500;
/// How many of the Space's pages the reply lists.
const SPACE_PAGES_SHOWN: usize = 12;

const RULES: &str = "\
Rules for a look (window.wenlan, api 1):
1. A look is one self-contained HTML document with inline <style> and <script>. It runs in a sandbox with no network.
2. Nothing may load from the internet: no http(s) URLs, no remote fonts, images, scripts or CSS. Draw pictures as inline SVG or data: URLs.
3. Read everything from window.wenlan when the page loads and fill the elements from script. Never copy the page's text into the HTML: a look that does is refused, and it would go stale when the page changes.
4. wenlan.page has title, text (Markdown), html (rendered), sections[{heading, level, text, html, items[{text, done}], tables[{columns, rows}]}] and links[{title, id}]. Use items and tables instead of parsing text. Arrays are never null; done is true, false or null.
5. wenlan.space.pages lists the pages of the same Space, newest first, at most 200: {id, title, created, modified, summary}. wenlan.space.more says there are more. Use it for home pages and lists.
6. wenlan.open(pageId) opens a Wenlan page. It is the only way out. An <a data-wenlan-page=\"id\"> inside page.html already works. Web links do not open.
7. The look is read only. No localStorage, cookies, fetch, forms, popups or editing: the person edits the text in Wenlan's Text view.
8. Colors arrive as CSS variables: --look-bg, --look-surface, --look-text, --look-muted, --look-border, --look-accent, --look-radius, --look-font, --look-mono. Use them so the look reads in light and dark. wenlan.theme.mode is \"light\" or \"dark\".
9. Make it work from 320px wide, handle a page with no sections, tables or items, and keep the look to what the page says.
10. Saving replaces the look and keeps the one before it, which the person can put back. Only change a look when asked.";

const EXAMPLE: &str = r#"Example: a dashboard that counts done items and shows each table as cards.

<!doctype html>
<html><head><meta charset="utf-8"><style>
  body { margin: 0; padding: 24px; background: var(--look-bg); color: var(--look-text); font-family: var(--look-font); }
  h1 { margin: 0 0 4px; }
  .bar { height: 8px; border-radius: 4px; background: var(--look-border); overflow: hidden; }
  .bar > i { display: block; height: 100%; background: var(--look-accent); }
  .cards { display: grid; grid-template-columns: repeat(auto-fill, minmax(160px, 1fr)); gap: 12px; margin-top: 16px; }
  .card { background: var(--look-surface); border: 1px solid var(--look-border); border-radius: var(--look-radius); padding: 12px; }
  .muted { color: var(--look-muted); }
</style></head><body>
<h1 id="title"></h1><p class="muted" id="sub"></p>
<div class="bar"><i id="fill"></i></div>
<div class="cards" id="cards"></div>
<script>
  const { page } = wenlan;
  document.getElementById('title').textContent = page.title;
  const items = page.sections.flatMap(s => s.items).filter(i => i.done !== null);
  const done = items.filter(i => i.done).length;
  document.getElementById('sub').textContent = done + ' of ' + items.length + ' done';
  document.getElementById('fill').style.width = (items.length ? 100 * done / items.length : 0) + '%';
  for (const table of page.sections.flatMap(s => s.tables)) {
    for (const row of table.rows) {
      const card = document.createElement('div');
      card.className = 'card';
      card.textContent = row.join(' · ');
      document.getElementById('cards').append(card);
    }
  }
</script></body></html>"#;

/// The page's data, cut down for a reply: the AI reads the shape and a taste
/// of the text, and the look reads all of it from `window.wenlan`.
fn trimmed(data: &LookData) -> LookData {
    let mut out = data.clone();
    let cut = |text: &mut String, chars: usize| {
        if text.chars().count() > chars {
            let kept: String = text.chars().take(chars).collect();
            *text = format!("{kept}… (cut here; the look gets it all)");
        }
    };
    cut(&mut out.page.text, TEXT_CHARS);
    out.page.html = "(the same text rendered; the look gets it in wenlan.page.html)".to_string();
    for section in &mut out.page.sections {
        cut(&mut section.text, 300);
        section.html = String::new();
    }
    if out.space.pages.len() > SPACE_PAGES_SHOWN {
        out.space.pages.truncate(SPACE_PAGES_SHOWN);
        out.space.more = true;
    }
    out
}

#[derive(Deserialize)]
struct PageHits {
    #[serde(default)]
    pages: Vec<PageHit>,
}

#[derive(Deserialize)]
struct PageHit {
    id: String,
    title: String,
}

impl WenlanMcpServer {
    /// The page's id: `page` as given when it is one, else the page whose
    /// title it is. An error text when none or several match.
    async fn look_page_id(
        &self,
        client: &super::WenlanClient,
        page: &str,
    ) -> Result<String, String> {
        let page = page.trim();
        if page.is_empty() {
            return Err("Give the page's title or page_id.".into());
        }
        if is_opaque_page_id(page)
            && client
                .get::<PageLookResponse>(&format!("/api/pages/{page}/look"))
                .await
                .is_ok()
        {
            return Ok(page.to_string());
        }
        let request = wenlan_types::requests::SearchPagesRequest {
            query: page.to_string(),
            limit: Some(20),
            page_type: None,
            space: None,
        };
        let hits: PageHits = client
            .post("/api/pages/search", &request)
            .await
            .map_err(|error| match error {
                WenlanError::Unreachable(_) => {
                    "Wenlan daemon is not reachable, so no page could be looked up.".to_string()
                }
                _ => "The page lookup failed.".to_string(),
            })?;
        let wanted = page.to_lowercase();
        let exact: Vec<&PageHit> = hits
            .pages
            .iter()
            .filter(|hit| hit.title.trim().to_lowercase() == wanted)
            .collect();
        match exact.as_slice() {
            [hit] => Ok(hit.id.clone()),
            [] => Err(format!(
                "No page is titled \"{page}\". Use the exact title, or pass its page_id from list_pages."
            )),
            several => Err(format!(
                "{} pages are titled \"{page}\" ({}). Pass the page_id of the one you mean.",
                several.len(),
                several
                    .iter()
                    .map(|hit| hit.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }

    pub async fn read_page_look_impl(
        &self,
        params: ReadPageLookParams,
    ) -> Result<CallToolResult, McpError> {
        if self.transport == TransportMode::Http {
            return Ok(CallToolResult::error(vec![ContentBlock::text(
                "This tool is not available over remote connections. Use local stdio MCP on the machine running Wenlan.",
            )]));
        }
        let client = self.session_client(params.session.as_deref()).await;
        let page_id = match self.look_page_id(&client, &params.page).await {
            Ok(id) => id,
            Err(why) => return Ok(CallToolResult::error(vec![ContentBlock::text(why)])),
        };
        let path = format!("/api/pages/{}", url_encode_simple(&page_id));
        let data: LookData = match client.get(&format!("{path}/look/data")).await {
            Ok(data) => data,
            Err(error) => return Ok(tool_error(error, "read page look")),
        };
        let current: PageLookResponse = match client.get(&format!("{path}/look")).await {
            Ok(current) => current,
            Err(error) => return Ok(tool_error(error, "read page look")),
        };
        let data_json = serde_json::to_string_pretty(&trimmed(&data))
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let look = match &current.look {
            Some(look) => {
                let mut text = format!(
                    "This page has a look (version {}, written for api {}):\n{}",
                    look.id, look.api, look.html
                );
                if let Some(error) = &look.last_error {
                    text.push_str(&format!(
                        "\n\nThe last error Wenlan saw while showing it (fix this):\n{error}"
                    ));
                }
                text
            }
            None => "This page has no look yet.".to_string(),
        };
        let text = format!(
            "Page \"{title}\" (page_id {id}).\n\n{look}\n\nThe data this page gives a look, as window.wenlan (text trimmed here):\n{data_json}\n\n{RULES}\n\n{EXAMPLE}\n\nWhen it is ready, call set_page_look with this page and the HTML. A refusal names the rule it broke; fix that and save again.",
            title = data.page.title,
            id = page_id,
        );
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
    }

    pub async fn set_page_look_impl(
        &self,
        params: SetPageLookParams,
    ) -> Result<CallToolResult, McpError> {
        let refuse = |why: &str| {
            Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "Look not saved: {why} Nothing was changed."
            ))]))
        };
        if self.transport == TransportMode::Http {
            return Ok(CallToolResult::error(vec![ContentBlock::text(
                "This tool is not available over remote connections. Use local stdio MCP on the machine running Wenlan.",
            )]));
        }
        let path = params
            .path
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty());
        let html = match (path, params.html) {
            (Some(path), _) => {
                if !std::path::Path::new(path).is_absolute() {
                    return refuse("path must be an absolute path on this computer.");
                }
                match tokio::fs::read_to_string(path).await {
                    Ok(text) => text,
                    Err(error) => return refuse(&format!("could not read {path}: {error}.")),
                }
            }
            (None, Some(html)) => html,
            (None, None) => return refuse("give the look's html or the path of the file."),
        };
        let client = self.session_client(params.session.as_deref()).await;
        let page_id = match self.look_page_id(&client, &params.page).await {
            Ok(id) => id,
            Err(why) => return refuse(&why),
        };
        let path = format!("/api/pages/{}/look", url_encode_simple(&page_id));
        let saved: SetPageLookResponse = match client.put(&path, &SetPageLookRequest { html }).await
        {
            Ok(saved) => saved,
            Err(WenlanError::Api {
                reason: Some(reason),
                ..
            }) if reason.starts_with("look_refused") => {
                let why = reason
                    .strip_prefix("look_refused:")
                    .map(str::trim)
                    .unwrap_or("the daemon refused it.");
                return refuse(why);
            }
            Err(error) => return Ok(tool_error(error, "set page look")),
        };
        let text = if saved.created {
            "Saved. Wenlan now shows this page with the look (Look | Text switch in the page header), filled with the page's current text. The earlier look, if any, is kept and the person can put it back. Tell the person to open the page in Wenlan."
        } else {
            "That is already the page's look. Nothing changed."
        };
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
    }
}

#[cfg(test)]
#[path = "page_look_tool_tests.rs"]
mod tests;
