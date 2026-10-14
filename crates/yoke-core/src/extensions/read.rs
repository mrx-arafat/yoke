//! Bounded, link-aware file access and command resolution without executing anything.

use std::fs::{self, File};
use std::io::{ErrorKind, Read};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use super::Note;

pub(super) const CONFIG_CAP: u64 = 4 * 1024 * 1024;
pub(super) const HEAD_CAP: u64 = 64 * 1024;
const MAX_DEPTH: usize = 8;
const MAX_FILES: usize = 2000;
const BUILTINS: &[&str] = &[
    ".", ":", "[", "case", "cd", "command", "echo", "eval", "exec", "exit", "export", "false",
    "for", "if", "printf", "set", "source", "test", "true", "while",
];

pub(super) struct Reader {
    roots: Vec<PathBuf>,
    pub(super) notes: Vec<Note>,
}

impl Reader {
    pub(super) fn new(roots: &[&Path]) -> Self {
        let mut reader = Self {
            roots: Vec::new(),
            notes: Vec::new(),
        };
        for root in roots {
            if fs::symlink_metadata(root).is_ok_and(|meta| meta.file_type().is_symlink()) {
                if let Ok(target) = root.canonicalize() {
                    reader.note(
                        root,
                        format!("scanned root is a symlink to {}", target.display()),
                    );
                }
            }
            reader.roots.extend(root.canonicalize());
        }
        reader
    }

    pub(super) fn note(&mut self, path: &Path, note: impl Into<String>) {
        self.notes.push(Note {
            path: path.to_path_buf(),
            note: note.into(),
        });
    }

    /// Report links that leave every scanned root; returns whether `path` is a symlink.
    pub(super) fn link(&mut self, path: &Path) -> bool {
        if !fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink()) {
            return false;
        }
        match path.canonicalize() {
            Ok(target) if self.roots.iter().any(|root| target.starts_with(root)) => {}
            Ok(target) => self.note(
                path,
                format!("symlink outside scanned roots: {}", target.display()),
            ),
            Err(_) => self.note(path, "symlink target is missing"),
        }
        true
    }

    /// Read at most `cap` bytes of a regular file; `whole` skips larger files instead.
    pub(super) fn text(&mut self, path: &Path, cap: u64, whole: bool) -> Option<String> {
        self.link(path);
        let meta = match fs::metadata(path) {
            Ok(meta) => meta,
            Err(error) if error.kind() == ErrorKind::NotFound => return None,
            Err(error) => {
                self.note(path, format!("unreadable: {error}"));
                return None;
            }
        };
        if !meta.is_file() {
            self.note(path, "not a regular file");
            return None;
        }
        if whole && meta.len() > cap {
            self.note(path, format!("skipped: larger than {cap} bytes"));
            return None;
        }
        let mut bytes = Vec::new();
        if let Err(error) = File::open(path).and_then(|file| file.take(cap).read_to_end(&mut bytes))
        {
            self.note(path, format!("unreadable: {error}"));
            return None;
        }
        match String::from_utf8(bytes) {
            Ok(text) => Some(text),
            Err(error) if !whole => Some(String::from_utf8_lossy(error.as_bytes()).into_owned()),
            Err(_) => {
                self.note(path, "skipped: not UTF-8");
                None
            }
        }
    }

    pub(super) fn exists(&mut self, path: &Path) -> bool {
        self.link(path);
        fs::metadata(path).is_ok_and(|meta| meta.is_file())
    }

    /// Sorted direct children; a missing directory has none.
    pub(super) fn entries(&mut self, dir: &Path) -> Vec<PathBuf> {
        let mut paths: Vec<PathBuf> = match fs::read_dir(dir) {
            Ok(entries) => entries
                .filter_map(|entry| Some(entry.ok()?.path()))
                .collect(),
            Err(error) if error.kind() == ErrorKind::NotFound => Vec::new(),
            Err(error) => {
                self.note(dir, format!("unreadable: {error}"));
                Vec::new()
            }
        };
        paths.sort();
        paths
    }

    /// Files with `extension` below `dir`; directory links are reported, never descended.
    pub(super) fn walk(&mut self, dir: &Path, extension: &str) -> Vec<PathBuf> {
        let mut files = Vec::new();
        let mut pending = vec![(dir.to_path_buf(), 0)];
        while let Some((current, depth)) = pending.pop() {
            for path in self.entries(&current) {
                let link = self.link(&path);
                let Ok(meta) = fs::metadata(&path) else {
                    continue;
                };
                if meta.is_dir() {
                    if link {
                        self.note(&path, "symlinked directory not walked");
                    } else if depth + 1 < MAX_DEPTH {
                        pending.push((path, depth + 1));
                    } else {
                        self.note(&path, "directory deeper than 8 levels not walked");
                    }
                } else if meta.is_file() && path.extension().is_some_and(|ext| ext == extension) {
                    if files.len() == MAX_FILES {
                        self.note(dir, "stopped after 2000 files");
                        return files;
                    }
                    files.push(path);
                }
            }
        }
        files.sort();
        files
    }
}

