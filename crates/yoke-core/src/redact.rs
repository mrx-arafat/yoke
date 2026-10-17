//! Secret masking at the decision-log persistence boundary.

use std::ops::Range;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

const MASK: &str = "[REDACTED]";

/// Names whose value is a secret, matched inside longer names such as `DB_PASSWORD`.
const KEYWORDS: &str = "token|secret|password|passwd|passphrase|api[_-]?key|authorization|credential|private[_-]?key|cookie|sess(?:ion)?[_-]?id|signature|account[_-]?key";
/// Short names that are secrets only as a whole word of the name (`DB_PASS`, `x-sig`, `auth`):
/// inside a longer word they are `bypass`, `compass`, `sigil` and `author`. `pwd` counts only
/// next to another word (`PWD_DB`), because a bare `PWD` is the working directory.
const WORD_NAME: &str = r"(?-u:\b)(?:(?:[a-z0-9_.-]*[_.-])?(?:pass|sig|auth)(?:[_.-][a-z0-9_.-]*)?|[a-z0-9_.-]*[_.-]pwd(?:[_.-][a-z0-9_.-]*)?|pwd[_.-][a-z0-9_.-]*)";

static SENSITIVE_KEY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!("(?i){KEYWORDS}|^(?:{WORD_NAME})$")).unwrap());
static PEM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)-----BEGIN [A-Z0-9 ]+-----.*?(?:-----END [A-Z0-9 ]+-----|\z)").unwrap()
});
// A prefix must start a word, so `ask-codex` and `<task-notification>` are not `sk-` keys.
static TOKEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?-u:\b)(?:ghp_|gho_|ghu_|ghr_|ghs_|github_pat_|sk-|sk_live_|rk_live_|pk_live_|xoxb-|xoxp-|xoxa-|xoxs-|xoxr-|xapp-|xoxe(?:\.xox[a-z])?-|glpat-|shp(?:at|ca|pa|ss)_|dop_v1_)[A-Za-z0-9_-]+",
        r"|AKIA[A-Za-z0-9]{16}",
        r"|(?-u:\b)AIza[A-Za-z0-9_-]{30,}",
        r"|(?-u:\b)ya29\.[A-Za-z0-9_-]{20,}",
        r"|(?-u:\b)npm_[A-Za-z0-9]{30,}",
        r"|(?-u:\b)hf_[A-Za-z0-9]{20,}",
        r"|(?-u:\b)SG\.[A-Za-z0-9_-]{16,}\.[A-Za-z0-9_-]{16,}",
        r"|(?-u:\b)eyJ[A-Za-z0-9_-]{5,}\.[A-Za-z0-9_-]{5,}\.[A-Za-z0-9_-]*",
    ))
    .unwrap()
});
// Group 1 of `scheme://user:password@host` is the password. It may hold `@` and `:`, so it runs to
// the last `@` before the path; whitespace, quotes and brackets end the search for it.
static URL_PASSWORD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r##"://[^\s:/?#@"'`<>]*:([^\s/?#"'`<>]+)@"##).unwrap());
// For the four patterns below, group 1 is kept visible and the masked value starts after it.
// Group 2 is the value's first character only; `quoted` and `Runs` find its end, so no regex
// rescans a long value.
static BEARER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)(\bBearer\s+)([^\s"'`,;\}\]])"#).unwrap());
static AUTHORIZATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)(authorization["']?\s*(?:[:=]\s*|\s+))(?:Basic|Bearer)\s+([^\s"'`,;\}\]])"#)
        .unwrap()
});
// `key=value`, `key:value` and `--key value`. Only a flag takes its value after bare whitespace,
// so prose such as `token is required` or `password-reset emails` is left alone.
static ASSIGNMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r#"(?i)(["']?(?:[a-z0-9_.-]*(?:{KEYWORDS})[a-z0-9_.-]*|{WORD_NAME})["']?(?:\s*=\s*|\s*:)|--(?:[a-z0-9_.-]*(?:{KEYWORDS})[a-z0-9_.-]*|{WORD_NAME})\s+)([^\s,;}}\]&])"#
    ))
    .unwrap()
});
// `key: value`, where the value is the rest of the line because prose and YAML have no other
// delimiter. The name must end in the keyword (or its plural), so `Tokenizer:` and
// `sshSecrets.ts:` are not keys, and must not follow a path separator, so `/run/secrets: No such
// file` is not one either.
static LINE_ASSIGNMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r#"(?im)(?:^|[^a-z0-9_./\\-])([a-z0-9_.-]*(?:{KEYWORDS})s?["']?\s*:\s+)([^\s,;}}\]`])"#
    ))
    .unwrap()
});
/// Characters besides whitespace that end an unquoted value of the patterns above. A
/// `LINE_ASSIGNMENT` value also ends at a line break, but not at other whitespace.
const BEARER_STOP: &str = "\"'`,;}]";
const ASSIGNMENT_STOP: &str = ",;}]&";
const LINE_STOP: &str = "\"'`,;}]";

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

