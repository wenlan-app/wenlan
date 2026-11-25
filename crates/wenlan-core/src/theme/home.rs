// SPDX-License-Identifier: Apache-2.0
//! A theme's home: the HTML view the home screen shows while that theme is in
//! use (UX-LOOK-10). It is a file in the theme's own folder, `home.html`, next
//! to `theme.json` and its images, so it switches with the theme and travels
//! with the folder. The built-in themes keep theirs in the themes folder
//! itself.
//!
//! Taking it off renames the file to `home.html.off`, so Undo puts the same
//! file back. The last error the app saw while showing it sits beside it in
//! `home-error.txt`, for the AI tool that fixes it.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use wenlan_types::page_look::{PageLook, LOOK_API};

use super::{read_active_id, valid_theme_id};

pub const HOME_FILE: &str = "home.html";
const HOME_OFF_FILE: &str = "home.html.off";
const HOME_ERROR_FILE: &str = "home-error.txt";

/// The folder whose `home.html` the home screen shows: the active custom
/// theme's own folder, or the themes folder for the built-in themes.
pub fn home_dir(root: &Path) -> PathBuf {
    match read_active_id(root) {
        Some(id) if valid_theme_id(&id) && root.join(&id).is_dir() => root.join(id),
        _ => root.to_path_buf(),
    }
}

/// A stable, positive number for one version of the HTML, so the app can
/// tell a new view from the one it already showed. 48 bits, so it stays
/// exact as a JavaScript number.
fn look_id(html: &str) -> i64 {
    let digest = Sha256::digest(html.as_bytes());
    let mut bytes = [0u8; 8];
    bytes[2..].copy_from_slice(&digest[..6]);
    i64::from_be_bytes(bytes).max(1)
}

fn modified_secs(path: &Path) -> i64 {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs() as i64)
}

/// The active theme's home view, if it has one.
pub fn read_home(root: &Path) -> Option<PageLook> {
    let dir = home_dir(root);
    let path = dir.join(HOME_FILE);
    let html = std::fs::read_to_string(&path).ok()?;
    if html.trim().is_empty() {
        return None;
    }
    let last_error = std::fs::read_to_string(dir.join(HOME_ERROR_FILE))
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty());
    Some(PageLook {
        id: look_id(&html),
        kept_at: modified_secs(&path),
        api: LOOK_API,
        html,
        session: None,
        last_error,
    })
}

fn write_atomic(dir: &Path, name: &str, body: &[u8]) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".{name}.tmp"));
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, dir.join(name))
}

/// What [`write_home`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HomeSave {
    /// The same HTML was already the home view: nothing written.
    Unchanged { id: i64 },
    /// Saved. `replaced` when it took the place of a different view.
    Saved { id: i64, replaced: bool },
    /// The theme in use already has a different home view, and the caller did
    /// not ask to replace it. `theme` is its name (`None` for the built-in
    /// themes). A new look for home belongs in a new theme, so another
    /// theme's view is never lost by accident.
    Taken { theme: Option<String> },
}

/// Save the active theme's home view. The caller has already checked the
/// HTML with `page_look::check_look`. A different view already there is
/// replaced only when `replace` is set.
pub fn write_home(root: &Path, html: &str, replace: bool) -> std::io::Result<HomeSave> {
    let dir = home_dir(root);
    let id = look_id(html);
    let replaced = match read_home(root) {
        Some(existing) if existing.html == html => return Ok(HomeSave::Unchanged { id }),
        Some(_) if !replace => {
            let theme = (dir != root)
                .then(|| {
                    super::read_theme_file(&dir)
                        .ok()
                        .map(|(theme, _)| theme.name)
                })
                .flatten();
            return Ok(HomeSave::Taken { theme });
        }
        Some(_) => true,
        None => false,
    };
    write_atomic(&dir, HOME_FILE, html.as_bytes())?;
    let _ = std::fs::remove_file(dir.join(HOME_OFF_FILE));
    let _ = std::fs::remove_file(dir.join(HOME_ERROR_FILE));
    Ok(HomeSave::Saved { id, replaced })
}

