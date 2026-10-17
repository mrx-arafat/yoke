//! Best-effort protection from accidental self-unblocking, not a sandbox.
//!
//! Shell commands are split into words the way a POSIX shell would, so quoting
//! cannot hide a human-only `yoke` command or a path inside `~/.yoke`, while
//! the same words used as data (grep patterns, commit messages) stay allowed.

use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use crate::event::{AgentEvent, EventKind};

const HUMAN_ONLY: &str = "this Yoke command is human-only; a human runs it in their own terminal";
const DATABASE: &str = "agent shell access to state.db is protected";
const SHELL_WRITE: &str = "agent shell writes to Yoke state, agent hook settings and the yoke executable are protected; a human makes that change in their own terminal";
const TOOL_WRITE: &str = "agent writes to Yoke state, agent hook settings and the yoke executable are protected; a human makes that change in their own terminal";
const PATCH_WRITE: &str = "agent patches to Yoke state, agent hook settings and the yoke executable are protected; a human makes that change in their own terminal";
const TOKEN: &str =
    "the Yoke bypass token is private to the human; a human runs yoke pause in their own terminal";

/// File under `~/.yoke` that holds the per-install bypass token, 0600.
pub const OFF_TOKEN_FILE: &str = "off-token";
/// Inline code that names one of these may write a hook registration file or the executable.
const CODE_MARKERS: &[&str] = &[
    ".claude/settings",
    ".codex/hooks.json",
    ".codex/config.toml",
    ".local/bin/yoke",
    ".cargo/bin/yoke",
];
/// Commands that only print names or metadata, so naming the token file is harmless.
const LISTERS: &[&str] = &[
    "ls", "stat", "file", "tree", "du", "test", "[", "cd", "pushd",
];
/// Commands that search every file below the directories they are given.
const SEARCHERS: &[&str] = &["grep", "egrep", "fgrep", "rg", "ag", "ack"];
/// Commands that copy a whole directory they are given.
const COPIERS: &[&str] = &["cp", "rsync", "tar", "zip", "ditto", "scp"];
/// Commands that put sources into a destination, and so may overwrite `dest/<source name>`.
const PLACERS: &[&str] = &["cp", "install", "rsync", "mv", "ln"];

/// A human-only `yoke` phrase inside text that an opaque executor may run.
static HUMAN_PHRASE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^|[^a-z0-9_-])yoke[^a-z0-9_-]+(?:run-hook|trust|console|install|uninstall|pause|project[^a-z0-9_-]+move|jobs[^a-z0-9_-]+retry|notifications[^a-z0-9_-]+ack|plan[^a-z0-9_-]+(?:approve|resume|cancel|reassign|accept|resolve|acknowledge))(?:$|[^a-z0-9_-])")
        .expect("constant human-phrase regex")
});

const HUMAN_PLAN: &[&str] = &[
    "approve",
    "resume",
    "cancel",
    "reassign",
    "accept",
    "resolve",
    "acknowledge",
];
const KEYWORDS: &[&str] = &[
    "if", "then", "else", "elif", "fi", "do", "done", "while", "until", "for", "in", "case",
    "esac", "select", "function", "!",
];
/// Shells whose `-c` script is scanned as a nested command line.
const SHELLS: &[&str] = &["sh", "bash", "zsh", "dash", "ksh", "fish", "su"];
/// Programs that run the command in their remaining arguments, with the flags that take a value.
const WRAPPERS: &[(&str, &[&str])] = &[
    ("command", &[]),
    ("exec", &["-a"]),
    ("builtin", &[]),
    ("nohup", &[]),
    ("time", &[]),
    ("env", &["-u", "-C", "-S"]),
    (
        "sudo",
        &["-u", "-g", "-C", "-D", "-h", "-p", "-r", "-t", "-T", "-U"],
    ),
    ("doas", &["-u", "-C"]),
    ("nice", &["-n"]),
    ("ionice", &["-c", "-n", "-p"]),
    ("timeout", &["-s", "-k"]),
    ("setsid", &[]),
    ("caffeinate", &["-t", "-w"]),
    ("stdbuf", &["-i", "-o", "-e"]),
    ("unbuffer", &[]),
    ("chronic", &[]),
    (
        "xargs",
        &["-n", "-P", "-L", "-s", "-I", "-E", "-d", "-a", "-l"],
    ),
    ("watch", &["-n", "-d"]),
    ("parallel", &["-j", "-I", "-S", "-a"]),
];
/// Programs that execute text the guard cannot see as shell words.
const EXECUTORS: &[&str] = &[
    "source",
    ".",
    "python",
    "python2",
    "python3",
    "pypy",
    "pypy3",
    "perl",
    "ruby",
    "node",
    "nodejs",
    "bun",
    "deno",
    "php",
    "lua",
    "luajit",
    "tclsh",
    "wish",
    "osascript",
    "expect",
    "awk",
    "gawk",
    "mawk",
    "nawk",
    "ssh",
    "tmux",
    "screen",
    "script",
    "crontab",
    "at",
    "batch",
    "launchctl",
];
const EXEC_FLAGS: &[&str] = &[
    "-exec",
    "-execdir",
    "-ok",
    "-okdir",
    "-x",
    "-X",
    "--exec",
    "--exec-batch",
];
/// Commands that remove, rename or re-permission any path argument, so an ancestor counts too.
const DESTROYERS: &[&str] = &[
    "rm", "rmdir", "unlink", "shred", "mv", "chmod", "chown", "chgrp", "chflags", "ln",
];
/// Commands that write to their path arguments.
const WRITERS: &[&str] = &[
    "cp", "install", "rsync", "touch", "mkdir", "truncate", "dd", "tee", "sqlite3", "sqlite",
    "curl", "wget", "patch",
];
/// Writers whose only written path is the last argument.
const DESTINATION_ONLY: &[&str] = &["cp", "install", "rsync"];
const MAX_DEPTH: usize = 4;

/// Return the reason to refuse a PreToolUse operation, regardless of mode.
pub fn evaluate(kind: EventKind, event: &AgentEvent, home: &Path, cwd: &Path) -> Option<String> {
    if kind != EventKind::PreToolUse {
        return None;
    }
    let tool = event.tool_name.as_deref().unwrap_or_default();
    let input = &event.tool_input;
    // Resolving every protected path costs a few filesystem calls, so skip tools with no path.
    let relevant = input.get("command").is_some()
        || input.get("cmd").is_some()
        || matches!(
            tool.to_ascii_lowercase().as_str(),
            "write" | "edit" | "multiedit" | "notebookedit" | "apply_patch" | "read" | "grep"
        );
    if !relevant {
        return None;
    }
    evaluate_in(kind, event, home, cwd, &Protected::from_env(home))
}

fn evaluate_in(
    kind: EventKind,
    event: &AgentEvent,
    home: &Path,
    cwd: &Path,
    protected: &Protected,
) -> Option<String> {
    if kind != EventKind::PreToolUse {
        return None;
    }
    let tool = event
        .tool_name
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let input = &event.tool_input;
    let is_patch = tool == "apply_patch";
    if !is_patch {
        let shell_cwd = ["workdir", "cwd"]
            .into_iter()
            .find_map(|field| input.get(field).and_then(Value::as_str))
            .map_or_else(|| cwd.to_path_buf(), |dir| expand(dir, home, cwd));
        for field in ["command", "cmd"] {
            // Raw strings keep patch headers intact; an argv array is already split into words.
            let raw: Vec<&str> = match input.get(field) {
                Some(Value::String(text)) => vec![text],
                Some(Value::Array(args)) => args.iter().filter_map(Value::as_str).collect(),
                _ => continue,
            };
            let tokens = match raw.as_slice() {
                [text] => lex(text),
                args => args.iter().map(|arg| Token::Word((*arg).into())).collect(),
            };
            for text in raw.iter().filter(|text| text.contains("*** Begin Patch")) {
                if let Some(reason) = patch_denial(text, home, &shell_cwd, protected) {
                    return Some(reason);
                }
            }
            if let Some(reason) = scan_shell(&tokens, home, &shell_cwd, protected, 0, false) {
                return Some(reason);
            }
        }
    }
    if matches!(
        tool.as_str(),
        "write" | "edit" | "multiedit" | "notebookedit" | "apply_patch"
    ) {
        if protected_target(input, home, cwd, protected) {
            return Some(TOOL_WRITE.into());
        }
        if is_patch {
            let patch = input
                .as_str()
                .or_else(|| input.get("command").and_then(Value::as_str));
            if let Some(reason) = patch.and_then(|patch| patch_denial(patch, home, cwd, protected))
            {
                return Some(reason);
            }
        }
    }
    if matches!(tool.as_str(), "read" | "grep") && names_token_target(input, home, cwd, protected) {
        return Some(TOKEN.into());
    }
    None
}

