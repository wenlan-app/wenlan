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
        r#"|(?i:[?&](?:token|api[_-]?key|secret|password)=(?:[a-z0-9]|%[a-f0-9]{2})[^\s"'#&\x60]*)"#,
    ))
    .expect("explicit credential pattern")
});

static BASIC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bbasic[ \t]+([a-z0-9+/]+={0,2})").expect("Basic credential pattern")
});

static BARE_USERINFO: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b[A-Za-z0-9_.%+-]+:([A-Za-z0-9%_.~+!-]+)@([A-Za-z0-9][A-Za-z0-9.-]*)")
        .expect("bare userinfo pattern")
});

static PACKAGE_VERSION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^v?[0-9]{1,4}(?:\.[0-9]+)*(?:[-+][A-Za-z0-9.+-]+)?$")
        .expect("package version pattern")
});

static ASSIGNMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r#"(?i)\b(?:[a-z0-9_]*[_-])?(?:api[_-]?key|secret|token|password|access[_-]?key)\b["'\x60]?"#,
        r#"[ \t]*[:=][ \t]*(?:"((?:\\[^\r\n]|[^"\\\r\n])*)"|'((?:\\[^\r\n]|[^'\\\r\n])*)'|\x60((?:\\[^\r\n]|[^\x60\\\r\n])*)\x60"#,
        r#"|(<[^>\r\n]*>|\$\{[^}\r\n]*\}|\{env:[^}\r\n]*\}|[^\s"'\x60,;()\[\]}]+))"#,
    ))
    .expect("credential assignment pattern")
});

static ENV_REFERENCE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"^(?:\$(?:env:)?[A-Za-z_][A-Za-z0-9_]*",
        r"|\$\{(?:env:)?[A-Za-z_][A-Za-z0-9_]*\}|\{env:[A-Za-z_][A-Za-z0-9_]*\})$",
    ))
    .expect("environment reference pattern")
});

static PRIVATE_KEY_MARKER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"-----(BEGIN|END) ((?:[A-Z0-9]+ )*PRIVATE KEY)-----")
        .expect("private key boundary pattern")
});

fn assignment_literal(value: &str, quoted: bool) -> bool {
    let value = value.trim();
    if value.chars().count() < 8
        || ENV_REFERENCE.is_match(value)
        || value.starts_with('<') && value.ends_with('>')
        || value.get(..5).is_some_and(|prefix| {
            prefix.eq_ignore_ascii_case("your_")
                && value[5..]
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.'))
        })
        || value
            .bytes()
            .all(|byte| matches!(byte, b'x' | b'X' | b'*' | b'.'))
    {
        return false;
    }
    if value.bytes().all(|byte| byte.is_ascii_alphabetic()) {
        // Bare identifiers and quoted plain words occur in rule examples.
        return quoted && value.bytes().any(|byte| byte.is_ascii_uppercase());
    }
    quoted || !property_reference(value)
}

fn property_reference(value: &str) -> bool {
    if value.contains('.') || value.contains("::") {
        return value
            .split('.')
            .flat_map(|part| part.split("::"))
            .all(|part| {
                let mut bytes = part.bytes();
                bytes
                    .next()
                    .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
                    && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            });
    }
    value.contains('_')
        && value.starts_with(|ch: char| ch.is_ascii_alphabetic() || ch == '_')
        && value.split('_').all(|part| {
            part.trim_end_matches(|ch: char| ch.is_ascii_digit())
                .bytes()
                .all(|byte| byte.is_ascii_alphabetic())
        })
}

