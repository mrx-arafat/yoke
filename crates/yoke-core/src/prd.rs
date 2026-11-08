//! Requirement documents: Markdown files under `docs/prd` in a project, the layout Pilot Shell uses.
//! Listing and reading never follow symbolic links and are size bounded; deletion needs the
//! exact content hash the human reviewed.
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::{bail, ensure, Context, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::gitcmd;

pub const DIR: &str = "docs/prd";
const MAX_BYTES: u64 = 1024 * 1024;
const MAX_DOCS: usize = 200;
/// Header lines Pilot reads from a document, in display order.
const META_KEYS: [&str; 6] = [
    "Author", "Agent", "Category", "Status", "Research", "Created",
];

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct PrdEntry {
    /// File name inside `docs/prd`, the only identifier clients use.
    pub file: String,
    /// File name without `.md` and without a leading `YYYY-MM-DD-`.
    pub name: String,
    pub title: String,
    pub modified_at: i64,
    pub bytes: u64,
    pub meta: BTreeMap<String, String>,
    /// `## ` headings, in order.
    pub sections: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Prd {
    pub entry: PrdEntry,
    pub content: String,
    /// SHA-256 of `content`, lower hex; deletion must quote it.
    pub sha256: String,
}

/// What deleting a document would lose, judged from Git.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Recovery {
    /// Committed and unchanged: `git restore` brings it back.
    Committed,
    /// Tracked but edited since the last commit: those edits would be lost.
    Edited,
    /// Not tracked by Git: nothing can bring it back.
    Untracked,
    /// Git could not say (not a repository, or Git failed); treat as unrecoverable.
    Unknown,
}

fn valid_file_name(name: &str) -> bool {
    name.len() <= 120
        && name.len() > 3
        && name.ends_with(".md")
        && !name.starts_with('.')
        && !name.contains("..")
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

/// `docs/prd` of the project, only when it is a real directory inside the project.
fn directory(project: &Path) -> Option<PathBuf> {
    let root = project.canonicalize().ok()?;
    let dir = root.join(DIR);
    let canonical = dir.canonicalize().ok()?;
    (canonical.starts_with(&root) && canonical.is_dir()).then_some(canonical)
}

fn display_name(file: &str) -> String {
    let stem = file.strip_suffix(".md").unwrap_or(file);
    let bytes = stem.as_bytes();
    let dated = bytes.len() > 11
        && bytes[..10].iter().enumerate().all(|(i, b)| match i {
            4 | 7 => *b == b'-',
            _ => b.is_ascii_digit(),
        })
        && bytes[10] == b'-';
    (if dated { &stem[11..] } else { stem }).to_owned()
}

fn parse(file: &str, content: &str, modified_at: i64, bytes: u64) -> PrdEntry {
    let mut meta = BTreeMap::new();
    let mut sections = Vec::new();
    let mut title = None;
    let mut fenced = false;
    for line in content.lines() {
        if line.starts_with("```") || line.starts_with("~~~") {
            fenced = !fenced;
        }
        if fenced {
            continue;
        }
        if let Some(heading) = line.strip_prefix("## ") {
            sections.push(heading.trim().to_owned());
        } else if title.is_none() {
            if let Some(heading) = line.strip_prefix("# ") {
                title = Some(heading.trim().to_owned());
            }
        }
        for key in META_KEYS {
            if let Some(value) = line
                .strip_prefix(key)
                .and_then(|rest| rest.strip_prefix(':'))
            {
                let value: String = value.trim().chars().take(200).collect();
                if !value.is_empty() {
                    meta.entry(key.to_owned()).or_insert(value);
                }
            }
        }
    }
    let name = display_name(file);
    PrdEntry {
        file: file.to_owned(),
        title: title
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| name.clone()),
        name,
        modified_at,
        bytes,
        meta,
        sections,
    }
}

/// Open a regular file without following a final symbolic link, reading at most the cap.
fn read_regular(path: &Path) -> Result<(Vec<u8>, i64)> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let metadata = file.metadata()?;
    ensure!(metadata.is_file(), "not a regular file");
    ensure!(metadata.len() <= MAX_BYTES, "larger than 1 MiB");
    let mut bytes = Vec::new();
    (&mut file).take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= MAX_BYTES, "larger than 1 MiB");
    let modified = metadata
        .modified()?
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    Ok((bytes, modified))
}