fn patch_denial(patch: &str, home: &Path, cwd: &Path, protected: &Protected) -> Option<String> {
    for line in patch.lines() {
        for prefix in [
            "*** Add File: ",
            "*** Update File: ",
            "*** Delete File: ",
            "*** Move to: ",
        ] {
            if let Some(path) = line.strip_prefix(prefix) {
                if protected.hit(&expand(path.trim(), home, cwd)) {
                    return Some(PATCH_WRITE.into());
                }
            }
        }
    }
    None
}

fn protected_target(value: &Value, home: &Path, cwd: &Path, protected: &Protected) -> bool {
    match value {
        Value::Object(fields) => fields.iter().any(|(key, value)| {
            (matches!(key.as_str(), "file_path" | "path" | "notebook_path")
                && value
                    .as_str()
                    .is_some_and(|path| protected.hit(&expand(path, home, cwd))))
                || protected_target(value, home, cwd, protected)
        }),
        Value::Array(values) => values
            .iter()
            .any(|value| protected_target(value, home, cwd, protected)),
        _ => false,
    }
}

/// Whether a Read or Grep path is the bypass token or a directory that holds it.
fn names_token_target(value: &Value, home: &Path, cwd: &Path, protected: &Protected) -> bool {
    match value {
        Value::Object(fields) => fields.iter().any(|(key, value)| {
            (matches!(key.as_str(), "file_path" | "path" | "notebook_path")
                && value.as_str().is_some_and(|path| {
                    !path.is_empty() && protected.token_within(&expand(path, home, cwd))
                }))
                || names_token_target(value, home, cwd, protected)
        }),
        Value::Array(values) => values
            .iter()
            .any(|value| names_token_target(value, home, cwd, protected)),
        _ => false,
    }
}

#[derive(Debug, PartialEq)]
enum Token {
    Word(String),
    Boundary,
}

/// Split a command line into quote-stripped words and command boundaries.
///
/// Boundaries are `;`, `|`, `&`, newlines, parentheses, braces, and the edges
/// of `$(...)` and backtick substitutions, so the word after one starts a command.
fn lex(text: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut word = String::new();
    let mut chars = text.chars().peekable();
    let mut double = false;
    // Open substitutions: whether each began inside double quotes, and its
    // parenthesis depth, where 0 marks a backtick substitution.
    let mut substitutions: Vec<(bool, usize)> = Vec::new();
    while let Some(c) = chars.next() {
        if double {
            match c {
                '"' => double = false,
                '\\' if matches!(chars.peek(), Some('"' | '\\' | '$' | '`')) => {
                    word.push(chars.next().expect("peeked"));
                }
                '$' if chars.peek() == Some(&'(') => {
                    chars.next();
                    boundary(&mut tokens, &mut word);
                    substitutions.push((true, 1));
                    double = false;
                }
                '`' => {
                    boundary(&mut tokens, &mut word);
                    substitutions.push((true, 0));
                    double = false;
                }
                _ => word.push(c),
            }
            continue;
        }
        match c {
            '\'' => {
                for inner in chars.by_ref() {
                    if inner == '\'' {
                        break;
                    }
                    word.push(inner);
                }
            }
            '"' => double = true,
            '\\' => {
                if let Some(next) = chars.next() {
                    if next != '\n' {
                        word.push(next);
                    }
                }
            }
            '$' if chars.peek() == Some(&'\'') => {}
            '$' if chars.peek() == Some(&'(') => {
                chars.next();
                boundary(&mut tokens, &mut word);
                substitutions.push((false, 1));
            }
            '`' => {
                boundary(&mut tokens, &mut word);
                match substitutions.last() {
                    Some((_, 0)) => double = substitutions.pop().expect("checked").0,
                    _ => substitutions.push((false, 0)),
                }
            }
            '(' => {
                boundary(&mut tokens, &mut word);
                if let Some((_, depth)) = substitutions.last_mut() {
                    if *depth > 0 {
                        *depth += 1;
                    }
                }
            }
            ')' => {
                boundary(&mut tokens, &mut word);
                if let Some((_, depth)) = substitutions.last_mut() {
                    if *depth > 0 {
                        *depth -= 1;
                        if *depth == 0 {
                            double = substitutions.pop().expect("checked").0;
                        }
                    }
                }
            }
            '&' if chars.peek() == Some(&'>') => {
                chars.next();
                flush(&mut tokens, &mut word);
                let mut op = String::from(">");
                if chars.peek() == Some(&'>') {
                    chars.next();
                    op.push('>');
                }
                tokens.push(Token::Word(op));
            }
            ';' | '|' | '&' | '\n' => boundary(&mut tokens, &mut word),
            '>' | '<' => {
                flush(&mut tokens, &mut word);
                let mut op = String::from(c);
                while let Some(&next) = chars.peek() {
                    if next == c || next == '|' || (c == '<' && next == '>') {
                        op.push(next);
                        chars.next();
                    } else {
                        break;
                    }
                }
                if chars.peek() == Some(&'&') {
                    // Duplicates a descriptor; there is no file target.
                    chars.next();
                    while chars
                        .peek()
                        .is_some_and(|next| next.is_ascii_digit() || *next == '-')
                    {
                        chars.next();
                    }
                    continue;
                }
                tokens.push(Token::Word(op));
            }
            c if c.is_whitespace() => flush(&mut tokens, &mut word),
            _ => word.push(c),
        }
    }
    flush(&mut tokens, &mut word);
    tokens
}

fn flush(tokens: &mut Vec<Token>, word: &mut String) {
    if word.is_empty() {
        return;
    }
    if word == "{" || word == "}" {
        tokens.push(Token::Boundary);
        word.clear();
    } else {
        tokens.push(Token::Word(std::mem::take(word)));
    }
}

fn boundary(tokens: &mut Vec<Token>, word: &mut String) {
    flush(tokens, word);
    tokens.push(Token::Boundary);
}

