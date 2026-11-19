// SPDX-License-Identifier: Apache-2.0
//! The person key: who counts as the person at this computer.
//!
//! The daemon writes 32 random bytes, as 64 lowercase hex digits, to
//! [`FILE_NAME`] in its data root the first time it starts, readable only by
//! this computer's account that runs Wenlan. The Wenlan app and the `wenlan`
//! command read it from the data root they already use and send it in
//! [`crate::requests::PERSON_KEY_HEADER`] next to the person marker; a
//! request that claims to be the person without it is not believed.
//!
//! It cannot stop a program that runs under the same account and reads the
//! file on purpose: the key keeps other programs from claiming to be the
//! person by writing one header, nothing more.
//!
//! Reading only, with `std`: the daemon side that creates the file lives in
//! `wenlan_core::person_key`.

use std::path::{Path, PathBuf};

/// The key file's name in the data root.
pub const FILE_NAME: &str = "person.key";

/// The key file inside `data_root`.
pub fn path_in(data_root: &Path) -> PathBuf {
    data_root.join(FILE_NAME)
}

/// A key as the daemon writes it: exactly 64 lowercase hex digits.
pub fn well_formed(key: &str) -> bool {
    key.len() == 64
        && key
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// The key in `data_root`, or `None` when the file is missing, unreadable or
/// not a key. A caller without it simply does not send it, and works as an
/// unnamed caller.
pub fn read(data_root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path_in(data_root)).ok()?;
    let key = text.trim();
    well_formed(key).then(|| key.to_string())
}

/// `POST /api/person/browser-pass`: a pass that lets one browser page, opened
/// by the person with `wenlan open`, carry the key. It works once, within a
/// minute, and only at the address the daemon serves on this computer.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BrowserPassResponse {
    pub pass: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ux_skill_13_person_key_reads_only_a_well_formed_key() {
        let dir = std::env::temp_dir().join(format!(
            "wenlan-person-key-read-{}-{}",
            std::process::id(),
            line!()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(read(&dir), None, "no file");
        let key = "0123456789abcdef".repeat(4);
        std::fs::write(path_in(&dir), format!("{key}\n")).unwrap();
        assert_eq!(read(&dir).as_deref(), Some(key.as_str()));
        for bad in [
            "",
            "person",
            &key[..63],
            &key.to_uppercase(),
            &format!("{key}0"),
        ] {
            std::fs::write(path_in(&dir), bad).unwrap();
            assert_eq!(read(&dir), None, "{bad:?}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