/// Status fields of a tool's response that carry no free text, kept in its summary.
const RESPONSE_STATUS: [&str; 10] = [
    "exit_code",
    "exitCode",
    "code",
    "is_error",
    "isError",
    "interrupted",
    "success",
    "truncated",
    "duration_ms",
    "durationMs",
];
/// Characters of a failed tool's `error` text kept in the decision log.
const ERROR_KEPT: usize = 256;
/// Characters of a command's error output kept as an excerpt; enough for a sandbox or shell
/// error message (`bwrap: loopback: Failed RTM_NEWADDR`), too few to hold a file.
const EXCERPT_KEPT: usize = 200;
/// Tools whose error output is worth an excerpt; other tools' output is file or web content.
const COMMAND_TOOLS: [&str; 2] = ["Bash", "PowerShell"];

/// What the decision log keeps of a hook payload: the event with secrets masked and the tool's
/// output reduced to its size and status flags, plus a short excerpt of a command's error output.
/// The output is where file contents and personal data show up.
pub fn hook_payload(input: &str, cap: usize) -> String {
    let Ok(Value::Object(mut event)) = serde_json::from_str::<Value>(input) else {
        return redact_payload(input, cap);
    };
    let command = event
        .get("tool_name")
        .and_then(Value::as_str)
        .is_some_and(|name| COMMAND_TOOLS.contains(&name));
    if let Some(response) = event.get_mut("tool_response") {
        *response = summarize_response(response, command);
    }
    if let Some(Value::String(error)) = event.get_mut("error") {
        if let Some((end, _)) = error.char_indices().nth(ERROR_KEPT) {
            error.truncate(end);
            error.push_str("...");
        }
    }
    redact_payload(&Value::Object(event).to_string(), cap)
}

fn summarize_response(response: &Value, command: bool) -> Value {
    let bytes = match response {
        Value::String(text) => text.len(),
        other => other.to_string().len(),
    };
    let mut summary = serde_json::Map::from_iter([("bytes".to_owned(), Value::from(bytes))]);
    if let Value::Object(fields) = response {
        for name in RESPONSE_STATUS {
            if let Some(value @ (Value::Bool(_) | Value::Number(_))) = fields.get(name) {
                summary.insert(name.to_owned(), value.clone());
            }
        }
    }
    // A command's standard output is data; its error output is a diagnostic.
    let diagnostic = match response {
        Value::String(text) => Some(text.as_str()),
        Value::Object(fields) => ["stderr", "output"]
            .iter()
            .find_map(|name| fields.get(*name).and_then(Value::as_str))
            .filter(|text| !text.is_empty()),
        _ => None,
    };
    if let Some(text) = diagnostic.filter(|_| command) {
        let end = text
            .char_indices()
            .nth(EXCERPT_KEPT)
            .map_or(text.len(), |(end, _)| end);
        summary.insert("excerpt".to_owned(), Value::from(&text[..end]));
    }
    Value::Object(summary)
}