/// Scan one command line: human-only `yoke` commands in command position,
/// `state.db` anywhere, and writes into `~/.yoke` by redirection or mutating commands.
fn scan_shell(
    tokens: &[Token],
    home: &Path,
    cwd: &Path,
    protected: &Protected,
    depth: usize,
    inherited: bool,
) -> Option<String> {
    if depth > MAX_DEPTH {
        return None;
    }
    let mut cwd = cwd.to_path_buf();
    // A protected path named earlier may reach a later command through a pipe or `{}`.
    let mut saw_protected = inherited;
    // Text an opaque executor (interpreter, shell reading stdin) could run.
    let mut phrase_text = String::new();
    let mut opaque_executor = false;
    for command in tokens.split(|token| *token == Token::Boundary) {
        let all: Vec<&str> = command
            .iter()
            .filter_map(|token| match token {
                Token::Word(word) => Some(word.as_str()),
                Token::Boundary => None,
            })
            .collect();
        if all.iter().any(|word| word.contains("state.db")) {
            return Some(DATABASE.into());
        }
        saw_protected |= all
            .iter()
            .any(|word| !word.starts_with('-') && touches(word, home, &cwd, protected, false));
        let mut words = Vec::with_capacity(all.len());
        let mut rest = all.iter().copied().peekable();
        while let Some(word) = rest.next() {
            if word.starts_with('>') || word == "<>" {
                if rest
                    .next()
                    .is_some_and(|target| touches(target, home, &cwd, protected, false))
                {
                    return Some(SHELL_WRITE.into());
                }
            } else if word.starts_with('<') {
                rest.next();
            } else {
                words.push(word);
            }
        }
        loop {
            match words.first().copied() {
                Some(first) if KEYWORDS.contains(&first) => {
                    words.remove(0);
                }
                Some(first) if is_assignment(first) => {
                    if HUMAN_PHRASE.is_match(&first.to_ascii_lowercase()) {
                        return Some(HUMAN_ONLY.into());
                    }
                    words.remove(0);
                }
                Some(first) => match WRAPPERS.iter().find(|(name, _)| *name == basename(first)) {
                    Some((_, value_flags)) => pop_wrapper(&mut words, value_flags),
                    None => break,
                },
                None => break,
            }
        }
        let Some(&first) = words.first() else {
            continue;
        };
        let name = basename(first).to_ascii_lowercase();
        let args = &words[1..];
        if !LISTERS.contains(&name.as_str()) && names_token(&all, &name, home, &cwd, protected) {
            return Some(TOKEN.into());
        }
        if first.contains("{}") || first.starts_with('$') {
            opaque_executor = true;
        }
        match name.as_str() {
            "yoke" if human_subcommand(args) => return Some(HUMAN_ONLY.into()),
            "cd" | "pushd" => {
                let target = args.iter().find(|word| !word.starts_with('-'));
                cwd = expand(target.unwrap_or(&"~"), home, &cwd);
            }
            "eval" => {
                let nested = lex(&args.join(" "));
                if let Some(reason) =
                    scan_shell(&nested, home, &cwd, protected, depth + 1, saw_protected)
                {
                    return Some(reason);
                }
                continue;
            }
            _ if SHELLS.contains(&name.as_str()) => match dash_c_script(args) {
                Some(script) => {
                    let nested = lex(script);
                    if let Some(reason) =
                        scan_shell(&nested, home, &cwd, protected, depth + 1, saw_protected)
                    {
                        return Some(reason);
                    }
                    continue;
                }
                None => opaque_executor = true,
            },
            "find" | "fd" => {
                if let Some(index) = args.iter().position(|word| EXEC_FLAGS.contains(word)) {
                    let nested: Vec<Token> = args[index + 1..]
                        .iter()
                        .map(|word| Token::Word((*word).into()))
                        .collect();
                    if let Some(reason) =
                        scan_shell(&nested, home, &cwd, protected, depth + 1, saw_protected)
                    {
                        return Some(reason);
                    }
                }
            }
            "git" if args.iter().any(|word| *word == "config" || *word == "-c") => {
                opaque_executor = true;
            }
            _ if EXECUTORS.contains(&name.as_str()) => {
                opaque_executor = true;
                if args
                    .iter()
                    .any(|word| CODE_MARKERS.iter().any(|marker| word.contains(marker)))
                {
                    return Some(SHELL_WRITE.into());
                }
            }
            _ => {}
        }
        let reach = if DESTROYERS.contains(&name.as_str())
            || (name == "find" && args.contains(&"-delete"))
        {
            Some(true)
        } else if WRITERS.contains(&name.as_str())
            || EXECUTORS.contains(&name.as_str())
            || edits_in_place(&name, args)
        {
            Some(false)
        } else {
            None
        };
        if let Some(ancestors) = reach {
            let mut paths: Vec<&str> = args
                .iter()
                .copied()
                .filter(|word| !word.starts_with('-'))
                .collect();
            let target_directory = args
                .iter()
                .any(|word| *word == "-t" || word.starts_with("--target-directory"));
            if PLACERS.contains(&name.as_str())
                && places_over_protected(&paths, target_directory, home, &cwd, protected)
            {
                return Some(SHELL_WRITE.into());
            }
            if DESTINATION_ONLY.contains(&name.as_str()) && !target_directory {
                paths = paths.last().copied().into_iter().collect();
            }
            if paths.is_empty() && saw_protected {
                return Some(SHELL_WRITE.into());
            }
            for word in paths {
                if word.contains("{}") {
                    if saw_protected {
                        return Some(SHELL_WRITE.into());
                    }
                    continue;
                }
                let value = word.split_once('=').map(|(_, value)| value);
                if [Some(word), value]
                    .into_iter()
                    .flatten()
                    .any(|word| touches(word, home, &cwd, protected, ancestors))
                {
                    return Some(SHELL_WRITE.into());
                }
            }
        }
        phrase_text.push_str(&all.join(" "));
        phrase_text.push('\n');
    }
    if opaque_executor && HUMAN_PHRASE.is_match(&phrase_text.to_ascii_lowercase()) {
        return Some(HUMAN_ONLY.into());
    }
    None
}

fn basename(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(key, _)| {
        !key.is_empty()
            && !key.starts_with(|c: char| c.is_ascii_digit())
            && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

/// Drop a wrapper and its options so the wrapped command is in command position.
fn pop_wrapper(words: &mut Vec<&str>, value_flags: &[&str]) {
    words.remove(0);
    while let Some(first) = words.first().copied() {
        if first == "--" {
            words.remove(0);
            break;
        }
        if first.starts_with('-') {
            words.remove(0);
            if value_flags.contains(&first) && !words.is_empty() {
                words.remove(0);
            }
        } else if first.starts_with(|c: char| c.is_ascii_digit()) {
            // A positional count or duration, as in `timeout 30 cmd`.
            words.remove(0);
        } else {
            break;
        }
    }
}

fn human_subcommand(args: &[&str]) -> bool {
    let dry_run = args.contains(&"--dry-run");
    let check = args.contains(&"--check");
    let mut words = args.iter().copied().filter(|word| !word.starts_with('-'));
    match words.next() {
        Some("trust" | "console" | "pause" | "run-hook") => true,
        Some("install" | "uninstall") => !dry_run,
        Some("update") => !check,
        Some("jobs") => words.next() == Some("retry"),
        Some("notifications") => words.next() == Some("ack"),
        Some("project") => words.next() == Some("move"),
        Some("plan") => words.next().is_some_and(|word| HUMAN_PLAN.contains(&word)),
        _ => false,
    }
}

/// The script a shell runs for `-c`, including combined flags such as `-lc`.
fn dash_c_script<'a>(args: &[&'a str]) -> Option<&'a str> {
    let index = args
        .iter()
        .position(|word| word.starts_with('-') && !word.starts_with("--") && word.contains('c'))?;
    args[index + 1..]
        .iter()
        .copied()
        .find(|word| !word.starts_with('-'))
}

fn edits_in_place(name: &str, args: &[&str]) -> bool {
    matches!(name, "sed" | "perl")
        && args.iter().any(|word| {
            word.starts_with("--in-place")
                || (word.starts_with('-') && !word.starts_with("--") && word.contains('i'))
        })
}

/// Whether a shell word names protected state, a hook registration file or the yoke executable,
/// or, with `ancestors`, a directory containing one of them. A glob counts when it can match one.
fn touches(word: &str, home: &Path, cwd: &Path, protected: &Protected, ancestors: bool) -> bool {
    let (prefix, glob) = split_glob(word);
    let path = expand(prefix, home, cwd);
    if protected.hit(&path) || (ancestors && protected.ancestor_of(&path)) {
        return true;
    }
    glob && protected.glob_hits(&expand(word, home, cwd), cwd, ancestors)
}

/// The literal prefix of a shell word and whether a wildcard follows it.
fn split_glob(word: &str) -> (&str, bool) {
    match word.find(['*', '?', '[']) {
        Some(index) => (&word[..index], true),
        None => (word, false),
    }
}

/// Whether any word is the bypass token's path, or, for commands that read whole directories,
/// the directory holding it.
fn names_token(words: &[&str], name: &str, home: &Path, cwd: &Path, protected: &Protected) -> bool {
    let searches = SEARCHERS.contains(&name);
    let copies = COPIERS.contains(&name)
        || (name == "find" && words.iter().any(|word| EXEC_FLAGS.contains(word)));
    words.iter().any(|word| {
        let (prefix, glob) = split_glob(word);
        let path = expand(prefix, home, cwd);
        if glob {
            return protected.glob_token(&expand(word, home, cwd));
        }
        protected.reads_token(&path)
            || (!word.starts_with('-')
                && ((searches && protected.token_within(&path))
                    || (copies && protected.is_yoke_dir(&path))))
    })
}

