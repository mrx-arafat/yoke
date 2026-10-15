//! Secret masking at the decision-log persistence boundary.

use std::ops::Range;
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
// Group 1 is kept visible and the masked value starts after it. Group 2 is the value's first
// character only; `quoted` and `Runs` find its end, so no regex rescans a long value.
static BEARER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)(\bBearer\s+)([^\s"'`,;\}\]])"#).unwrap());
static AUTHORIZATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)(authorization["']?\s*(?:[:=]\s*|\s+))(?:Basic|Bearer)\s+([^\s"'`,;\}\]])"#)
        .unwrap()
});
static ASSIGNMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?ix)(["']?(?:--)?[a-z0-9_.-]*(?:token|secret|password|passwd|api[_-]?key|authorization)[a-z0-9_.-]*["']?\s*(?:[=:]\s*|\s+))([^\s,;\}\]&])"#).unwrap()
});
/// Characters besides whitespace that end an unquoted value of the patterns above.
const BEARER_STOP: &str = "\"'`,;}]";
const ASSIGNMENT_STOP: &str = ",;}]&";

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

/// Compile every pattern now, so a caller can pay for it off its critical path.
pub fn warm() {
    LazyLock::force(&SENSITIVE_KEY);
    LazyLock::force(&PEM);
    LazyLock::force(&TOKEN);
    LazyLock::force(&BEARER);
    LazyLock::force(&AUTHORIZATION);
    LazyLock::force(&ASSIGNMENT);
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
    let shapes = PEM.find_iter(input).chain(TOKEN.find_iter(input));
    let text = mask(input, shapes.map(|found| found.range()).collect());
    let mut spans = Vec::new();
    for (pattern, stop) in [
        (&*AUTHORIZATION, BEARER_STOP),
        (&*BEARER, BEARER_STOP),
        (&*ASSIGNMENT, ASSIGNMENT_STOP),
    ] {
        let mut runs = Runs {
            text: &text,
            stop,
            last: 0..0,
        };
        // Resume at each value so a key inside it (`--token a/password b`) still masks its own.
        let mut at = 0;
        while let Some(found) = pattern.captures_at(&text, at) {
            let value = found.get(2).unwrap().start();
            // Only ASSIGNMENT values can start with a quote.
            let end = quoted(&text, value).unwrap_or_else(|| runs.end(value));
            at = found.get(1).unwrap().end();
            spans.push(at..end);
        }
    }
    mask(&text, spans)
}

/// End of a `"..."` or `'...'` value at `start`, where a backslash escapes any character. A
/// quote that never closes runs to the end of the text, as a shell reads it, so the value is
/// never shorter than where an earlier mask of an escaping backslash would have closed it.
fn quoted(text: &str, start: usize) -> Option<usize> {
    let quote = text[start..]
        .chars()
        .next()
        .filter(|c| matches!(c, '"' | '\''))?;
    let mut chars = text[start + 1..].char_indices();
    while let Some((offset, c)) = chars.next() {
        if c == quote {
            return Some(start + 2 + offset);
        }
        if c == '\\' {
            chars.next();
        }
    }
    Some(text.len())
}

/// Ends of unquoted values at increasing starts in one text. An earlier mask is one opaque
/// unit, so it is replaced whole with any tail. Glued values share their run's end, which is
/// scanned once; this keeps redaction linear in the input size.
struct Runs<'a> {
    text: &'a str,
    stop: &'static str,
    last: Range<usize>,
}

impl Runs<'_> {
    fn end(&mut self, start: usize) -> usize {
        if self.last.contains(&start) {
            // Inside a mask the last scan stepped over, the value ends at that mask's `]`.
            let inside = (start.saturating_sub(MASK.len() - 1)..start)
                .find(|&at| at >= self.last.start && mask_at(self.text, at));
            return inside.map_or(self.last.end, |at| at + MASK.len() - 1);
        }
        let mut end = start;
        while let Some(c) = self.text[end..].chars().next() {
            if mask_at(self.text, end) {
                end += MASK.len();
            } else if c.is_whitespace() || self.stop.contains(c) {
                break;
            } else {
                end += c.len_utf8();
            }
        }
        self.last = start..end;
        end
    }
}

/// Whether a mask starts at `at`, compared case-insensitively like the patterns.
fn mask_at(text: &str, at: usize) -> bool {
    text.get(at..at + MASK.len())
        .is_some_and(|word| word.eq_ignore_ascii_case(MASK))
}