/// The `name:` value of leading YAML frontmatter, without parsing the rest of the file.
pub(super) fn frontmatter_name(text: &str) -> Option<String> {
    let mut lines = text.lines();
    if lines.next()?.trim_end() != "---" {
        return None;
    }
    for line in lines {
        let line = line.trim_end();
        if line == "---" {
            break;
        }
        if let Some(value) = line.strip_prefix("name:") {
            let value = value.trim().trim_matches(['"', '\'']);
            return (!value.is_empty()).then(|| value.to_owned());
        }
    }
    None
}

/// The program a shell command line starts and how many words follow it on the line, when
/// knowable without running a shell. Leading variable assignments are skipped.
pub(super) fn program(command: &str) -> Option<(String, usize)> {
    let mut rest = command.trim_start();
    loop {
        let (word, tail) = shell_word(rest)?;
        let assignment = word.split_once('=').is_some_and(|(name, _)| {
            name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        });
        if !assignment {
            return (!word.is_empty()).then(|| (word, words(tail)));
        }
        rest = tail.trim_start();
    }
}

/// Shell words in `line`; operators separate words and are not counted.
fn words(mut line: &str) -> usize {
    let mut count = 0;
    loop {
        line = line.trim_start_matches(|c: char| c.is_whitespace() || matches!(c, ';' | '&' | '|'));
        if line.is_empty() {
            return count;
        }
        count += 1;
        // An unclosed quote runs to the end of the line.
        let Some((_, tail)) = shell_word(line) else {
            return count;
        };
        line = tail;
    }
}

fn shell_word(input: &str) -> Option<(String, &str)> {
    let mut word = String::new();
    let mut index = 0;
    while let Some(byte) = input.as_bytes().get(index) {
        match byte {
            b' ' | b'\t' | b'\n' | b';' | b'&' | b'|' => break,
            b'\'' | b'"' => {
                let close = input[index + 1..].find(*byte as char)? + index + 1;
                word.push_str(&input[index + 1..close]);
                index = close + 1;
            }
            _ => {
                let next = index + input[index..].chars().next()?.len_utf8();
                word.push_str(&input[index..next]);
                index = next;
            }
        }
    }
    Some((word, &input[index..]))
}

/// Whether `program` names an existing executable; `None` when shell expansion would decide.
pub(super) fn resolves(program: &str, home: &Path, project: &Path) -> Option<bool> {
    let mut expanded = program.to_owned();
    for (variable, value) in [
        ("${CLAUDE_PROJECT_DIR}", project),
        ("$CLAUDE_PROJECT_DIR", project),
        ("${HOME}", home),
        ("$HOME", home),
    ] {
        expanded = expanded.replace(variable, value.to_str()?);
    }
    if let Some(rest) = expanded.strip_prefix("~/") {
        expanded = home.join(rest).to_str()?.to_owned();
    }
    if expanded.is_empty() || expanded.contains(['$', '`', '\\', '*', '?', '[', '{', '(', '~']) {
        return None;
    }
    if expanded.contains('/') {
        return Some(executable(&project.join(expanded)));
    }
    if BUILTINS.contains(&expanded.as_str()) {
        return Some(true);
    }
    let paths = std::env::var_os("PATH")?;
    Some(std::env::split_paths(&paths).any(|dir| executable(&dir.join(&expanded))))
}

fn executable(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}
