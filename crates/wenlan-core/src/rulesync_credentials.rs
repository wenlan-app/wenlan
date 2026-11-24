// SPDX-License-Identifier: Apache-2.0
//! Detect credential literals before rules or skill assets leave their tool.

use std::sync::LazyLock;

use anyhow::Result;
use base64::{engine::general_purpose::STANDARD_NO_PAD, Engine as _};
use regex::Regex;

static EXPLICIT_CREDENTIAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"\b(?:sk-[A-Za-z0-9_-]{20,}|gh[pousr]_[A-Za-z0-9]{20,}",
        r"|github_pat_[A-Za-z0-9_]{20,}|xox[baprs]-[A-Za-z0-9-]{16,}",
        r"|(?:AKIA|ASIA)[A-Z0-9]{16}|AIza[A-Za-z0-9_-]{35}|wlt_[a-f0-9]{64})\b",
        r"|\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{16,}\b",
        r"|(?i:\bbearer[ \t]+[A-Za-z0-9_.~+/-]{20,})",
        r#"|(?i:\b(?:proxy[-_])?authorization["']?[ \t]*[:=][ \t]*["']?(?:basic|bearer)[ \t]+[A-Za-z0-9][A-Za-z0-9_.~+/-]*)"#,
        r#"|(?i:\b[a-z][a-z0-9+.-]*://[^:/@?#\s"'\x60]+:[^/@?#\s"'\x60]+@)"#,
        r#"|\b[a-zA-Z0-9_.%+-]+:[a-zA-Z0-9%_.~+!-]+@[a-zA-Z0-9][a-zA-Z0-9.-]*"#,
        r#"|(?i:[?&](?:token|api[_-]?key|secret|password)=(?:[a-z0-9]|%[a-f0-9]{2})[^\s"'#&\x60]*)"#,
    ))
    .expect("explicit credential pattern")
});

static BASIC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bbasic[ \t]+([a-z0-9+/]+={0,2})").expect("Basic credential pattern")
});

static ASSIGNMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r#"(?i)\b(?:[a-z0-9_]*[_-])?(?:api[_-]?key|secret|token|password|access[_-]?key)\b["']?"#,
        r#"[ \t]*[:=][ \t]*(?:"([a-z0-9_+./~-]+=*)"|'([a-z0-9_+./~-]+=*)'|\x60([a-z0-9_+./~-]+=*)\x60|([a-z0-9_+./~-]+=*))"#,
    ))
    .expect("credential assignment pattern")
});

fn key_shaped(value: &str, quoted: bool) -> bool {
    if value.len() < 20 || !value.is_ascii() {
        return false;
    }
    let mut distinct = [false; 128];
    let mut lower = false;
    let mut upper = false;
    let mut digits = 0;
    for byte in value.bytes() {
        distinct[usize::from(byte)] = true;
        lower |= byte.is_ascii_lowercase();
        upper |= byte.is_ascii_uppercase();
        digits += usize::from(byte.is_ascii_digit());
    }
    // Unquoted property paths and snake-case names can have numeric suffixes
    // such as auth_v2_token. A digit inside a word is more typical of a key.
    if !quoted
        && value.contains(['.', '_'])
        && value.split(['.', '_']).all(|word| {
            word.trim_end_matches(|ch: char| ch.is_ascii_digit())
                .bytes()
                .all(|byte| byte.is_ascii_alphabetic())
        })
    {
        return false;
    }
    (digits >= 4 && (lower || upper) || quoted && lower && upper)
        && distinct.into_iter().filter(|seen| *seen).count() >= 10
}

