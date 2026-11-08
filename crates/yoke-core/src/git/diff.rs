//! Parse one file's `git diff` text into hunks the console renders as unified or split views.

/// Larger diffs, and untracked files, are replaced by a note instead of being parsed.
pub(super) const MAX_BYTES: usize = 2 * 1024 * 1024;
const MAX_LINES: usize = 10_000;
const MAX_LINE_CHARS: usize = 4000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Add,
    Remove,
    /// git's `\ No newline at end of file` marker.
    NoNewline,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub kind: LineKind,
    pub old: Option<u32>,
    pub new: Option<u32>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub header: String,
    pub lines: Vec<Line>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    pub path: String,
    /// Previous path when git detected a rename.
    pub old_path: Option<String>,
    pub binary: bool,
    /// Header facts without hunks: new or deleted file, mode change, rename similarity.
    pub notes: Vec<String>,
    pub hunks: Vec<Hunk>,
    pub additions: u32,
    pub deletions: u32,
    /// The diff was cut at the line limit.
    pub truncated: bool,
    /// Diff text in a format the console does not parse (a merge conflict's combined diff).
    pub raw: Option<String>,
    /// Why there is nothing to show.
    pub message: Option<String>,
}

impl FileDiff {
    fn empty(path: &str) -> Self {
        Self {
            path: path.to_owned(),
            old_path: None,
            binary: false,
            notes: Vec::new(),
            hunks: Vec::new(),
            additions: 0,
            deletions: 0,
            truncated: false,
            raw: None,
            message: None,
        }
    }

    pub(super) fn note(path: &str, message: &str) -> Self {
        Self {
            message: Some(message.to_owned()),
            ..Self::empty(path)
        }
    }
}

fn start(token: &str, sign: char) -> Option<u32> {
    token.strip_prefix(sign)?.split(',').next()?.parse().ok()
}

fn cut(text: &str) -> String {
    let mut chars = text.chars();
    let mut out: String = chars.by_ref().take(MAX_LINE_CHARS).collect();
    if chars.next().is_some() {
        out.push_str("...");
    }
    out
}

pub(super) fn parse(path: &str, text: &str) -> FileDiff {
    let mut diff = FileDiff::empty(path);
    let (mut old, mut new) = (0_u32, 0_u32);
    let mut count = 0;
    for line in text.lines() {
        if line.starts_with("@@@") {
            return FileDiff {
                raw: Some(cut_raw(text)),
                ..FileDiff::empty(path)
            };
        }
        if let Some(rest) = line.strip_prefix("@@ ") {
            let mut tokens = rest.split(' ');
            let (Some(from), Some(to)) = (
                tokens.next().and_then(|token| start(token, '-')),
                tokens.next().and_then(|token| start(token, '+')),
            ) else {
                continue;
            };
            (old, new) = (from, to);
            diff.hunks.push(Hunk {
                header: cut(line),
                lines: Vec::new(),
            });
            continue;
        }
        let Some(hunk) = diff.hunks.last_mut() else {
            header(&mut diff, line);
            continue;
        };
        if count >= MAX_LINES {
            diff.truncated = true;
            break;
        }
        count += 1;
        let (kind, body) = match line.chars().next() {
            Some('+') => (LineKind::Add, &line[1..]),
            Some('-') => (LineKind::Remove, &line[1..]),
            Some('\\') => (LineKind::NoNewline, line),
            Some(' ') => (LineKind::Context, &line[1..]),
            _ => (LineKind::Context, line),
        };
        let numbers = match kind {
            LineKind::Add => {
                new += 1;
                diff.additions += 1;
                (None, Some(new - 1))
            }
            LineKind::Remove => {
                old += 1;
                diff.deletions += 1;
                (Some(old - 1), None)
            }
            LineKind::Context => {
                old += 1;
                new += 1;
                (Some(old - 1), Some(new - 1))
            }
            LineKind::NoNewline => (None, None),
        };
        hunk.lines.push(Line {
            kind,
            old: numbers.0,
            new: numbers.1,
            text: cut(body),
        });
    }
    diff
}

fn cut_raw(text: &str) -> String {
    text.chars().take(MAX_BYTES / 4).collect()
}

fn header(diff: &mut FileDiff, line: &str) {
    if line.starts_with("Binary files ") || line == "GIT binary patch" {
        diff.binary = true;
    } else if let Some(from) = line.strip_prefix("rename from ") {
        diff.old_path = Some(from.to_owned());
        diff.notes.push(line.to_owned());
    } else if !(line.is_empty()
        || ["diff --git ", "index ", "--- ", "+++ "]
            .iter()
            .any(|prefix| line.starts_with(prefix)))
    {
        diff.notes.push(line.to_owned());
    }
}
