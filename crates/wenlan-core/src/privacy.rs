// SPDX-License-Identifier: Apache-2.0
use regex::Regex;
use std::sync::LazyLock;

struct PiiPattern {
    regex: Regex,
    replacement: String,
}

fn pii_pattern(pattern: &str, label: &'static str) -> PiiPattern {
    PiiPattern {
        regex: Regex::new(pattern).unwrap(),
        replacement: format!("[REDACTED:{label}]"),
    }
}

static PII_PATTERNS: LazyLock<Vec<PiiPattern>> = LazyLock::new(|| {
    vec![
        pii_pattern(r"\b\d{3}-\d{2}-\d{4}\b", "SSN"),
        pii_pattern(r"(?i)AKIA[0-9A-Z]{16}", "AWS_KEY"),
        pii_pattern(
            r"-----BEGIN (?:RSA |EC |DSA )?PRIVATE KEY-----[\s\S]*?-----END (?:RSA |EC |DSA )?PRIVATE KEY-----",
            "PRIVATE_KEY",
        ),
        pii_pattern(
            r#"(?i)(?:api_key|apikey|api-key|secret_key|secret|token|password|passwd)\s*[=:]\s*["']?[A-Za-z0-9\-_.]{8,}["']?"#,
            "API_KEY",
        ),
        pii_pattern(
            r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b",
            "EMAIL",
        ),
    ]
});

static CREDIT_CARD_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b\d{4}[\s-]?\d{4}[\s-]?\d{4}[\s-]?\d{4}\b").unwrap());
static PHONE_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:\+?1[-.\s]?)?\(?\d{3}\)?[-.\s]?\d{3}[-.\s]?\d{4}\b").unwrap());
static UUID_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}").unwrap()
});

fn redact_numeric_matches_outside_uuids(text: &str, pattern: &Regex, replacement: &str) -> String {
    let bytes = text.as_bytes();
    let uuid_spans: Vec<_> = UUID_PATTERN
        .find_iter(text)
        .filter(|uuid| {
            let token_char =
                |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-';
            (uuid.start() == 0 || !token_char(bytes[uuid.start() - 1]))
                && (uuid.end() == bytes.len() || !token_char(bytes[uuid.end()]))
        })
        .map(|uuid| uuid.start()..uuid.end())
        .collect();
    let mut result = String::with_capacity(text.len());
    let mut last = 0;
    for matched in pattern.find_iter(text) {
        if uuid_spans
            .iter()
            .any(|uuid| uuid.start <= matched.start() && matched.end() <= uuid.end)
        {
            continue;
        }
        result.push_str(&text[last..matched.start()]);
        result.push_str(replacement);
        last = matched.end();
    }
    result.push_str(&text[last..]);
    result
}

/// Redact PII patterns from text, replacing matches with [REDACTED].
pub fn redact_pii(text: &str) -> String {
    let mut result =
        redact_numeric_matches_outside_uuids(text, &CREDIT_CARD_PATTERN, "[REDACTED:CREDIT_CARD]");
    for pattern in PII_PATTERNS.iter() {
        if let std::borrow::Cow::Owned(next) = pattern
            .regex
            .replace_all(&result, pattern.replacement.as_str())
        {
            result = next;
        }
    }
    redact_numeric_matches_outside_uuids(&result, &PHONE_PATTERN, "[REDACTED:PHONE]")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_credit_card_redaction() {
        let input = "My card is 4111-1111-1111-1111 and 5500 0000 0000 0004";
        let result = redact_pii(input);
        assert!(result.contains("[REDACTED:CREDIT_CARD]"));
        assert!(!result.contains("4111"));
        assert!(!result.contains("5500"));
    }

    #[test]
    fn test_ssn_redaction() {
        let input = "SSN: 123-45-6789";
        let result = redact_pii(input);
        assert!(result.contains("[REDACTED:SSN]"));
        assert!(!result.contains("123-45-6789"));
    }

    #[test]
    fn test_aws_key_redaction() {
        let input = "aws_access_key = AKIAIOSFODNN7EXAMPLE";
        let result = redact_pii(input);
        assert!(result.contains("[REDACTED:AWS_KEY]"));
    }

    #[test]
    fn test_api_key_redaction() {
        let input = r#"api_key="sk-abc123def456ghi789""#;
        let result = redact_pii(input);
        assert!(result.contains("[REDACTED:API_KEY]"));
    }

    #[test]
    fn test_private_key_redaction() {
        let input =
            "-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQ...\n-----END RSA PRIVATE KEY-----";
        let result = redact_pii(input);
        assert!(result.contains("[REDACTED:PRIVATE_KEY]"));
    }

    #[test]
    fn test_no_false_positives() {
        let input = "Regular text with a number 12345 and no PII";
        let result = redact_pii(input);
        assert_eq!(result, input);
    }

    #[test]
    fn test_email_redaction() {
        let input = "Contact us at user@example.com or admin.team@company.co.uk for help";
        let result = redact_pii(input);
        assert!(result.contains("[REDACTED:EMAIL]"));
        assert!(!result.contains("user@example.com"));
        assert!(!result.contains("admin.team@company.co.uk"));
    }

    #[test]
    fn test_phone_redaction() {
        let input = "Call me at (555) 123-4567 or +1-800-555-0199";
        let result = redact_pii(input);
        assert!(result.contains("[REDACTED:PHONE]"));
        assert!(!result.contains("555) 123-4567"));
        assert!(!result.contains("800-555-0199"));
    }

    #[test]
    fn test_phone_no_false_positive_on_short_numbers() {
        // 5-6 digit numbers should NOT be redacted as phone numbers
        let input = "Error code 12345 and version 1.2.3";
        let result = redact_pii(input);
        assert!(!result.contains("[REDACTED:PHONE]"));
    }

    #[test]
    fn test_phone_preserves_bounded_uuid_but_redacts_nearby_numbers() {
        let run = "9c1cd858-fc2d-4d9e-857d-ff4125043945";
        let numeric_tail = "9c1cd858-fc2d-4d9e-857d-123456789012";
        let numeric_card_shape = "9c1cd858-fc2d-4d9e-1234-123456789012";
        let input = format!(
            "Run {run}, {numeric_tail}, and {numeric_card_shape}. Card 4111-1111-1111-1111; call (555) 123-4567, +1-800-555-0199, or tel5551234567."
        );
        let result = redact_pii(&input);
        assert!(result.contains(run), "run UUID was changed: {result}");
        assert!(
            result.contains(numeric_tail),
            "numeric UUID was changed: {result}"
        );
        assert!(
            result.contains(numeric_card_shape),
            "card-shaped UUID was changed: {result}"
        );
        assert_eq!(result.matches("[REDACTED:CREDIT_CARD]").count(), 1);
        assert_eq!(result.matches("[REDACTED:PHONE]").count(), 3);
        assert!(!result.contains("5551234567"));
    }

    #[test]
    fn test_phone_does_not_exempt_lookalikes_or_other_pii() {
        let uuid = "9c1cd858-fc2d-4d9e-857d-ff4125043945";
        let input = format!("api_key={uuid} and broken-ff4125043945 and abc4125043945");
        let result = redact_pii(&input);
        assert!(result.contains("[REDACTED:API_KEY]"));
        assert_eq!(result.matches("[REDACTED:PHONE]").count(), 2);
        assert!(!result.contains("broken-ff4125043945"));
    }
}
