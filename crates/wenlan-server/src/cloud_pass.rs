// SPDX-License-Identifier: AGPL-3.0-only
//! Personal cloud: the pass the entrance signs for each request it forwards.
//!
//! A person's Wenlan never signs anyone in. The entrance does, and puts a
//! pass on every request it forwards: the account it is for, when it
//! expires and whether a browser or one of the person's computers sent it,
//! signed with HMAC-SHA256. A pass lives a minute.
//!
//! Each person's Wenlan gets only its own key, derived from the entrance's
//! key and the account id, so one person's Wenlan cannot make a pass that
//! another person's would take.

use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::cloud_locator::AccountId;

/// The request header the pass travels in.
pub const PASS_HEADER: &str = "x-wenlan-pass";
/// How long a pass is good for.
pub const PASS_TTL: Duration = Duration::from_secs(60);
/// The file in a person's folder that holds their Wenlan's key.
pub const ACCOUNT_KEY_FILE: &str = "cloud-pass.key";
/// The environment variable that tells a person's Wenlan whose it is.
pub const ACCOUNT_ENV: &str = "WENLAN_CLOUD_ACCOUNT";

const VERSION: &str = "v1";

type HmacSha256 = Hmac<Sha256>;

/// A 32-byte signing key.
#[derive(Clone)]
pub struct PassKey([u8; 32]);

impl std::fmt::Debug for PassKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PassKey(<hidden>)")
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

impl PassKey {
    /// A new random key.
    pub fn random() -> std::io::Result<Self> {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).map_err(|error| std::io::Error::other(error.to_string()))?;
        Ok(Self(bytes))
    }

    /// Reads the key kept at `path` (64 hex digits).
    pub fn read(path: &Path) -> std::io::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        let bytes = hex::decode(text.trim())
            .ok()
            .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "not a pass key")
            })?;
        Ok(Self(bytes))
    }

    /// Writes the key to `path`, readable by this user only. An existing
    /// file is replaced.
    pub fn write(&self, path: &Path) -> std::io::Result<()> {
        let dir = path.parent().unwrap_or(Path::new("."));
        let mut file = tempfile::NamedTempFile::new_in(dir)?;
        // A temp file is made readable by its owner only (0600 on Unix).
        std::io::Write::write_all(&mut file, hex::encode(self.0).as_bytes())?;
        file.as_file().sync_all()?;
        file.persist(path).map_err(|error| error.error)?;
        Ok(())
    }

    /// The key kept at `path`, made the first time.
    pub fn read_or_create(path: &Path) -> std::io::Result<Self> {
        match Self::read(path) {
            Ok(key) => Ok(key),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let key = Self::random()?;
                key.write(path)?;
                Ok(key)
            }
            Err(error) => Err(error),
        }
    }

    /// The key one person's Wenlan checks passes with.
    pub fn for_account(&self, account: &AccountId) -> Self {
        Self(self.mac(&format!("account:{account}")))
    }

    fn mac(&self, message: &str) -> [u8; 32] {
        let mut mac = HmacSha256::new_from_slice(&self.0).expect("any key length");
        mac.update(message.as_bytes());
        mac.finalize().into_bytes().into()
    }

    /// A pass for `account`, good until `expires` (seconds since 1970).
    /// The entrance signs with the account's key ([`PassKey::for_account`]).
    pub fn sign_until(&self, account: &AccountId, expires: u64) -> String {
        self.sign_via_until(account, Via::Browser, expires)
    }

    /// [`PassKey::sign_until`] for a request that came `via` this way.
    pub fn sign_via_until(&self, account: &AccountId, via: Via, expires: u64) -> String {
        let body = format!("{VERSION}.{account}.{expires}.{}", via.as_str());
        let mac = hex::encode(self.mac(&body));
        format!("{body}.{mac}")
    }

    /// A browser's pass for `account` good for [`PASS_TTL`] from now.
    pub fn sign(&self, account: &AccountId) -> String {
        self.sign_via(account, Via::Browser)
    }

    /// A pass for `account` good for [`PASS_TTL`] from now.
    pub fn sign_via(&self, account: &AccountId, via: Via) -> String {
        self.sign_via_until(account, via, now_unix() + PASS_TTL.as_secs())
    }

    /// The account a pass is for and how the request came, when this key
    /// signed it and it has not expired at `now`. A pass that claims to
    /// last longer than [`PASS_TTL`] is refused too.
    pub fn verify_at(&self, pass: &str, now: u64) -> Option<(AccountId, Via)> {
        let mut parts = pass.split('.');
        let (version, account, expires, via, mac) = (
            parts.next()?,
            parts.next()?,
            parts.next()?,
            parts.next()?,
            parts.next()?,
        );
        if parts.next().is_some() || version != VERSION {
            return None;
        }
        let account = AccountId::parse(account)?;
        let via = Via::parse(via)?;
        let expires: u64 = expires.parse().ok()?;
        if expires < now || expires > now + PASS_TTL.as_secs() {
            return None;
        }
        let mac = hex::decode(mac).ok()?;
        let mut check = HmacSha256::new_from_slice(&self.0).expect("any key length");
        check.update(format!("{VERSION}.{account}.{expires}.{}", via.as_str()).as_bytes());
        check.verify_slice(&mac).ok()?;
        Some((account, via))
    }

    /// [`PassKey::verify_at`] now.
    pub fn verify(&self, pass: &str) -> Option<(AccountId, Via)> {
        self.verify_at(pass, now_unix())
    }
}

/// How a forwarded request reached the entrance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Via {
    /// A signed-in browser (its session cookie): a change must come from
    /// the personal cloud's own page.
    Browser,
    /// One of the person's computers, with its own key.
    Computer,
}

impl Via {
    fn as_str(self) -> &'static str {
        match self {
            Self::Browser => "browser",
            Self::Computer => "computer",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "browser" => Some(Self::Browser),
            "computer" => Some(Self::Computer),
            _ => None,
        }
    }
}

#[cfg(test)]
#[path = "cloud_pass_test.rs"]
mod tests;