/// Whether placing the sources of `cp`, `mv` or `ln` in a directory destination, or through
/// `--target-directory`, would overwrite `<directory>/<source name>` where that is protected.
fn places_over_protected(
    operands: &[&str],
    target_directory: bool,
    home: &Path,
    cwd: &Path,
    protected: &Protected,
) -> bool {
    let Some((last, others)) = operands.split_last() else {
        return false;
    };
    // `-t DIR` puts the directory first among the operands once its flag is dropped.
    let (directories, sources): (Vec<&str>, Vec<&str>) = if target_directory {
        (operands.to_vec(), operands.to_vec())
    } else {
        (vec![*last], others.to_vec())
    };
    directories.iter().any(|directory| {
        let directory_path = expand(directory, home, cwd);
        (directory.ends_with('/') || directory_path.is_dir())
            && sources.iter().any(|source| {
                protected.hit(&directory_path.join(basename(source.trim_end_matches('/'))))
            })
    })
}

/// Expand a leading `~`, `$HOME` or `${HOME}`, then anchor relative paths at `cwd`.
fn expand(word: &str, home: &Path, cwd: &Path) -> PathBuf {
    for prefix in ["~", "$HOME", "${HOME}"] {
        if let Some(rest) = word.strip_prefix(prefix) {
            if rest.is_empty() {
                return home.to_path_buf();
            }
            if let Some(rest) = rest.strip_prefix('/') {
                return home.join(rest);
            }
        }
    }
    cwd.join(word)
}

/// Everything an agent must not change: Yoke's state, the files that register or configure hooks,
/// and the yoke executable. Each path is kept both lexically normalized and symlink-resolved.
struct Protected {
    yoke: Vec<PathBuf>,
    claude_dirs: Vec<PathBuf>,
    codex_dirs: Vec<PathBuf>,
    binaries: Vec<PathBuf>,
    token: Vec<PathBuf>,
    /// Where a symlinked registration file really lives, for example in a dotfiles repository.
    leaves: Vec<PathBuf>,
}

/// A path as written, and as the filesystem resolves it.
fn forms(path: &Path) -> Vec<PathBuf> {
    let mut forms = vec![normalized(path)];
    let resolved = resolved(path);
    if !forms.contains(&resolved) {
        forms.push(resolved);
    }
    forms
}

impl Protected {
    fn new(
        home: &Path,
        claude_config: Option<PathBuf>,
        codex_home: Option<PathBuf>,
        exe: Option<PathBuf>,
    ) -> Self {
        // The home prefix is shared by almost every protected path, so resolve it once.
        let base = resolved(home);
        let under = |relative: &str| -> Vec<PathBuf> {
            let mut forms = vec![normalized(&home.join(relative))];
            let resolved = resolved_from(&base, Path::new(relative));
            if !forms.contains(&resolved) {
                forms.push(resolved);
            }
            forms
        };
        let directories = |default: &str, configured: Option<PathBuf>| -> Vec<PathBuf> {
            under(default)
                .into_iter()
                .chain(configured.iter().flat_map(|dir| forms(dir)))
                .collect()
        };
        let claude_dirs = directories(".claude", claude_config);
        let codex_dirs = directories(".codex", codex_home);
        let mut leaves = Vec::new();
        for (dirs, names) in [
            (&claude_dirs, ["settings.json", "settings.local.json"]),
            (&codex_dirs, ["hooks.json", "config.toml"]),
        ] {
            for file in dirs.iter().flat_map(|dir| names.map(|name| dir.join(name))) {
                let linked = file
                    .symlink_metadata()
                    .is_ok_and(|metadata| metadata.file_type().is_symlink());
                if let Some(target) = linked.then(|| resolved(&file)) {
                    if !leaves.contains(&target) {
                        leaves.push(target);
                    }
                }
            }
        }
        Self {
            yoke: under(".yoke"),
            claude_dirs,
            codex_dirs,
            binaries: [".local/bin/yoke", ".cargo/bin/yoke"]
                .into_iter()
                .flat_map(under)
                .chain(exe.iter().flat_map(|path| forms(path)))
                .collect(),
            token: under(".yoke/off-token"),
            leaves,
        }
    }

    /// The paths this process's agent environment points at, and the running executable.
    fn from_env(home: &Path) -> Self {
        let absolute = |key: &str| {
            std::env::var_os(key)
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
        };
        Self::new(
            home,
            absolute("CLAUDE_CONFIG_DIR"),
            absolute("CODEX_HOME"),
            std::env::current_exe().ok(),
        )
    }

    /// Whether `path` is inside `~/.yoke`, a hook registration file, or the yoke executable.
    fn hit(&self, path: &Path) -> bool {
        forms(path).iter().any(|form| {
            self.yoke.iter().any(|dir| form.starts_with(dir))
                || self.binaries.contains(form)
                || self.leaves.contains(form)
                || self.registers_hooks(form)
        })
    }

    /// Claude Code reads hooks, `env` and `disableAllHooks` from `settings*.json` in any `.claude`
    /// directory, Codex from `hooks.json` and `config.toml` in its home and in a project `.codex`.
    fn registers_hooks(&self, path: &Path) -> bool {
        let (Some(name), Some(parent)) = (
            path.file_name().and_then(|name| name.to_str()),
            path.parent(),
        ) else {
            return false;
        };
        let settings = name.starts_with("settings") && name.ends_with(".json");
        if settings
            && (parent.file_name().is_some_and(|dir| dir == ".claude")
                || self.claude_dirs.iter().any(|dir| dir == parent))
        {
            return true;
        }
        if matches!(name, "hooks.json" | "config.toml")
            && self.codex_dirs.iter().any(|dir| dir == parent)
        {
            return true;
        }
        !self.codex_dirs.iter().any(|dir| path.starts_with(dir))
            && path.components().any(|part| part.as_os_str() == ".codex")
    }

    /// The protected files that exist at fixed places, for ancestor and glob checks.
    fn fixed_targets(&self) -> Vec<PathBuf> {
        let mut targets = self.yoke.clone();
        for dir in &self.claude_dirs {
            targets.extend(["settings.json", "settings.local.json"].map(|name| dir.join(name)));
        }
        for dir in &self.codex_dirs {
            targets.extend(["hooks.json", "config.toml"].map(|name| dir.join(name)));
        }
        targets.extend(self.leaves.iter().cloned());
        targets
    }

    /// Whether removing or moving `path` would remove a protected file: it holds one, or it is
    /// a `.claude` or `.codex` directory. The executable is not covered: its directory is shared.
    fn ancestor_of(&self, path: &Path) -> bool {
        let targets = self.fixed_targets();
        forms(path).iter().any(|form| {
            form.file_name()
                .is_some_and(|name| name == ".claude" || name == ".codex")
                || targets.iter().any(|target| target.starts_with(form))
        })
    }

    /// Whether a shell pattern, already expanded to an absolute path, can match a protected file.
    fn glob_hits(&self, pattern: &Path, cwd: &Path, ancestors: bool) -> bool {
        let pattern = normalized(pattern);
        let mut targets = self.fixed_targets();
        targets.extend(self.binaries.iter().cloned());
        for dir in cwd.ancestors() {
            for file in [
                ".claude/settings.json",
                ".claude/settings.local.json",
                ".codex/hooks.json",
                ".codex/config.toml",
            ] {
                targets.push(dir.join(file));
            }
            if ancestors {
                targets.push(dir.join(".claude"));
                targets.push(dir.join(".codex"));
            }
        }
        if ancestors {
            targets.extend(self.claude_dirs.iter().chain(&self.codex_dirs).cloned());
            targets.extend(self.yoke.iter().cloned());
        }
        targets.iter().any(|target| glob_path(&pattern, target))
    }

    fn reads_token(&self, path: &Path) -> bool {
        forms(path).iter().any(|form| self.token.contains(form))
    }

