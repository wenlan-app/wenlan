// SPDX-License-Identifier: Apache-2.0
//! Page looks: an HTML layout an AI tool drew for one page (UX-LOOK-1..8).
//!
//! A look is stored beside the page, never inside it. The app shows it in a
//! frame that holds the page's live data as `window.wenlan`. [`LookData`] is
//! that object. `api: 1` is the stable promise: within it fields are only
//! added, never renamed or removed, and arrays are never `null`.

use serde::{Deserialize, Serialize};

/// The version of `window.wenlan` a look is written against.
pub const LOOK_API: u32 = 1;

/// Most pages `space.pages` lists.
pub const LOOK_SPACE_PAGES_MAX: usize = 200;

/// The id the home screen's look is kept under, in place of a page id
/// (UX-LOOK-10). No page has it: page ids are opaque generated strings.
pub const HOME_LOOK_ID: &str = "home";

/// Most pages `home.pages` lists.
pub const LOOK_HOME_PAGES_MAX: usize = 200;

/// One saved look for a page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageLook {
    /// Row id; a version of the look is named by it.
    pub id: i64,
    pub html: String,
    /// The `window.wenlan` version this look was written for.
    pub api: u32,
    /// When it was saved (unix seconds).
    pub kept_at: i64,
    /// The AI session that saved it, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// The last error the app saw while showing it, so the AI can fix it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

/// An earlier or current version in the page's look history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageLookVersion {
    pub id: i64,
    pub kept_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    pub active: bool,
    /// Size of the HTML in bytes.
    pub bytes: i64,
}

/// `GET /api/pages/{id}/look`: the page's active look (none when the page has
/// none) and every kept version, newest first.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PageLookResponse {
    pub look: Option<PageLook>,
    #[serde(default)]
    pub versions: Vec<PageLookVersion>,
}

/// `PUT /api/pages/{id}/look`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetPageLookRequest {
    pub html: String,
}

/// `PUT /api/pages/{id}/look` answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetPageLookResponse {
    pub page_id: String,
    /// The version now active.
    pub look_id: i64,
    /// False when the same HTML was already the active look.
    pub created: bool,
}

/// `DELETE /api/pages/{id}/look` answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemovePageLookResponse {
    pub page_id: String,
    /// The version that was active, so the removal can be undone with
    /// `POST /api/pages/{id}/look/restore`. `None` when there was none.
    #[serde(default)]
    pub removed_id: Option<i64>,
}

/// `POST /api/pages/{id}/look/restore`: make an earlier version the look.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestorePageLookRequest {
    /// The version to bring back. Omitted: the newest version that is not
    /// active.
    #[serde(default)]
    pub id: Option<i64>,
}

/// `POST /api/pages/{id}/look/error`: what went wrong while showing the look.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageLookErrorRequest {
    pub message: String,
}

/// `window.wenlan` for one page, without its functions (`onChange`, `open`),
/// which the frame adds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LookData {
    pub api: u32,
    pub page: LookPageData,
    pub space: LookSpaceData,
    pub theme: LookTheme,
    /// Only in the home screen's look (UX-LOOK-10): the person's Spaces and
    /// their pages. A page's look never has it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<LookHomeData>,
}

/// `window.wenlan.home`: what the home screen shows, for a look of it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LookHomeData {
    /// The person's Spaces, the one with the page changed last first.
    pub spaces: Vec<LookHomeSpace>,
    /// Pages across every Space, changed last first, at most
    /// [`LOOK_HOME_PAGES_MAX`], without their text.
    pub pages: Vec<LookHomePage>,
    /// True when there are more pages than `pages` lists.
    pub more: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LookHomeSpace {
    pub name: String,
    /// Empty when the Space has none.
    pub description: String,
    pub starred: bool,
    /// When its newest listed page changed; empty when none is listed.
    pub modified: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LookHomePage {
    pub id: String,
    pub title: String,
    /// The Space's name; empty when the page is in none.
    pub space: String,
    pub created: String,
    pub modified: String,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LookPageData {
    pub id: String,
    pub title: String,
    /// The Space's name; empty when the page is in none.
    pub space: String,
    pub created: String,
    pub modified: String,
    /// The part for people, as Markdown.
    pub text: String,
    /// The same, rendered. Raw HTML is dropped and links are reduced to
    /// their text; a `[[link]]` to a page is `<a data-wenlan-page="id">`. A
    /// picture kept in the pages folder is `<img data-wenlan-image="n">`,
    /// `n` its place in `images`; a picture on the web keeps its alt text.
    pub html: String,
    pub sections: Vec<LookSection>,
    pub links: Vec<LookLink>,
    /// The page's pictures kept in the pages folder, in order, each once.
    /// The app adds each one's `url` (a `data:` URL) when it shows the look.
    #[serde(default)]
    pub images: Vec<LookImage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LookImage {
    /// The file as the page names it: `_attachments/x.png` or an embed's
    /// `x.png`.
    pub src: String,
    pub alt: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LookSection {
    /// Empty for the text before the first heading.
    pub heading: String,
    /// 0 for the text before the first heading, else 1 to 6.
    pub level: u8,
    pub text: String,
    pub html: String,
    pub items: Vec<LookItem>,
    pub tables: Vec<LookTable>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LookItem {
    pub text: String,
    /// `true` or `false` for a checkbox item, `null` for a plain one.
    pub done: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LookTable {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LookLink {
    pub title: String,
    /// `null` when no page has that title.
    pub id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LookSpaceData {
    pub name: String,
    /// This Space's pages, newest first, at most [`LOOK_SPACE_PAGES_MAX`],
    /// without their text.
    pub pages: Vec<LookSpacePage>,
    /// True when the Space has more pages than `pages` lists.
    pub more: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LookSpacePage {
    pub id: String,
    pub title: String,
    pub created: String,
    pub modified: String,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LookTheme {
    /// `light` or `dark`. The daemon answers `dark`; the app fills in the
    /// theme in use.
    pub mode: String,
}

/// `POST /api/pages/{id}/look/error` answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageLookErrorResponse {
    /// False when the page has no active look to attach the error to.
    pub recorded: bool,
}