/// Take the active theme's home view off; the file stays for Undo.
pub fn remove_home(root: &Path) -> std::io::Result<Option<i64>> {
    let dir = home_dir(root);
    let Some(look) = read_home(root) else {
        return Ok(None);
    };
    std::fs::rename(dir.join(HOME_FILE), dir.join(HOME_OFF_FILE))?;
    Ok(Some(look.id))
}

/// Put back the home view [`remove_home`] took off, if it is still there.
pub fn restore_home(root: &Path) -> std::io::Result<Option<i64>> {
    let dir = home_dir(root);
    let off = dir.join(HOME_OFF_FILE);
    if !off.is_file() || dir.join(HOME_FILE).exists() {
        return Ok(None);
    }
    std::fs::rename(&off, dir.join(HOME_FILE))?;
    Ok(read_home(root).map(|look| look.id))
}

/// Keep the last error the app saw while showing the home view.
pub fn record_home_error(root: &Path, message: &str) -> bool {
    let dir = home_dir(root);
    dir.join(HOME_FILE).is_file() && write_atomic(&dir, HOME_ERROR_FILE, message.as_bytes()).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{create_theme, set_active_id};
    use wenlan_types::theme::ThemeBase;

    #[test]
    fn ux_look_10_home_belongs_to_the_active_theme() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        // Built-in themes keep their home in the themes folder.
        assert!(read_home(root).is_none());
        let HomeSave::Saved {
            id: first,
            replaced,
        } = write_home(root, "<p>built-in home</p>", false).unwrap()
        else {
            panic!("a first home view is saved");
        };
        assert!(!replaced);
        assert_eq!(read_home(root).unwrap().id, first);
        assert!(root.join(HOME_FILE).is_file());

        // A custom theme starts without one, and its own lives in its folder.
        let detail = create_theme(root, "Night", ThemeBase::Dark).unwrap();
        set_active_id(root, Some(&detail.id)).unwrap();
        assert!(read_home(root).is_none());
        let HomeSave::Saved { id: night, .. } =
            write_home(root, "<p>night home</p>", false).unwrap()
        else {
            panic!("the custom theme's own home is saved");
        };
        assert_ne!(night, first);
        assert!(night < (1_i64 << 53), "exact in JavaScript");
        assert!(root.join(&detail.id).join(HOME_FILE).is_file());
        assert!(record_home_error(root, "boom"));
        assert_eq!(read_home(root).unwrap().last_error.as_deref(), Some("boom"));

        // Switching back to the built-in themes brings their home back.
        set_active_id(root, None).unwrap();
        assert_eq!(read_home(root).unwrap().html, "<p>built-in home</p>");

        // Taking it off keeps the file for Undo; a new save clears the old one.
        assert_eq!(remove_home(root).unwrap(), Some(first));
        assert!(read_home(root).is_none());
        assert_eq!(restore_home(root).unwrap(), Some(first));
        assert_eq!(read_home(root).unwrap().id, first);
        remove_home(root).unwrap();
        write_home(root, "<p>again</p>", false).unwrap();
        assert_eq!(restore_home(root).unwrap(), None);
        assert_eq!(read_home(root).unwrap().html, "<p>again</p>");
    }

    #[test]
    fn a_new_home_never_replaces_another_themes_view_by_accident() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let ship = create_theme(root, "Ship Console", ThemeBase::Dark).unwrap();
        set_active_id(root, Some(&ship.id)).unwrap();
        write_home(root, "<p>bridge</p>", false).unwrap();

        // The same view again changes nothing, and says so.
        assert!(matches!(
            write_home(root, "<p>bridge</p>", false).unwrap(),
            HomeSave::Unchanged { .. }
        ));
        // A different view is refused, naming the theme, and the old one stays.
        assert_eq!(
            write_home(root, "<p>magazine</p>", false).unwrap(),
            HomeSave::Taken {
                theme: Some("Ship Console".into())
            }
        );
        assert_eq!(read_home(root).unwrap().html, "<p>bridge</p>");
        // Asked for, it is replaced.
        assert!(matches!(
            write_home(root, "<p>bridge v2</p>", true).unwrap(),
            HomeSave::Saved { replaced: true, .. }
        ));
        assert_eq!(read_home(root).unwrap().html, "<p>bridge v2</p>");
    }
}