    fn glob_token(&self, pattern: &Path) -> bool {
        let pattern = normalized(pattern);
        self.token.iter().any(|token| glob_path(&pattern, token))
    }

    /// Whether `path` is the token or a directory that holds it.
    fn token_within(&self, path: &Path) -> bool {
        forms(path)
            .iter()
            .any(|form| self.token.iter().any(|token| token.starts_with(form)))
    }

    fn is_yoke_dir(&self, path: &Path) -> bool {
        forms(path).iter().any(|form| self.yoke.contains(form))
    }
}

/// Whether a path pattern matches `target` segment by segment, as a shell glob would.
fn glob_path(pattern: &Path, target: &Path) -> bool {
    let (pattern, target): (Vec<_>, Vec<_>) = (
        pattern.components().collect(),
        target.components().collect(),
    );
    pattern.len() == target.len()
        && pattern
            .iter()
            .zip(&target)
            .all(|(pattern, target)| match (pattern, target) {
                (Component::Normal(pattern), Component::Normal(target)) => {
                    let (pattern, target): (Vec<char>, Vec<char>) = (
                        pattern.to_string_lossy().chars().collect(),
                        target.to_string_lossy().chars().collect(),
                    );
                    // A wildcard never matches a leading dot.
                    !(target.first() == Some(&'.') && pattern.first() != Some(&'.'))
                        && glob_segment(&pattern, &target)
                }
                (pattern, target) => pattern == target,
            })
}

/// `*`, `?` and `[...]` (treated as any one character, which can only over-match).
fn glob_segment(pattern: &[char], text: &[char]) -> bool {
    match pattern.split_first() {
        None => text.is_empty(),
        Some(('*', rest)) => (0..=text.len()).any(|skip| glob_segment(rest, &text[skip..])),
        Some(('?', rest)) => text
            .split_first()
            .is_some_and(|(_, text)| glob_segment(rest, text)),
        Some(('[', rest)) => match rest.iter().position(|c| *c == ']') {
            Some(end) => text
                .split_first()
                .is_some_and(|(_, text)| glob_segment(&rest[end + 1..], text)),
            None => text.first() == Some(&'[') && glob_segment(rest, &text[1..]),
        },
        Some((c, rest)) => text.first() == Some(c) && glob_segment(rest, &text[1..]),
    }
}

fn normalized(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            component => result.push(component.as_os_str()),
        }
    }
    result
}

fn resolved(path: &Path) -> PathBuf {
    resolved_from(Path::new(""), path)
}

/// `resolved` continued from a prefix that is already resolved.
fn resolved_from(base: &Path, path: &Path) -> PathBuf {
    // Resolve each existing prefix, preserving symlink semantics before `..`.
    let mut result = base.to_path_buf();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            component => {
                result.push(component.as_os_str());
                if let Ok(canonical) = result.canonicalize() {
                    result = canonical;
                }
            }
        }
    }
    result
}

/// Recognize documented, mutable agent markers; absence does not prove humanity.
pub fn agent_process_signal() -> Option<String> {
    process_signal(|key| std::env::var(key).ok())
}