fn load(dir: &Path, file: &str) -> Result<(PrdEntry, String)> {
    let (bytes, modified) = read_regular(&dir.join(file))?;
    let content = String::from_utf8(bytes).context("not UTF-8 text")?;
    let entry = parse(file, &content, modified, content.len() as u64);
    Ok((entry, content))
}

/// Documents in the project, newest first. A missing directory is an empty list; a file that
/// is a link, oversized, not UTF-8 or oddly named is skipped rather than failing the page.
pub fn list(project: &Path) -> Vec<PrdEntry> {
    newest(project, MAX_DOCS)
}

/// The `limit` newest documents. Files are ordered by modification time first, so only those
/// that will be shown are opened and parsed.
fn newest(project: &Path, limit: usize) -> Vec<PrdEntry> {
    let Some(dir) = directory(project) else {
        return Vec::new();
    };
    let Ok(read) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut files: Vec<(std::time::SystemTime, String)> = read
        .filter_map(|item| {
            let item = item.ok()?;
            let name = item.file_name().into_string().ok()?;
            // DirEntry metadata does not follow links, so a symbolic link is not a regular file here.
            let metadata = item.metadata().ok()?;
            (valid_file_name(&name) && metadata.is_file())
                .then(|| (metadata.modified().unwrap_or(UNIX_EPOCH), name))
        })
        .collect();
    // Newest first; ties fall back to the name, also descending.
    files.sort_unstable();
    files.reverse();
    let mut entries = Vec::new();
    for (_, name) in files {
        if entries.len() >= limit {
            break;
        }
        if let Ok((entry, _)) = load(&dir, &name) {
            entries.push(entry);
        }
    }
    entries.sort_by(|a, b| {
        b.modified_at
            .cmp(&a.modified_at)
            .then_with(|| b.file.cmp(&a.file))
    });
    entries
}

/// Newest documents across several projects, with the project each came from.
pub fn recent<'a>(
    projects: impl IntoIterator<Item = &'a str>,
    limit: usize,
) -> Vec<(String, PrdEntry)> {
    let mut all: Vec<(String, PrdEntry)> = projects
        .into_iter()
        .flat_map(|project| {
            newest(Path::new(project), limit)
                .into_iter()
                .map(|entry| (project.to_owned(), entry))
        })
        .collect();
    all.sort_by_key(|(_, entry)| std::cmp::Reverse(entry.modified_at));
    all.truncate(limit);
    all
}

pub fn read(project: &Path, file: &str) -> Result<Prd> {
    ensure!(
        valid_file_name(file),
        "unknown requirement document; open it from the Requirements list"
    );
    let dir = directory(project).context("this project has no docs/prd directory")?;
    let (entry, content) = load(&dir, file)
        .with_context(|| format!("cannot read {DIR}/{file}; it may have been moved or replaced"))?;
    let sha256 = format!("{:x}", Sha256::digest(content.as_bytes()));
    Ok(Prd {
        entry,
        content,
        sha256,
    })
}

pub fn recovery(project: &Path, file: &str) -> Recovery {
    let Some(dir) = directory(project) else {
        return Recovery::Unknown;
    };
    let Ok(inside) = gitcmd::run(&dir, &["rev-parse", "--is-inside-work-tree"]) else {
        return Recovery::Unknown;
    };
    if !inside.success {
        return Recovery::Unknown;
    }
    let Ok(tracked) = gitcmd::run(&dir, &["ls-files", "--error-unmatch", "--", file]) else {
        return Recovery::Unknown;
    };
    if !tracked.success {
        // Git exits 1 with "did not match" for a file it does not track; anything else is unknown.
        return if tracked.stderr.contains("did not match") {
            Recovery::Untracked
        } else {
            Recovery::Unknown
        };
    }
    match gitcmd::run(&dir, &["status", "--porcelain", "--", file]) {
        Ok(status) if status.success && status.stdout.trim().is_empty() => Recovery::Committed,
        Ok(status) if status.success => Recovery::Edited,
        _ => Recovery::Unknown,
    }
}