fn has_literal(line: &str) -> bool {
    EXPLICIT_CREDENTIAL.is_match(line)
        || BASIC.captures_iter(line).any(|capture| {
            STANDARD_NO_PAD
                .decode(capture[1].trim_end_matches('='))
                .is_ok_and(|bytes| bytes.contains(&b':'))
        })
        || ASSIGNMENT.captures_iter(line).any(|capture| {
            let quoted = capture.iter().skip(1).take(3).flatten().next();
            let value = quoted.or_else(|| capture.get(4)).expect("assignment value");
            let tail = line[capture.get(0).expect("assignment").end()..].trim_start();
            !tail.starts_with(['(', '[', '.']) && key_shaped(value.as_str(), quoted.is_some())
        })
}

pub(crate) fn scanned_lines(text: &str) -> impl Iterator<Item = (usize, &str, bool)> {
    text.split_inclusive('\n')
        .enumerate()
        .scan(false, |in_key, (index, line)| {
            let key_marker = line.contains("PRIVATE KEY-----");
            *in_key |= key_marker && line.contains("-----BEGIN ");
            let has_key = *in_key || has_literal(line);
            *in_key &= !(key_marker && line.contains("-----END "));
            Some((index + 1, line, has_key))
        })
}

pub fn credential_line_numbers(text: &str) -> Vec<usize> {
    scanned_lines(text)
        .filter_map(|(number, _, has_key)| has_key.then_some(number))
        .collect()
}

/// Error messages identify only the position of the suspected credential.
pub fn reject_secret_lines(text: &str) -> Result<()> {
    if let Some((number, _, _)) = scanned_lines(text).find(|(_, _, has_key)| *has_key) {
        anyhow::bail!("possible credential on line {number}");
    }
    Ok(())
}

pub fn contains_credential_bytes(bytes: &[u8]) -> bool {
    reject_secret_lines(&String::from_utf8_lossy(bytes)).is_err()
}

pub const CREDENTIAL_FILE_REASON: &str =
    "Holds text that looks like a key, so Wenlan leaves it where it is.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rulesync_credential_scan_marks_literals_and_complete_private_key_blocks() {
        let text = concat!(
            "ordinary rule\r\n",
            "Authorization: Basic Zml4dHVyZTpydWxlLW9ubHk=\r\n",
            "const headers = { Authorization: 'Bearer zn7SpKc2bVT9jpD4RfyK2Hwg4zfcQFtQ29xwBwC2enAZpx3sY5hcpTMcqdX' };\r\n",
            "Use https://example:rule-credential-fixture@fixture.invalid\r\n",
            "Use example:rule-credential-fixture@localhost\r\n",
            "Use https://fixture.invalid/?token=rule-query-fixture\r\n",
            "api-key: w79wxza928af410e9c4f36xdpycb01h6\r\n",
            "SERVICE_TOKEN=Z8sm1sFz7dJ5dKwS9E4nyP2g_87\r\n",
            "api-key: \x60w79wxza928af410e9c4f36xdpycb01h6\x60\r\n",
            "-----BEGIN OPENSSH PRIVATE KEY-----\r\n",
            "fixture-only-payload\r\n",
            "-----END OPENSSH PRIVATE KEY-----\r\n",
            "ordinary rule again",
        );
        assert_eq!(credential_line_numbers(text), (2..=12).collect::<Vec<_>>());
        assert_eq!(
            reject_secret_lines(text).unwrap_err().to_string(),
            "possible credential on line 2"
        );
    }

    #[test]
    fn rulesync_credential_scan_ignores_identifiers_calls_env_lookups_and_plain_words() {
        let text = concat!(
            "token = data.get(\"token\")\n",
            "token = response.json()[\"token\"]\n",
            "api_key = os.getenv(\"KEY\")\n",
            "api_key = os.environ[\"X\"]\n",
            "token = current_token_from_storage_v2026\n",
            "token = library.api_v2.current_token_2026\n",
            "token = getTokenFromDatabase2026(user)\n",
            "token = authorization\n",
            "secret: 'uncharacteristicallylongword'\n",
            "Authorization: Bearer ${AUTH_TOKEN}\n",
            "https://fixture.invalid/?token=$TOKEN\n",
        );
        assert!(credential_line_numbers(text).is_empty());
    }
}