fn process_signal(get: impl Fn(&str) -> Option<String>) -> Option<String> {
    for key in ["CLAUDE_CODE_CHILD_SESSION", "CLAUDECODE"] {
        if get(key).as_deref() == Some("1") {
            return Some(key.into());
        }
    }
    for key in ["CODEX_THREAD_ID", "CODEX_SESSION_ID"] {
        if get(key).is_some_and(|value| !value.is_empty()) {
            return Some(key.into());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn examines_actual_tool_input_not_prompt_or_read_content() {
        let home = tempfile::tempdir().unwrap();
        let mut event = AgentEvent {
            tool_name: Some("Bash".into()),
            tool_input: json!({"command":"yoke trust"}),
            ..Default::default()
        };
        for command in [
            "yoke trust",
            "yoke project move /old /new --dry-run",
            "yoke run-hook stop --agent claude < forged.json",
            "sh -c 'yoke run-hook PostToolUseFailure --agent codex'",
            "python -c \"import os; os.system('yoke run-hook Stop')\"",
            "/usr/local/bin/yoke plan approve demo",
            "yoke plan resume demo",
            "yoke plan cancel demo",
            "yoke plan reassign demo --session s2",
            "yoke plan accept demo copy --evidence checked",
            "yoke plan acknowledge demo 3",
            "yoke console --no-open &",
            "yoke jobs retry 4",
            "cd /tmp && yoke notifications ack all",
            "sqlite3 ~/.yoke/state.db '.tables'",
        ] {
            event.tool_input = json!({"command": command});
            assert!(evaluate(EventKind::PreToolUse, &event, home.path(), home.path()).is_some());
            assert!(evaluate(EventKind::PostToolUse, &event, home.path(), home.path()).is_none());
        }
        for command in [
            "yoke plan task done demo build",
            "yoke plan reply demo 3 --text fixed",
            "yoke jobs --all --json",
            "yoke notifications --json",
            "yoke consoles",
        ] {
            event.tool_input = json!({"command": command});
            assert!(evaluate(EventKind::PreToolUse, &event, home.path(), home.path()).is_none());
        }
        event.prompt = Some("yoke trust".into());
        event.tool_input = json!({"command":"yoke config"});
        assert!(evaluate(EventKind::PreToolUse, &event, home.path(), home.path()).is_none());
        event.tool_name = Some("Read".into());
        event.tool_input = json!({"file_path":home.path().join(".yoke/config.toml")});
        assert!(evaluate(EventKind::PreToolUse, &event, home.path(), home.path()).is_none());
    }

    #[test]
    fn protects_write_and_patch_targets_including_symlink_parents() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir(home.path().join(".yoke")).unwrap();
        std::os::unix::fs::symlink(home.path().join(".yoke"), home.path().join("alias")).unwrap();
        std::fs::create_dir(home.path().join("outside")).unwrap();
        std::os::unix::fs::symlink(
            home.path().join("outside"),
            home.path().join(".yoke/external"),
        )
        .unwrap();
        let mut event = AgentEvent {
            tool_name: Some("Write".into()),
            ..Default::default()
        };
        for path in [
            "~/.yoke/config.toml",
            "alias/new-file",
            ".yoke/../.yoke/new-file",
            ".yoke/external/config.toml",
        ] {
            event.tool_input = json!({"file_path":path});
            assert!(
                evaluate(EventKind::PreToolUse, &event, home.path(), home.path()).is_some(),
                "{path}"
            );
        }
        event.tool_input = json!({"file_path":".yoke-other/config.toml", "content":"state.db"});
        assert!(evaluate(EventKind::PreToolUse, &event, home.path(), home.path()).is_none());
        event.tool_name = Some("apply_patch".into());
        event.tool_input =
            json!({"command":"*** Begin Patch\n*** Update File: alias/config.toml\n*** End Patch"});
        assert!(evaluate(EventKind::PreToolUse, &event, home.path(), home.path()).is_some());
    }

    #[test]
    fn human_detection_uses_verified_markers_without_assuming_tty_proves_humanity() {
        assert_eq!(
            process_signal(|key| (key == "CODEX_THREAD_ID").then(|| "test-session".into())),
            Some("CODEX_THREAD_ID".into())
        );
        assert_eq!(
            process_signal(|key| (key == "CLAUDE_CODE_CHILD_SESSION").then(|| "1".into())),
            Some("CLAUDE_CODE_CHILD_SESSION".into())
        );
        assert_eq!(process_signal(|_| Some(String::new())), None);
        assert_eq!(process_signal(|_| None), None);
    }

    fn shell(command: &str) -> AgentEvent {
        AgentEvent {
            tool_name: Some("Bash".into()),
            tool_input: json!({"command": command}),
            ..Default::default()
        }
    }

    /// A home with a real `.yoke` directory and a project directory beside it.
    fn homes() -> (tempfile::TempDir, PathBuf) {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir(home.path().join(".yoke")).unwrap();
        let project = home.path().join("project");
        std::fs::create_dir(&project).unwrap();
        (home, project)
    }

    fn mismatches(home: &Path, cwd: &Path, rows: &[(&str, bool)]) -> Vec<String> {
        rows.iter()
            .filter_map(|(command, deny)| {
                let reason = evaluate(EventKind::PreToolUse, &shell(command), home, cwd);
                (reason.is_some() != *deny)
                    .then(|| format!("{command:?}: expected deny={deny}, got {reason:?}"))
            })
            .collect()
    }

    #[test]
    fn denies_disguised_human_command_invocations() {
        let (home, project) = homes();
        let rows = [
            ("~/.cargo/bin/yoke trust", true),
            ("/usr/bin/env yoke trust", true),
            ("command yoke console", true),
            ("bash -c 'yoke trust'", true),
            (r#"sh -lc "yoke console --no-open""#, true),
            ("nohup yoke console &", true),
            ("FOO=1 yoke trust", true),
            ("yoke\ttrust", true),
            ("yo''ke trust", true),
            (r#"yoke "trust""#, true),
            ("yoke plan  approve demo", true),
            ("$(yoke trust)", true),
            ("echo $(yoke trust)", true),
            (r#"echo "$(yoke trust)""#, true),
            ("echo `yoke trust`", true),
            ("exec yoke console", true),
            ("xargs yoke trust", true),
            (r"find . -exec yoke trust \;", true),
            (r#"eval "yoke trust""#, true),
            ("sudo -u me yoke trust", true),
            ("timeout 30 yoke console --no-open", true),
            ("echo yoke trust | sh", true),
            (r"printf 'yoke trust\n' | bash", true),
            (
                r#"python3 -c "import subprocess; subprocess.run(['yoke','trust'])""#,
                true,
            ),
            ("GIT_EDITOR='yoke trust' git commit", true),
            ("cd /tmp && yoke notifications ack all", true),
            ("true; yoke jobs retry 4", true),
            ("ls && yoke plan resume demo", true),
            ("ls || yoke plan cancel demo", true),
            ("(yoke plan reassign demo --session s2)", true),
            ("{ yoke plan accept demo copy --evidence checked; }", true),
            ("if yoke plan resolve demo 3; then echo ok; fi", true),
            (r"YOKE=1 \yoke trust", true),
            ("$'yoke' trust", true),
            ("tmux send-keys 'yoke trust' Enter", true),
            (r#"osascript -e 'do shell script "yoke trust"'"#, true),
            (r#"bash -lc 'cd ~ && yo""ke console'"#, true),
            ("YOKE trust", true),
        ];
        let failures = mismatches(home.path(), &project, &rows);
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn allows_mentions_of_human_commands_that_do_not_run_them() {
        let (home, project) = homes();
        let rows = [
            (r#"git log --grep "yoke trust""#, false),
            (r#"grep -rn "yoke console" docs/"#, false),
            (r#"echo "run yoke trust in your terminal""#, false),
            ("cat docs/cli.md", false),
            ("cargo test -p yoke-core guard::", false),
            ("rg 'yoke plan approve' crates/", false),
            (r#"git commit -m "docs: explain yoke trust""#, false),
            (
                r#"find docs -name '*.md' | xargs grep -l "yoke console""#,
                false,
            ),
            ("yoke plan task done demo build", false),
            ("yoke consoles", false),
            ("yoke trusted", false),
            ("python3 gen.py docs/yoke-console.md", false),
            ("yoke --version", false),
            (r#"bash -lc 'grep -rn "yoke console" docs/'"#, false),
            ("ls ~/.yoke", false),
            ("cat ~/.yoke/config.toml", false),
            ("yoke config", false),
        ];
        let failures = mismatches(home.path(), &project, &rows);
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn denies_shell_mutation_of_yoke_dir_and_database_siblings() {
        let (home, project) = homes();
        let abs = home.path().display();
        let owned = [
            format!("rm -rf {abs}/.yoke/"),
            format!("sqlite3 {abs}/.yoke/state.db-journal .tables"),
            format!(r#"python3 -c "import sqlite3; sqlite3.connect('{abs}/.yoke/state.db')""#),
        ];
        let mut rows = vec![
            ("rm ~/.yoke/state.db*", true),
            ("rm -rf ~/.yoke", true),
            (r#"rm -rf "$HOME"/.yoke/"#, true),
            ("rm -rf ${HOME}/.yoke", true),
            ("mv ~/.yoke ~/.yoke.bak", true),
            ("sqlite3 ~/.yoke/state.d*", true),
            ("sqlite3 ~/.yoke/sta''te.db", true),
            (r#"sqlite3 "$HOME/.yoke/sta""te.db""#, true),
            ("echo x > ~/.yoke/config.toml", true),
            ("echo x >> ~/.yoke/config.toml", true),
            ("echo x >~/.yoke/config.toml", true),
            ("echo x | tee ~/.yoke/config.toml", true),
            (r#"echo x | tee -a "$HOME/.yoke/config.toml""#, true),
            ("ln -s /tmp/evil ~/.yoke/config.toml", true),
            ("ln -s ~/.yoke /tmp/alias", true),
            ("truncate -s 0 ~/.yoke/state.db-wal", true),
            ("dd if=/dev/zero of=~/.yoke/state.db-shm", true),
            ("chmod 000 ~/.yoke", true),
            ("chown -R nobody ~/.yoke/", true),
            ("cd ~/.yoke && rm -rf .", true),
            ("cd ~ && rm -rf .yoke", true),
            ("cp /tmp/x ~/.yoke/", true),
            ("install -m 644 /tmp/x ~/.yoke/config.toml", true),
            ("touch ~/.yoke/new", true),
            ("mkdir ~/.yoke/sub", true),
            ("sed -i '' 's/a/b/' ~/.yoke/config.toml", true),
            ("rm -rf ~/.yok*", true),
            ("rm -rf ~", true),
            ("bash -c 'rm -rf ~/.yoke'", true),
            ("find ~/.yoke -delete", true),
            ("find ~/.yoke -type f | xargs rm -f", true),
            (r"find ~/.yoke -type f -exec rm {} ;", true),
            ("ls ~/.yoke | xargs rm", true),
            ("cd ~/.yoke && ls | xargs rm -f", true),
            (
                "apply_patch <<'EOF'\n*** Begin Patch\n*** Add File: ~/.yoke/x\n+1\n*** End Patch\nEOF",
                true,
            ),
            // Any mention of state.db is denied, even outside ~/.yoke: documented contract.
            ("ls tests/fixtures/state.db", true),
        ];
        rows.extend(owned.iter().map(|command| (command.as_str(), true)));
        let failures = mismatches(home.path(), &project, &rows);
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn allows_shell_reads_and_unprotected_paths() {
        let (home, project) = homes();
        // A sibling directory, never a fixed one: tests create their home under the system temp
        // directory, and deleting an ancestor of ~/.yoke is denied by design.
        let elsewhere = tempfile::tempdir().unwrap();
        let cd_elsewhere = format!("cd {} && rm -rf .", elsewhere.path().display());
        let rows = [
            ("rm -rf target/", false),
            ("rm -rf .yoke-other", false),
            ("ls ~/.yoke", false),
            ("cat ~/.yoke/config.toml", false),
            ("cp ~/.yoke/config.toml /tmp/", false),
            ("echo x > docs/yoke-console.md", false),
            ("rm -rf ~/.yoke-backup", false),
            ("touch ~/.yokefile", false),
            ("mkdir -p ~/.config/yoke", false),
            ("tee notes.txt", false),
            ("git status", false),
            ("sqlite3 tests/fixtures/other.db .tables", false),
            ("rm -rf .", false),
            (cd_elsewhere.as_str(), false),
            ("echo x 2>&1 | tee log.txt", false),
            ("cat list.txt | xargs rm -f", false),
            (r"find target -name '*.o' -exec rm {} ;", false),
            (r"find . -name '*.rs' -exec grep -l yoke {} +", false),
            (r#"echo "yoke trust" > notes.txt"#, false),
            ("cd .. && rm -rf build", false),
            ("./target/debug/yoke plan status demo", false),
            ("python3 -m pytest tests/", false),
            ("sed -i '' 's/x/y/' ~/.yokerc", false),
            ("yoke --json jobs retry 1", true),
        ];
        let failures = mismatches(home.path(), &project, &rows);
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn handles_codex_argv_cmd_and_workdir_shapes() {
        let (home, project) = homes();
        let yoke_dir = home.path().join(".yoke");
        let mut event = AgentEvent {
            tool_name: Some("shell".into()),
            ..Default::default()
        };
        let rows = [
            (json!({"command": ["bash", "-lc", "yoke trust"]}), true),
            (json!({"command": ["yoke", "trust"]}), true),
            (json!({"command": ["yoke trust"]}), true),
            (
                json!({"command": ["bash", "-lc", "grep -rn 'yoke console' docs/"]}),
                false,
            ),
            (json!({"command": ["grep", "yoke trust", "docs"]}), false),
            (json!({"command": ["rm", "-rf", yoke_dir]}), true),
            (json!({"cmd": "yoke trust"}), true),
            (json!({"command": "rm -rf .", "workdir": yoke_dir}), true),
            (json!({"command": "rm -rf .", "workdir": project}), false),
            (
                json!({"command": ["bash", "-lc", "rm -rf ."], "workdir": yoke_dir}),
                true,
            ),
        ];
        let failures: Vec<String> = rows
            .iter()
            .filter_map(|(input, deny)| {
                event.tool_input = input.clone();
                let reason = evaluate(EventKind::PreToolUse, &event, home.path(), &project);
                (reason.is_some() != *deny)
                    .then(|| format!("{input}: expected deny={deny}, got {reason:?}"))
            })
            .collect();
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn protects_write_tool_paths_with_home_variable_traversal_and_nested_edits() {
        let (home, project) = homes();
        let yoke_dir = home.path().join(".yoke");
        let rows = [
            (
                "Write",
                json!({"file_path": "$HOME/.yoke/x"}),
                &project,
                true,
            ),
            (
                "Write",
                json!({"file_path": "~/.yoke/../.yoke/x"}),
                &project,
                true,
            ),
            ("Write", json!({"file_path": "~/.yoke/"}), &project, true),
            ("Write", json!({"file_path": "x"}), &yoke_dir, true),
            ("Write", json!({"file_path": "../.yoke/x"}), &project, true),
            (
                "Edit",
                json!({"file_path": yoke_dir.join("config.toml")}),
                &project,
                true,
            ),
            (
                "MultiEdit",
                json!({"edits": [{"file_path": "~/.yoke/x"}]}),
                &project,
                true,
            ),
            (
                "NotebookEdit",
                json!({"notebook_path": "~/.yoke/n.ipynb"}),
                &project,
                true,
            ),
            (
                "Write",
                json!({"file_path": "docs/yoke-console.md"}),
                &project,
                false,
            ),
            (
                "Write",
                json!({"file_path": "~/.yoke-other/x"}),
                &project,
                false,
            ),
            ("Write", json!({"file_path": "x"}), &project, false),
            (
                "apply_patch",
                json!({"command": "*** Begin Patch\n*** Update File: a.md\n*** Move to: ~/.yoke/a.md\n*** End Patch"}),
                &project,
                true,
            ),
            (
                "apply_patch",
                json!("*** Begin Patch\n*** Add File: $HOME/.yoke/a.md\n+1\n*** End Patch"),
                &project,
                true,
            ),
            (
                "apply_patch",
                json!({"command": "*** Begin Patch\n*** Add File: docs/yoke-console.md\n+1\n*** End Patch"}),
                &project,
                false,
            ),
        ];
        let failures: Vec<String> = rows
            .iter()
            .filter_map(|(tool, input, cwd, deny)| {
                let event = AgentEvent {
                    tool_name: Some((*tool).into()),
                    tool_input: input.clone(),
                    ..Default::default()
                };
                let reason = evaluate(EventKind::PreToolUse, &event, home.path(), cwd);
                (reason.is_some() != *deny)
                    .then(|| format!("{tool} {input}: expected deny={deny}, got {reason:?}"))
            })
            .collect();
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn protects_hook_registration_files_and_the_yoke_binary() {
        let (home, project) = homes();
        let abs = home.path().display();
        std::fs::create_dir_all(home.path().join(".claude")).unwrap();
        std::fs::create_dir_all(home.path().join(".codex")).unwrap();
        std::fs::create_dir_all(home.path().join(".local/bin")).unwrap();
        std::fs::create_dir_all(project.join(".claude")).unwrap();
        std::fs::write(home.path().join(".claude/settings.json"), "{}").unwrap();
        std::os::unix::fs::symlink(
            home.path().join(".claude/settings.json"),
            project.join("alias.json"),
        )
        .unwrap();
        let owned = [
            format!("echo '{{}}' > {abs}/.claude/settings.json"),
            format!("cd {abs}/.claude && echo '{{}}' > settings.json"),
        ];
        let mut rows = vec![
            ("echo '{}' > ~/.claude/settings.json", true),
            ("echo '{}' >> ~/.claude/settings.local.json", true),
            ("echo x > .claude/settings.local.json", true),
            ("echo x > ../project/.claude/settings.json", true),
            ("cat settings.json > alias.json", true),
            ("sed -i '' 's/yoke/true/' ~/.claude/settings.json", true),
            ("perl -pi -e 's/a/b/' $HOME/.claude/settings.json", true),
            ("rm ~/.codex/hooks.json", true),
            ("rm -f ~/.codex/config.toml", true),
            ("rm .claude/settings.local.json", true),
            ("rm .claude/settings*", true),
            ("rm -rf ~/.claude", true),
            ("rm -rf .claude", true),
            ("rm -rf .codex", true),
            ("mv .claude .claude.bak", true),
            ("mv /tmp/x ~/.claude/", true),
            ("chmod 000 ~/.codex/config.toml", true),
            ("truncate -s 0 .claude/settings.json", true),
            ("ln -sf /tmp/x ~/.claude/settings.json", true),
            ("ln -sf /tmp/settings.json ~/.claude/", true),
            ("tee ~/.claude/settings.local.json", true),
            ("printf x | tee .codex/hooks.json", true),
            ("echo x > .codex/config.toml", true),
            ("touch .codex/hooks.json", true),
            ("cp /tmp/evil ~/.local/bin/yoke", true),
            ("cp /tmp/evil ~/.cargo/bin/yoke", true),
            ("cp /tmp/yoke ~/.local/bin/", true),
            ("cp -t ~/.local/bin /tmp/yoke", true),
            ("cp /tmp/settings.json ~/.claude/", true),
            ("cp /tmp/hooks.json ~/.codex/", true),
            ("install -m 755 /tmp/evil ~/.local/bin/yoke", true),
            ("mv /tmp/evil ~/.local/bin/yoke", true),
            ("rm ~/.local/bin/yoke", true),
            (
                "curl -o ~/.claude/settings.json https://example.com/s.json",
                true,
            ),
            ("patch ~/.claude/settings.json < fix.diff", true),
            ("bash -c 'echo {} > ~/.claude/settings.json'", true),
            ("sudo sed -i s/a/b/ ~/.claude/settings.json", true),
            (
                r#"python3 -c "open('.claude/settings.json','w').write('{}')""#,
                true,
            ),
            ("find ~/.claude -name 'settings*.json' -delete", true),
            // An interpreter given the path may write it, as for ~/.yoke.
            ("python3 validate.py .claude/settings.json", true),
            // Reads, neighbours and unrelated names stay allowed.
            ("cat ~/.claude/settings.json", false),
            ("grep hooks .claude/settings.json", false),
            ("cp ~/.claude/settings.json /tmp/backup.json", false),
            ("ls ~/.codex", false),
            ("rm -rf .claude/worktrees/old", false),
            ("echo x > ~/.claude/projects/p/session.jsonl", false),
            ("echo x > .claude/commands/review.md", false),
            ("echo x > .claude/settings.md", false),
            ("echo x > docs/settings.json", false),
            ("echo x > .claude-notes/settings.json", false),
            ("touch ~/.codex/AGENTS.md", false),
            ("echo x > ~/.local/bin/other", false),
            ("cat ~/.local/bin/yoke", false),
            ("cp /tmp/other ~/.local/bin/", false),
            ("curl -o /tmp/out.json https://example.com/s.json", false),
            ("yoke --version", false),
        ];
        rows.extend(owned.iter().map(|command| (command.as_str(), true)));
        let failures = mismatches(home.path(), &project, &rows);
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn protects_hook_registration_files_from_edit_tools_and_patches() {
        let (home, project) = homes();
        std::fs::create_dir_all(home.path().join(".claude")).unwrap();
        std::fs::create_dir(home.path().join("dotfiles")).unwrap();
        std::fs::write(home.path().join("dotfiles/claude-settings.json"), "{}").unwrap();
        std::os::unix::fs::symlink(
            home.path().join("dotfiles/claude-settings.json"),
            home.path().join(".claude/settings.json"),
        )
        .unwrap();
        std::os::unix::fs::symlink(
            home.path().join(".claude/settings.json"),
            project.join("alias.json"),
        )
        .unwrap();
        let rows = [
            // The real file behind a symlinked settings.json is protected too.
            (
                "Write",
                json!({"file_path": "~/dotfiles/claude-settings.json"}),
                true,
            ),
            (
                "Write",
                json!({"file_path": "~/.claude/settings.json"}),
                true,
            ),
            (
                "Write",
                json!({"file_path": ".claude/settings.local.json"}),
                true,
            ),
            (
                "Write",
                json!({"file_path": "$HOME/.codex/hooks.json"}),
                true,
            ),
            ("Write", json!({"file_path": ".codex/config.toml"}), true),
            ("Write", json!({"file_path": "alias.json"}), true),
            ("Write", json!({"file_path": "~/.local/bin/yoke"}), true),
            (
                "Edit",
                json!({"file_path": "~/.claude/settings.json"}),
                true,
            ),
            (
                "MultiEdit",
                json!({"edits": [{"file_path": ".claude/settings.json"}]}),
                true,
            ),
            (
                "apply_patch",
                json!({"command": "*** Begin Patch\n*** Update File: .claude/settings.json\n*** End Patch"}),
                true,
            ),
            (
                "apply_patch",
                json!("*** Begin Patch\n*** Add File: $HOME/.codex/hooks.json\n+{}\n*** End Patch"),
                true,
            ),
            (
                "Write",
                json!({"file_path": "~/.claude/commands/x.md"}),
                false,
            ),
            ("Write", json!({"file_path": ".claude/settings.md"}), false),
            ("Write", json!({"file_path": "docs/settings.json"}), false),
            (
                "Read",
                json!({"file_path": "~/.claude/settings.json"}),
                false,
            ),
            (
                "apply_patch",
                json!({"command": "*** Begin Patch\n*** Add File: docs/settings.json\n+1\n*** End Patch"}),
                false,
            ),
        ];
        let failures: Vec<String> = rows
            .iter()
            .filter_map(|(tool, input, deny)| {
                let event = AgentEvent {
                    tool_name: Some((*tool).into()),
                    tool_input: input.clone(),
                    ..Default::default()
                };
                let reason = evaluate(EventKind::PreToolUse, &event, home.path(), &project);
                (reason.is_some() != *deny)
                    .then(|| format!("{tool} {input}: expected deny={deny}, got {reason:?}"))
            })
            .collect();
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn protects_configured_agent_directories_and_the_running_executable() {
        let (home, project) = homes();
        let config = tempfile::tempdir().unwrap();
        let exe = home.path().join("custom/bin/yoke");
        let protected = Protected::new(
            home.path(),
            Some(config.path().join("claude")),
            Some(config.path().join("codex")),
            Some(exe.clone()),
        );
        let rows = [
            (
                format!("echo x > {}/claude/settings.json", config.path().display()),
                true,
            ),
            (
                format!("echo x > {}/codex/hooks.json", config.path().display()),
                true,
            ),
            (format!("cp /tmp/evil {}", exe.display()), true),
            (
                format!("echo x > {}/claude/history.jsonl", config.path().display()),
                false,
            ),
        ];
        for (command, deny) in rows {
            let reason = evaluate_in(
                EventKind::PreToolUse,
                &shell(&command),
                home.path(),
                &project,
                &protected,
            );
            assert_eq!(reason.is_some(), deny, "{command}: {reason:?}");
        }
    }

    #[test]
    fn makes_install_uninstall_update_and_pause_human_only() {
        let (home, project) = homes();
        let rows = [
            ("yoke uninstall", true),
            ("yoke uninstall --agent claude,codex", true),
            ("yoke install", true),
            ("yoke install --agent claude --statusline", true),
            ("~/.local/bin/yoke update", true),
            ("yoke pause", true),
            ("yoke pause --for 30m", true),
            ("bash -c 'yoke uninstall'", true),
            ("sudo yoke install", true),
            (
                r#"python3 -c "import subprocess; subprocess.run(['yoke','uninstall'])""#,
                true,
            ),
            ("echo yoke pause | sh", true),
            ("yoke install --dry-run", false),
            ("yoke install --agent claude --dry-run", false),
            ("yoke uninstall --dry-run", false),
            ("yoke update --check", false),
            ("yoke installed", false),
            (r#"git commit -m "docs: explain yoke install""#, false),
            (r#"grep -rn "yoke pause" docs/"#, false),
        ];
        let failures = mismatches(home.path(), &project, &rows);
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn keeps_the_bypass_token_private_to_the_human() {
        let (home, project) = homes();
        std::fs::write(home.path().join(".yoke/off-token"), "secret").unwrap();
        let abs = home.path().display();
        let owned = [
            format!("cat {abs}/.yoke/off-token"),
            format!("cd {abs}/.yoke && cat off-token"),
        ];
        let mut rows = vec![
            ("cat ~/.yoke/off-token", true),
            ("cat $HOME/.yoke/off-token", true),
            ("head -c 40 ~/.yoke/off-token", true),
            ("cat < ~/.yoke/off-token", true),
            ("cat ~/.yoke/off-*", true),
            ("cat ~/.yoke/*", true),
            ("cp ~/.yoke/off-token /tmp/t", true),
            ("bash -c 'cat ~/.yoke/off-token'", true),
            ("cat ~/.yoke/config.toml", false),
            ("cat ~/.yoke/*.toml", false),
            ("ls ~/.yoke/*", false),
            ("stat ~/.yoke/off-token", false),
        ];
        rows.extend(owned.iter().map(|command| (command.as_str(), true)));
        let failures = mismatches(home.path(), &project, &rows);
        assert!(failures.is_empty(), "{}", failures.join("\n"));
        for (tool, input, deny) in [
            ("Read", json!({"file_path": "~/.yoke/off-token"}), true),
            ("Read", json!({"file_path": "~/.yoke/config.toml"}), false),
            ("Grep", json!({"pattern": "a", "path": "~/.yoke"}), true),
            ("Grep", json!({"pattern": "a", "path": "src"}), false),
        ] {
            let event = AgentEvent {
                tool_name: Some(tool.into()),
                tool_input: input.clone(),
                ..Default::default()
            };
            let reason = evaluate(EventKind::PreToolUse, &event, home.path(), &project);
            assert_eq!(reason.is_some(), deny, "{tool} {input}: {reason:?}");
        }
    }

    #[test]
    #[ignore = "residual: shell aliases, variables, copied binaries and encoded payloads are not resolved"]
    fn residual_bypasses_through_indirection_are_known() {
        let (home, project) = homes();
        let rows = [
            ("alias y=yoke; y trust", true),
            ("Y=yoke; $Y trust", true),
            ("cp $(command -v yoke) /tmp/y && /tmp/y trust", true),
            ("echo eW9rZSB0cnVzdAo= | base64 -d | sh", true),
            (r#"echo "yoke trust" | xargs -I CMD sh -c CMD"#, true),
            (
                "python3 - <<'EOF'\nimport sqlite3, os\nsqlite3.connect(os.path.expanduser('~/.yoke/sta' + 'te.db'))\nEOF",
                true,
            ),
        ];
        let failures = mismatches(home.path(), &project, &rows);
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }
}