/// Delete exactly the document whose content hash the human reviewed. A changed, replaced or
/// missing file refuses; nothing else is touched.
pub fn delete(project: &Path, file: &str, reviewed_sha256: &str) -> Result<()> {
    let current = read(project, file)?;
    if current.sha256 != reviewed_sha256 {
        bail!("{DIR}/{file} changed since you opened it; reopen it from Requirements and review it again");
    }
    let dir = directory(project).context("this project has no docs/prd directory")?;
    std::fs::remove_file(dir.join(file)).with_context(|| format!("deleting {DIR}/{file}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join(DIR)).unwrap();
        (temp, root)
    }

    #[test]
    fn documents_list_with_pilot_metadata_and_skip_links_and_oversized_files() {
        let (_temp, root) = project();
        let dir = root.join(DIR);
        std::fs::write(
            dir.join("2026-11-01-export.md"),
            "# Export\nAuthor: Sam\nStatus: Draft\n\n## Problem\n```\n## not a section\n```\n## Flows\n",
        )
        .unwrap();
        std::fs::write(dir.join("notes.txt"), "ignored").unwrap();
        std::fs::write(dir.join("big.md"), vec![b'x'; (MAX_BYTES + 1) as usize]).unwrap();
        std::fs::write(root.join("secret.md"), "# secret").unwrap();
        std::os::unix::fs::symlink(root.join("secret.md"), dir.join("link.md")).unwrap();

        let docs = list(&root);

        assert_eq!(docs.len(), 1, "{docs:?}");
        let doc = &docs[0];
        assert_eq!(
            (doc.name.as_str(), doc.title.as_str()),
            ("export", "Export")
        );
        assert_eq!(doc.sections, ["Problem", "Flows"]);
        assert_eq!(doc.meta["Author"], "Sam");
        assert!(!doc.meta.contains_key("Agent"));
        assert!(read(&root, "link.md").is_err());
        assert!(read(&root, "../secret.md").is_err());
    }

    #[test]
    fn deletion_needs_the_reviewed_hash_and_reports_git_recoverability() {
        let (_temp, root) = project();
        let git = |args: &[&str]| {
            let output = crate::process::git()
                .unwrap()
                .current_dir(&root)
                .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success(), "{args:?}");
        };
        git(&["init", "-q"]);
        std::fs::write(root.join(DIR).join("kept.md"), "# Kept\n").unwrap();
        std::fs::write(root.join(DIR).join("loose.md"), "# Loose\n").unwrap();
        git(&["add", "docs/prd/kept.md"]);
        git(&["commit", "-qm", "kept"]);
        assert_eq!(recovery(&root, "kept.md"), Recovery::Committed);
        assert_eq!(recovery(&root, "loose.md"), Recovery::Untracked);
        std::fs::write(root.join(DIR).join("kept.md"), "# Kept\nedit\n").unwrap();
        assert_eq!(recovery(&root, "kept.md"), Recovery::Edited);

        let reviewed = read(&root, "loose.md").unwrap().sha256;
        std::fs::write(root.join(DIR).join("loose.md"), "# Loose\nchanged\n").unwrap();
        let refused = delete(&root, "loose.md", &reviewed).unwrap_err();
        assert!(refused.to_string().contains("changed since"), "{refused:#}");
        assert!(root.join(DIR).join("loose.md").exists());

        let reviewed = read(&root, "loose.md").unwrap().sha256;
        delete(&root, "loose.md", &reviewed).unwrap();
        assert!(!root.join(DIR).join("loose.md").exists());
        assert!(root.join(DIR).join("kept.md").exists());
    }
}
