//! Secret masking at the decision-log persistence boundary.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

const MASK: &str = "[REDACTED]";

static SENSITIVE_KEY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)token|secret|password|passwd|api[_-]?key|authorization").unwrap()
});
static PEM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)-----BEGIN [A-Z0-9 ]+-----.*?(?:-----END [A-Z0-9 ]+-----|\z)").unwrap()
});
static TOKEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:ghp_|gho_|ghs_|github_pat_|sk-|xoxb-|xoxp-)[A-Za-z0-9_-]+|AKIA[A-Za-z0-9]{16}")
        .unwrap()
});
static BEARER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)\bBearer\s+[^\s"'`,;\}\]]+"#).unwrap());
static AUTHORIZATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)(authorization["']?\s*(?:[:=]\s*|\s+))(?:Basic|Bearer)\s+[^\s"'`,;\}\]]+"#)
        .unwrap()
});
static ASSIGNMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?ix)(["']?(?:--)?[a-z0-9_.-]*(?:token|secret|password|passwd|api[_-]?key|authorization)[a-z0-9_.-]*["']?\s*(?:[=:]\s*|\s+))(?:"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|[^\s,;\}\]&]+)"#).unwrap()
});

/// Mask supported secret shapes in JSON and arbitrary text.
pub fn redact(input: &str) -> String {
    match serde_json::from_str::<Value>(input) {
        Ok(mut value @ (Value::Object(_) | Value::Array(_))) => {
            redact_value(&mut value);
            value.to_string()
        }
        _ => redact_text(input),
    }
}

/// Mask secrets, then cap the UTF-8 payload by bytes without splitting a character.
pub fn redact_payload(input: &str, cap: usize) -> String {
    let mut output = redact(input);
    let mut end = cap.min(output.len());
    while !output.is_char_boundary(end) {
        end -= 1;
    }
    output.truncate(end);
    output
}

pub(crate) fn redact_value(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            for (key, mut value) in std::mem::take(fields) {
                if SENSITIVE_KEY.is_match(&key) {
                    value = Value::String(MASK.into());
                } else {
                    redact_value(&mut value);
                }
                fields.insert(redact_text(&key), value);
            }
        }
        Value::Array(values) => values.iter_mut().for_each(redact_value),
        Value::String(text) => *text = redact(text),
        _ => {}
    }
}

fn redact_text(input: &str) -> String {
    let output = PEM.replace_all(input, MASK);
    let output = TOKEN.replace_all(&output, MASK);
    let output = AUTHORIZATION.replace_all(&output, "${1}[REDACTED]");
    let output = BEARER.replace_all(&output, "Bearer [REDACTED]");
    ASSIGNMENT
        .replace_all(&output, "${1}[REDACTED]")
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_sensitive_shapes_in_json_and_free_text() {
        let cases = [
            r#"{"nested":{"access_token":"hidden-one","PASSWORD":["hidden-two"]}}"#,
            r#"{"ghp_hiddenkey":"safe"}"#,
            r#"curl --api-key='hidden-three words' --passwd hidden-four"#,
            "Authorization: Bearer hidden-five",
            "Authorization: Basic hidden-basic",
            "curl --authorization Basic hidden-flag",
            "TOKEN=hidden-six secret: hidden-seven password=\"hidden-eight words\"",
            "ghp_hiddennine gho_hiddenten ghs_hiddeneleven github_pat_hiddentwelve",
            "sk-hiddenthirteen xoxb-hiddenfourteen xoxp-hiddenfifteen AKIA1234567890ABCDEF",
            "-----BEGIN PRIVATE KEY-----\nhidden-sixteen\n-----END PRIVATE KEY-----",
            "-----BEGIN RSA PRIVATE KEY-----\nhidden-seventeen",
            r#"{"command":"echo --token hidden-eighteen","output":"Bearer hidden-nineteen"}"#,
        ];
        for input in cases {
            let output = redact(input);
            assert!(!output.contains("hidden"), "leaked: {output}");
            assert!(!output.contains("AKIA1234567890ABCDEF"));
        }
        assert_eq!(redact("ordinary text বাংলা"), "ordinary text বাংলা");
    }

    #[test]
    fn caps_only_after_redaction_on_unicode_boundaries() {
        for cap in 0..30 {
            let output = redact_payload("🙂বাংলা token=hidden-value", cap);
            assert!(output.len() <= cap);
            assert!(!output.contains("hidden"));
        }
        assert_eq!(redact_payload("🙂abc", 3), "");
        assert_eq!(redact_payload("🙂abc", 5), "🙂a");
    }
}