/// Replace each run of overlapping or touching spans with one mask.
fn mask(text: &str, mut spans: Vec<Range<usize>>) -> String {
    spans.sort_unstable_by_key(|span| span.start);
    let mut output = String::with_capacity(text.len());
    let mut copied = 0;
    let mut spans = spans.into_iter().peekable();
    while let Some(span) = spans.next() {
        let mut end = span.end;
        while let Some(next) = spans.next_if(|next| next.start <= end) {
            end = end.max(next.end);
        }
        output.push_str(&text[copied..span.start]);
        output.push_str(MASK);
        copied = end;
    }
    output.push_str(&text[copied..]);
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_every_corpus_secret_without_reopening_a_mask() {
        // (input, exact output, substrings that must not survive)
        let corpus: &[(&str, &str, &[&str])] = &[
            // A value that swallows the next key must not hide that key's value.
            (
                "--token sk-abc/password hunter2",
                "--token [REDACTED] [REDACTED]",
                &["sk-abc", "hunter2"],
            ),
            (
                "TOKEN=ghp_x:passwd p4ssw0rd",
                "TOKEN=[REDACTED] [REDACTED]",
                &["ghp_x", "p4ssw0rd"],
            ),
            (
                "--token ghp_abc.password p4ssw0rdXYZ",
                "--token [REDACTED] [REDACTED]",
                &["ghp_abc", "p4ssw0rdXYZ"],
            ),
            // Token tails, earlier masks and boundaries a mask creates.
            (
                "--token sk-hidden.two",
                "--token [REDACTED]",
                &["hidden", ".two"],
            ),
            (
                "--token sk-live0123456789",
                "--token [REDACTED]",
                &["live", "]]"],
            ),
            ("--token [REDACTED]", "--token [REDACTED]", &["]]"]),
            (
                "password=\"first hidden-two Bearer xyz\\\" hidden-tail",
                "password=[REDACTED]",
                &["hidden", "xyz"],
            ),
            (
                "AKIA1234567890ABCDEFBearer hidden-glued",
                "[REDACTED]Bearer [REDACTED]",
                &["AKIA", "hidden"],
            ),
            (
                "Authorization: Bearer hidden-five",
                "Authorization: [REDACTED]",
                &["hidden", "]]"],
            ),
            (
                "Authorization: Basic hidden-basic",
                "Authorization: [REDACTED]",
                &["hidden"],
            ),
            (
                "curl --authorization Basic hidden-flag",
                "curl --authorization [REDACTED]",
                &["hidden"],
            ),
            (
                r#"{"nested":{"access_token":"hidden-one","PASSWORD":["hidden-two"]}}"#,
                r#"{"nested":{"PASSWORD":"[REDACTED]","access_token":"[REDACTED]"}}"#,
                &["hidden"],
            ),
            (
                r#"{"ghp_hiddenkey":"safe"}"#,
                r#"{"[REDACTED]":"safe"}"#,
                &["hidden"],
            ),
            (
                "curl --api-key='hidden-three words' --passwd hidden-four",
                "curl --api-key=[REDACTED] --passwd [REDACTED]",
                &["hidden", "words"],
            ),
            (
                "TOKEN=hidden-six secret: hidden-seven password=\"hidden-eight words\"",
                "TOKEN=[REDACTED] secret: [REDACTED] password=[REDACTED]",
                &["hidden", "words"],
            ),
            (
                "ghp_hiddennine gho_hiddenten ghs_hiddeneleven github_pat_hiddentwelve",
                "[REDACTED] [REDACTED] [REDACTED] [REDACTED]",
                &["hidden"],
            ),
            (
                "sk-hiddenthirteen xoxb-hiddenfourteen xoxp-hiddenfifteen AKIA1234567890ABCDEF",
                "[REDACTED] [REDACTED] [REDACTED] [REDACTED]",
                &["hidden", "AKIA"],
            ),
            (
                "-----BEGIN PRIVATE KEY-----\nhidden-sixteen\n-----END PRIVATE KEY-----",
                "[REDACTED]",
                &["hidden"],
            ),
            (
                "-----BEGIN RSA PRIVATE KEY-----\nhidden-seventeen",
                "[REDACTED]",
                &["hidden"],
            ),
            (
                r#"{"command":"echo --token hidden-eighteen","output":"Bearer hidden-nineteen"}"#,
                r#"{"command":"echo --token [REDACTED]","output":"Bearer [REDACTED]"}"#,
                &["hidden"],
            ),
            ("ordinary text বাংলা", "ordinary text বাংলা", &[]),
        ];
        for (input, expected, secrets) in corpus {
            let output = redact(input);
            assert_eq!(output, *expected, "input: {input}");
            for secret in *secrets {
                assert!(!output.contains(secret), "{secret} survived in {output}");
            }
        }
    }

    #[test]
    fn redacts_a_mebibyte_of_glued_key_value_pairs_in_linear_time() {
        // Quadratic rescans took minutes here, long enough for an agent to time out a hook.
        let shapes = [
            ("token=a", "token="),
            ("password:x", "password:"),
            ("api_key=1|", "api_key="),
            ("secret=s/", "secret="),
            ("token=[REDACTED]", "token="),
        ];
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for (unit, _) in shapes {
                let input = unit.repeat((1 << 20) / unit.len()) + " tail";
                sender.send(redact(&input)).unwrap();
            }
        });
        for (unit, key) in shapes {
            let output = receiver
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap_or_else(|_| panic!("1 MiB of {unit} took over 10 s"));
            assert_eq!(output, format!("{key}[REDACTED] tail"));
        }
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