/// Compile every pattern now, so a caller can pay for it off its critical path.
pub fn warm() {
    LazyLock::force(&SENSITIVE_KEY);
    LazyLock::force(&PEM);
    LazyLock::force(&TOKEN);
    LazyLock::force(&URL_PASSWORD);
    LazyLock::force(&BEARER);
    LazyLock::force(&AUTHORIZATION);
    LazyLock::force(&ASSIGNMENT);
    LazyLock::force(&LINE_ASSIGNMENT);
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
    let mut spans: Vec<_> = URL_PASSWORD
        .captures_iter(&text)
        .map(|found| found.get(1).unwrap().range())
        .collect();
    for (pattern, stop, line) in [
        (&*AUTHORIZATION, BEARER_STOP, false),
        (&*BEARER, BEARER_STOP, false),
        (&*ASSIGNMENT, ASSIGNMENT_STOP, false),
        (&*LINE_ASSIGNMENT, LINE_STOP, true),
    ] {
        let mut runs = Runs {
            text: &text,
            stop,
            line,
            last: 0..0,
        };
        // Resume at each value so a key inside it (`--token a/--password b`) still masks its own.
        let mut at = 0;
        while let Some(found) = pattern.captures_at(&text, at) {
            let value = found.get(2).unwrap().start();
            // Only the assignments' values can start with a quote.
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
    /// Whether the value runs to the end of its line instead of ending at whitespace.
    line: bool,
    last: Range<usize>,
}

impl Runs<'_> {
    /// Whether the value ends at `c`, which is followed by `rest`.
    fn ends_at(&self, c: char, rest: &str) -> bool {
        if !self.line {
            return c.is_whitespace() || self.stop.contains(c);
        }
        // An apostrophe inside a word (`it's`) belongs to a prose value; one that ends a word
        // closes a quote opened before the key.
        let apostrophe = c == '\'' && rest.starts_with(char::is_alphanumeric);
        matches!(c, '\n' | '\r') || (self.stop.contains(c) && !apostrophe)
    }

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
            } else if self.ends_at(c, &self.text[end + c.len_utf8()..]) {
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
                "--token sk-abc/--password hunter2",
                "--token [REDACTED] [REDACTED]",
                &["sk-abc", "hunter2"],
            ),
            (
                "TOKEN=ghp_x:passwd: p4ssw0rd",
                "TOKEN=[REDACTED] [REDACTED]",
                &["ghp_x", "p4ssw0rd"],
            ),
            (
                "--token ghp_abc.password: p4ssw0rdXYZ",
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
                "TOKEN=hidden-six password=\"hidden-eight words\" secret: hidden-seven",
                "TOKEN=[REDACTED] password=[REDACTED] secret: [REDACTED]",
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
    fn masks_url_passwords_and_whole_key_value_lines_but_keeps_prose() {
        // (input, exact output, substrings that must not survive)
        let corpus: &[(&str, &str, &[&str])] = &[
            // Credentials in URL userinfo, in text, in a command and inside JSON.
            (
                "DATABASE_URL=postgres://admin:s3cr3tpw@db.internal:5432/app",
                "DATABASE_URL=postgres://admin:[REDACTED]@db.internal:5432/app",
                &["s3cr3tpw"],
            ),
            (
                "git clone https://user:ghp_token@github.com/o/r",
                "git clone https://user:[REDACTED]@github.com/o/r",
                &["ghp_token"],
            ),
            (
                "curl https://user:pass@host/x",
                "curl https://user:[REDACTED]@host/x",
                &[":pass"],
            ),
            (
                "redis://:p@ss:w0rd@cache:6379/0",
                "redis://:[REDACTED]@cache:6379/0",
                &["p@ss", "w0rd"],
            ),
            (
                r#"{"command":"curl https://user:hunter2@host/x","cwd":"/repo"}"#,
                r#"{"command":"curl https://user:[REDACTED]@host/x","cwd":"/repo"}"#,
                &["hunter2"],
            ),
            // A `key: value` value is the rest of the line, up to a quote or JSON closer.
            (
                "password: correct horse battery staple",
                "password: [REDACTED]",
                &["correct", "horse", "battery", "staple"],
            ),
            (
                "note\npassword: correct horse\nuser: bob",
                "note\npassword: [REDACTED]\nuser: bob",
                &["correct", "horse"],
            ),
            (
                r#"curl -d '{"password": 12345, "user": "bob"}'"#,
                r#"curl -d '{"password": [REDACTED], "user": "bob"}'"#,
                &["12345"],
            ),
            (
                r#"curl -d '{"password": "hidden words", "user": "bob"}'"#,
                r#"curl -d '{"password": [REDACTED], "user": "bob"}'"#,
                &["hidden", "words"],
            ),
            (
                r#"curl -H "X-Api-Key: hidden words" https://host/x"#,
                r#"curl -H "X-Api-Key: [REDACTED]" https://host/x"#,
                &["hidden", "words"],
            ),
            // An apostrophe inside the value is part of it; a quote that closes an earlier one is not.
            (
                "password: it's hidden here",
                "password: [REDACTED]",
                &["hidden", "here"],
            ),
            (
                "curl -H 'X-Api-Key: hidden words' https://host/x",
                "curl -H 'X-Api-Key: [REDACTED]' https://host/x",
                &["hidden", "words"],
            ),
            // Flags still take the next word; `Authorization` still masks its credentials.
            ("--token abc", "--token [REDACTED]", &["abc"]),
            (
                "Authorization: Bearer x",
                "Authorization: [REDACTED]",
                &["Bearer x"],
            ),
            // Prose, plain URLs and email-like text are not secrets.
            (
                "the tokenizer module handles keyboard input and password-reset emails",
                "the tokenizer module handles keyboard input and password-reset emails",
                &[],
            ),
            ("token is required", "token is required", &[]),
            (
                "password rotates every 90 days",
                "password rotates every 90 days",
                &[],
            ),
            ("Tokenizer: splits words", "Tokenizer: splits words", &[]),
            // A key-like word at the end of a path or file name is not a key.
            (
                "rg: server/src/sshSecrets.ts: No such file or directory (os error 2)",
                "rg: server/src/sshSecrets.ts: No such file or directory (os error 2)",
                &[],
            ),
            (
                "cat: /run/secrets: Is a directory\nls: cannot access '/etc/password': No such file",
                "cat: /run/secrets: Is a directory\nls: cannot access '/etc/password': No such file",
                &[],
            ),
            (
                "existing password-mode fallback to public key.",
                "existing password-mode fallback to public key.",
                &[],
            ),
            // A secret prefix inside a word is not a key, but one that starts a word still is.
            (
                "skills ask-codex task-runner risk-review disk-cleanup mask-data <task-notification>",
                "skills ask-codex task-runner risk-review disk-cleanup mask-data <task-notification>",
                &[],
            ),
            (
                "ask-codex with sk-abc and (ghp_abc) and \"xoxb-abc\"",
                "ask-codex with [REDACTED] and ([REDACTED]) and \"[REDACTED]\"",
                &["sk-abc", "ghp_abc", "xoxb-abc"],
            ),
            (
                "https://github.com/o/r me@example.com ssh://git@github.com/o/r http://localhost:8080/path@x",
                "https://github.com/o/r me@example.com ssh://git@github.com/o/r http://localhost:8080/path@x",
                &[],
            ),
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
            ("token=a", "token=[REDACTED] tail"),
            ("password:x", "password:[REDACTED] tail"),
            ("api_key=1|", "api_key=[REDACTED] tail"),
            ("secret=s/", "secret=[REDACTED] tail"),
            ("token=[REDACTED]", "token=[REDACTED] tail"),
            // A `key: value` value runs to the end of its line, so the tail is masked too.
            ("password: x ", "password: [REDACTED]"),
        ];
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for (unit, _) in shapes {
                let input = unit.repeat((1 << 20) / unit.len()) + " tail";
                sender.send(redact(&input)).unwrap();
            }
        });
        for (unit, expected) in shapes {
            let output = receiver
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap_or_else(|_| panic!("1 MiB of {unit} took over 10 s"));
            assert_eq!(output, expected);
        }
    }

    /// Joins two halves at run time, so no key-shaped literal sits in the source where secret
    /// scanners would mistake a test fixture for a leaked credential.
    fn j(left: &str, right: &str) -> &'static str {
        Box::leak(format!("{left}{right}").into_boxed_str())
    }

    #[test]
    fn masks_the_credential_shapes_a_security_audit_found_missing() {
        // (input, the secret that must not survive). Generic hex and base64 with no keyword
        // or prefix cannot be recognized by any pattern and are deliberately not here.
        let leaked: &[(&str, &str)] = &[
            ("ghu_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA", "AAAAAAAA"),
            ("ghr_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA", "AAAAAAAA"),
            (j("sk_li", "ve_AAAAAAAAAAAAAAAAAAAAAAAA"), "AAAAAAAA"),
            (j("rk_li", "ve_AAAAAAAAAAAAAAAAAAAAAAAA"), "AAAAAAAA"),
            (j("pk_li", "ve_AAAAAAAAAAAAAAAAAAAAAAAA"), "AAAAAAAA"),
            ("AIzaSyAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA", "AAAAAAAA"),
            ("ya29.A0AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA", "AAAAAAAA"),
            (j("xapp-", "1-A0AAAAAAAAA-111111-aaaaaaaaaaaa"), "aaaaaaaa"),
            (j("xoxa-", "2-111111111111-aaaaaaaaaaaa"), "aaaaaaaa"),
            (j("xoxs-", "111111111111-aaaaaaaaaaaa"), "aaaaaaaa"),
            (j("xoxe.", "xoxp-1-aaaaaaaaaaaaaaaa"), "aaaaaaaa"),
            ("glpat-AAAAAAAAAAAAAAAAAAAA", "AAAAAAAA"),
            ("npm_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA", "AAAAAAAA"),
            ("hf_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA", "AAAAAAAA"),
            (j("shpat", "_0123456789abcdef0123456789abcdef"), "0123456789"),
            (j("dop_v", "1_0123456789abcdef0123456789abcdef"), "0123456789"),
            (
                "SG.AAAAAAAAAAAAAAAAAAAAAA.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
                "AAAAAAAA",
            ),
            (
                "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
                "AAAAAAAA",
            ),
            ("--passphrase correcthorsebattery", "correcthorse"),
            ("PASSPHRASE=correcthorsebattery", "correcthorse"),
            ("DB_PASS=hunter2hunter2", "hunter2"),
            ("--pass hunter2hunter2", "hunter2"),
            ("PWD_DB=hunter2hunter2", "hunter2"),
            ("CREDENTIALS=hunter2hunter2", "hunter2"),
            ("PRIVATE_KEY=hunter2hunter2hunter2", "hunter2"),
            ("Cookie: session=abcdef0123456789abcdef", "abcdef0123"),
            ("Set-Cookie: sid=abcdef0123456789abcdef; HttpOnly", "abcdef0123"),
            ("sessionid=abcdef0123456789abcdef", "abcdef0123"),
            ("PHPSESSID=abcdef0123456789abcdef", "abcdef0123"),
            ("TWILIO_AUTH=0123456789abcdef0123456789abcdef", "0123456789"),
            (
                "DefaultEndpointsProtocol=https;AccountName=a;AccountKey=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA==;EndpointSuffix=core.windows.net",
                "AAAAAAAA",
            ),
            (
                r#"{"auths":{"ghcr.io":{"auth":"dXNlcjpwYXNz"}}}"#,
                "dXNlcjpwYXNz",
            ),
            (r#"{"auth":"dXNlcjpwYXNzd29yZA=="}"#, "dXNlcjpwYXNzd29yZA"),
            (
                "https://x.blob.core.windows.net/c?sig=AAAAAAAAAAAAAAAAAAAAAAAA%3D&sv=2020",
                "AAAAAAAA",
            ),
            (
                "https://b.s3.amazonaws.com/k?X-Amz-Signature=0123456789abcdef0123456789abcdef&X-Amz-Expires=60",
                "0123456789",
            ),
        ];
        for (input, secret) in leaked {
            let output = redact(&format!("echo {input}"));
            assert!(!output.contains(secret), "{secret} survived in {output}");
        }
        // Words that merely contain a short keyword, and identifiers with a token-like prefix,
        // are not secrets; neither is a 40-digit commit hash.
        let kept = [
            "jest --passWithNoTests src/app.test.ts",
            "3 tests passed: 12 skipped: 1",
            "compass: north bypass=1 passthrough=2",
            "cd $PWD && PWD=/home/user/project",
            "export npm_config_registry=https://registry.npmjs.org/ hf_model_name=bert",
            "git show 0123456789abcdef0123456789abcdef01234567",
            "author: Ada, signing off; the sigil is a symbol",
        ];
        for input in kept {
            assert_eq!(redact(input), input, "over-masked: {input}");
        }
    }

    #[test]
    fn stores_a_summary_of_tool_output_never_the_output() {
        let event = serde_json::json!({
            "session_id": "s",
            "hook_event_name": "PostToolUse",
            "tool_name": "Read",
            "tool_input": {"file_path": "/work/people.csv"},
            "tool_response": {
                "content": "Ada Lovelace ada@example.com 4111 1111 1111 1111",
                "exit_code": 0,
                "interrupted": false,
            },
            "error": "e".repeat(5000),
        })
        .to_string();
        let stored = hook_payload(&event, 4096);
        for personal in ["Ada", "example.com", "4111"] {
            assert!(!stored.contains(personal), "{personal} stored in {stored}");
        }
        let parsed: Value = serde_json::from_str(&stored).unwrap();
        assert_eq!(parsed["tool_name"], "Read");
        assert_eq!(parsed["tool_input"]["file_path"], "/work/people.csv");
        assert_eq!(parsed["tool_response"]["exit_code"], 0);
        assert_eq!(parsed["tool_response"]["interrupted"], false);
        assert!(parsed["tool_response"]["bytes"].as_u64().unwrap() > 40);
        assert!(parsed["error"].as_str().unwrap().len() < 600);
        // A command's error output keeps a short excerpt, so a sandbox error stays diagnosable;
        // its standard output and any other tool's output do not.
        let sandboxed = serde_json::json!({
            "tool_name": "Bash",
            "tool_response": format!("bwrap: loopback: Failed RTM_NEWADDR: Operation not permitted\n{}", "x".repeat(900)),
        })
        .to_string();
        let stored: Value = serde_json::from_str(&hook_payload(&sandboxed, 4096)).unwrap();
        let excerpt = stored["tool_response"]["excerpt"].as_str().unwrap();
        assert!(excerpt.starts_with("bwrap: loopback: Failed RTM_NEWADDR"));
        assert!(excerpt.chars().count() <= 200);
        let listing = serde_json::json!({
            "tool_name": "Bash",
            "tool_response": {"stdout": "Ada Lovelace 4111 1111 1111 1111", "stderr": ""},
        })
        .to_string();
        let stored = hook_payload(&listing, 4096);
        assert!(
            !stored.contains("Ada") && !stored.contains("4111"),
            "{stored}"
        );
        // Text that is not a JSON object has no tool output to summarize.
        assert_eq!(
            hook_payload("not json token=abc", 64),
            "not json token=[REDACTED]"
        );
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