fn has_literal(line: &str) -> bool {
    EXPLICIT_CREDENTIAL.is_match(line)
        || BARE_USERINFO.captures_iter(line).any(|capture| {
            let host = capture[2].trim_end_matches('.');
            host.eq_ignore_ascii_case("localhost")
                || host.contains('.')
                || !PACKAGE_VERSION.is_match(&capture[1])
        })
        || BASIC.captures_iter(line).any(|capture| {
            STANDARD_NO_PAD
                .decode(capture[1].trim_end_matches('='))
                .is_ok_and(|bytes| bytes.contains(&b':'))
        })
        || ASSIGNMENT.captures_iter(line).any(|capture| {
            let quoted = capture.iter().skip(1).take(3).flatten().next();
            let value = quoted.or_else(|| capture.get(4)).expect("assignment value");
            let tail = line[capture.get(0).expect("assignment").end()..].trim_start();
            (quoted.is_some() || !tail.starts_with(['(', '[', '.']))
                && assignment_literal(value.as_str(), quoted.is_some())
        })
}

pub(crate) fn scanned_lines(text: &str) -> impl Iterator<Item = (usize, &str, bool)> {
    let lines = text.split_inclusive('\n').enumerate();
    let mut markers = lines
        .clone()
        .filter_map(|(index, line)| {
            let capture = PRIVATE_KEY_MARKER.captures(line)?;
            let kind = capture.get(1)?.as_str();
            let label = capture.get(2)?.as_str();
            Some((index, kind, label))
        })
        .peekable();
    lines.scan(false, move |in_key, (index, line)| {
        let marker = markers.next_if(|&(at, _, _)| at == index);
        let opening = marker.is_some_and(|(_, kind, _)| kind == "BEGIN");
        let has_key = *in_key || opening || has_literal(line);
        if let Some((_, _, label)) = marker {
            *in_key = opening
                && markers
                    .peek()
                    .is_some_and(|&(_, kind, closing)| kind == "END" && label == closing);
        }
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
            "Use example:rule-credential-fixture@internal-host\r\n",
            "Use https://fixture.invalid/?token=rule-query-fixture\r\n",
            "api-key: w79wxza928af410e9c4f36xdpycb01h6\r\n",
            "SERVICE_TOKEN=Z8sm1sFz7dJ5dKwS9E4nyP2g_87\r\n",
            "api-key: \x60w79wxza928af410e9c4f36xdpycb01h6\x60\r\n",
            "-----BEGIN OPENSSH PRIVATE KEY-----\r\n",
            "fixture-only-payload\r\n",
            "-----END OPENSSH PRIVATE KEY-----\r\n",
            "ordinary rule again",
        );
        assert_eq!(credential_line_numbers(text), (2..=13).collect::<Vec<_>>());
        assert_eq!(
            reject_secret_lines(text).unwrap_err().to_string(),
            "possible credential on line 2"
        );
    }

    #[test]
    fn rulesync_credential_scan_refuses_short_and_low_entropy_assignments() {
        let fixtures = [
            "DB_PASSWORD=Summer2024!",
            "password: correct-horse-battery-staple",
            "token: 3f9a2b7c1d8e4f60",
            "PASSWORD=synthetic-rule-credential",
            "- DB_PASSWORD = \"Summer2024!\".",
            "DB_PASSWORD=Summer2024.",
            r#"PASSWORD="Pass\"word42!""#,
            "TOKEN=${TOKEN:-fallback-password}",
        ];
        assert_eq!(
            credential_line_numbers(&fixtures.join("\n")),
            (1..=fixtures.len()).collect::<Vec<_>>()
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
            "token = env::var(\"TOKEN\")\n",
            "api_key = process.env.OPENAI2KEY\n",
            "password = $DATABASE_PASSWORD\n",
            "password = '${DATABASE_PASSWORD}'\n",
            "access_key: <insert key here>\n",
            "token = \"YOUR_TOKEN_HERE\"\n",
            "password = 'xxxxxxxx'\n",
            "token = '********'\n",
            "api_key: '........'\n",
            "Authorization: Bearer ${AUTH_TOKEN}\n",
            "https://fixture.invalid/?token=$TOKEN\n",
        );
        assert!(credential_line_numbers(text).is_empty());
    }
}
