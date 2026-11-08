//! File operations behind the console's Extensions page: view, rename, move between scopes and
//! delete into a Yoke-owned trash.
//!
//! Every target is looked up in a fresh inventory scan, so a path from the browser can only name
//! something the scan found. Changes only reach user and project extension folders, refuse links
//! and anything that would overwrite, and a delete moves the item into the trash instead of
//! unlinking it, with its original path recorded beside it.

use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};

use anyhow::{anyhow, bail, ensure, Context, Result};
use serde::Serialize;

use crate::extensions::{self, Item, Kind, Owner, Roots, Scope};
use crate::redact::redact;

/// Largest file shown in full; a longer one is cut and marked.
const VIEW_CAP: u64 = 512 * 1024;
const NAME_MAX: usize = 64;
const COPY_MAX_ENTRIES: usize = 2000;
const COPY_MAX_BYTES: u64 = 64 * 1024 * 1024;
const COPY_MAX_DEPTH: usize = 8;

/// One extension file the inventory found, resolved to what the console can act on.
#[derive(Clone, Debug)]
pub struct Target {
    pub agent: Owner,
    pub kind: Kind,
    pub scope: Scope,
    /// Name the inventory shows; a skill's own name can differ from its folder name.
    pub name: String,
    /// File that holds the content (`SKILL.md` for a skill).
    pub file: PathBuf,
    /// What rename, move and delete act on: the file, or a skill's folder.
    pub unit: PathBuf,
    /// Folder that holds this kind of extension in this scope; `None` when it is read-only here.
    root: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    Markdown,
    Toml,
    Starlark,
}

#[derive(Debug, Serialize)]
pub struct Content {
    pub text: String,
    pub format: Format,
    pub chars: usize,
    pub lines: usize,
    /// About one token per four characters, the estimate Pilot also uses.
    pub tokens: usize,
    /// The file is longer than the view limit and `text` stops early.
    pub truncated: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Rename,
    Move,
    Delete,
}

/// A validated change. `to` is the new path, or for a delete the folder inside the trash.
#[derive(Clone, Debug)]
pub struct Change {
    pub op: Op,
    pub from: PathBuf,
    pub to: PathBuf,
    /// Delete only: what is written beside the item so it can be put back by hand.
    origin_note: Option<String>,
}

pub fn parse_scope(text: &str) -> Option<Scope> {
    match text {
        "user" => Some(Scope::User),
        "project" => Some(Scope::Project),
        _ => None,
    }
}

pub fn scope_label(scope: Scope) -> &'static str {
    match scope {
        Scope::Managed => "managed",
        Scope::Local => "local",
        Scope::Project => "project",
        Scope::User => "user",
        Scope::System => "system",
    }
}

/// Kinds whose content is a file the page can show.
pub fn is_file_kind(kind: Kind) -> bool {
    matches!(
        kind,
        Kind::Skill | Kind::Command | Kind::Agent | Kind::Rule | Kind::Memory
    )
}

/// Folder that holds `kind` for `agent` in `scope`, where Yoke is willing to change things.
pub fn kind_root(roots: &Roots, agent: Owner, kind: Kind, scope: Scope) -> Option<PathBuf> {
    let project = &roots.project;
    let dir = match (agent, kind, scope) {
        (Owner::Claude, Kind::Skill, Scope::User) => roots.claude_dir.join("skills"),
        (Owner::Claude, Kind::Skill, Scope::Project) => project.join(".claude/skills"),
        (Owner::Claude, Kind::Command, Scope::User) => roots.claude_dir.join("commands"),
        (Owner::Claude, Kind::Command, Scope::Project) => project.join(".claude/commands"),
        (Owner::Claude, Kind::Agent, Scope::User) => roots.claude_dir.join("agents"),
        (Owner::Claude, Kind::Agent, Scope::Project) => project.join(".claude/agents"),
        (Owner::Claude, Kind::Rule, Scope::User) => roots.claude_dir.join("rules"),
        (Owner::Claude, Kind::Rule, Scope::Project) => project.join(".claude/rules"),
        (Owner::Codex, Kind::Skill, Scope::User) => roots.home.join(".agents/skills"),
        (Owner::Codex, Kind::Skill, Scope::Project) => project.join(".agents/skills"),
        (Owner::Codex, Kind::Agent, Scope::User) => roots.codex_home.join("agents"),
        (Owner::Codex, Kind::Agent, Scope::Project) => project.join(".codex/agents"),
        (Owner::Codex, Kind::Command, Scope::User) => roots.codex_home.join("prompts"),
        (Owner::Codex, Kind::Rule, Scope::User) => roots.codex_home.join("rules"),
        (Owner::Codex, Kind::Rule, Scope::Project) => project.join(".codex/rules"),
        _ => return None,
    };
    Some(dir)
}

/// File extension of a single-file extension; `None` for a skill (a folder) or an unsupported kind.
pub fn extension(agent: Owner, kind: Kind) -> Option<&'static str> {
    match (agent, kind) {
        (Owner::Claude, Kind::Command | Kind::Agent | Kind::Rule) => Some("md"),
        (Owner::Codex, Kind::Command) => Some("md"),
        (Owner::Codex, Kind::Agent) => Some("toml"),
        (Owner::Codex, Kind::Rule) => Some("rules"),
        _ => None,
    }
}

/// Where the extension called `name` lives (or would live) for this agent, kind and scope.
pub fn unit_path(
    roots: &Roots,
    agent: Owner,
    kind: Kind,
    scope: Scope,
    name: &str,
) -> Option<PathBuf> {
    let root = kind_root(roots, agent, kind, scope)?;
    match (kind, extension(agent, kind)) {
        (Kind::Skill, _) => Some(root.join(name)),
        (_, Some(ext)) => Some(root.join(format!("{name}.{ext}"))),
        _ => None,
    }
}

/// Look `origin` up in a fresh inventory scan; anything the scan did not find is refused.
pub fn find(roots: &Roots, origin: &Path) -> Result<Target> {
    let report = extensions::scan(roots);
    let item = report
        .items
        .iter()
        .find(|item| item.origin == origin && is_file_kind(item.kind))
        .ok_or_else(|| {
            anyhow!("That extension is not in the inventory any more; reload the page")
        })?;
    target_for(roots, item)
}

/// Resolve an inventory item the caller already holds.
pub fn target_for(roots: &Roots, item: &Item) -> Result<Target> {
    let root = kind_root(roots, item.agent, item.kind, item.scope);
    let unit = match item.kind {
        Kind::Skill => item
            .origin
            .parent()
            .map(Path::to_path_buf)
            .context("A skill file has no folder")?,
        _ => item.origin.clone(),
    };
    // The scan walks the folder it was given, so a file it found sits under that folder as
    // listed; anything else is not something to act on.
    let root = root.filter(|root| unit.strip_prefix(root).is_ok());
    Ok(Target {
        agent: item.agent,
        kind: item.kind,
        scope: item.scope,
        name: item.name.clone(),
        file: item.origin.clone(),
        unit,
        root,
    })
}

impl Target {
    /// Whether rename, move and delete may be offered; managed, local, system and memory
    /// files are shown but changed where they live.
    pub fn manageable(&self) -> bool {
        self.root.is_some()
    }

    /// Directly inside the kind folder (not in a subfolder), so it has a plain shareable name.
    pub fn is_top_level(&self) -> bool {
        self.root
            .as_deref()
            .is_some_and(|root| self.unit.parent() == Some(root))
    }

    /// Refuse links and anything that is not a plain file or folder inside the kind folder.
    pub fn ensure_plain(&self) -> Result<()> {
        check_source(self)
    }

    /// The other of user and project, when this agent keeps this kind there too.
    pub fn other_scope(&self, roots: &Roots) -> Option<Scope> {
        let other = match self.scope {
            Scope::User => Scope::Project,
            Scope::Project => Scope::User,
            _ => return None,
        };
        self.manageable()
            .then(|| kind_root(roots, self.agent, self.kind, other))
            .flatten()
            .map(|_| other)
    }

    fn root(&self) -> Result<&Path> {
        self.root.as_deref().ok_or_else(|| {
            anyhow!(
                "This {} is outside the user and project extension folders Yoke manages; change it where it lives",
                extensions_kind(self.kind)
            )
        })
    }

    /// Path below the kind folder, e.g. `team/review.md`.
    fn relative(&self) -> Result<PathBuf> {
        Ok(self.unit.strip_prefix(self.root()?)?.to_path_buf())
    }
}

fn extensions_kind(kind: Kind) -> &'static str {
    match kind {
        Kind::Skill => "skill",
        Kind::Command => "command",
        Kind::Agent => "agent",
        Kind::Rule => "rule",
        _ => "entry",
    }
}

/// Show the file: at most `VIEW_CAP` bytes, secret-shaped values hidden, with size statistics.
pub fn read(target: &Target) -> Result<Content> {
    let mut bytes = Vec::new();
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(&target.file)
        .with_context(|| format!("Cannot open {}", target.file.display()))?;
    ensure!(file.metadata()?.is_file(), "Not a regular file");
    let length = file.metadata()?.len();
    file.take(VIEW_CAP).read_to_end(&mut bytes)?;
    let raw = String::from_utf8_lossy(&bytes).into_owned();
    let format = match target.file.extension().and_then(|ext| ext.to_str()) {
        Some("rules") => Format::Starlark,
        Some("toml") => Format::Toml,
        _ => Format::Markdown,
    };
    let chars = raw.chars().count();
    Ok(Content {
        text: redact(&raw),
        format,
        chars,
        lines: raw.lines().count(),
        tokens: chars.div_ceil(4),
        truncated: length > VIEW_CAP,
    })
}

/// A single path segment: letters, digits, `.`, `_` and `-`, starting with a letter or digit.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= NAME_MAX
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        && !name.contains("..")
        && !name.ends_with('.')
}

fn exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

fn is_link(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
}

/// Refuse when `root` or any existing folder or file from `root` down to `path` is a link.
pub fn ensure_no_links(root: &Path, path: &Path) -> Result<()> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| anyhow!("{} is outside {}", path.display(), root.display()))?;
    let mut current = root.to_path_buf();
    ensure!(
        !is_link(&current),
        "{} is a link; change it where it lives",
        current.display()
    );
    for part in relative.components() {
        ensure!(
            matches!(part, Component::Normal(_)),
            "{} is not a plain path inside {}",
            path.display(),
            root.display()
        );
        current.push(part);
        ensure!(
            !is_link(&current),
            "{} is a link; change it where it lives",
            current.display()
        );
    }
    Ok(())
}

fn check_source(target: &Target) -> Result<()> {
    let root = target.root()?;
    ensure_no_links(root, &target.unit)?;
    ensure_no_links(root, &target.file)?;
    let meta = fs::symlink_metadata(&target.unit)
        .with_context(|| format!("{} is gone; reload the page", target.unit.display()))?;
    ensure!(
        meta.is_file() || meta.is_dir(),
        "{} is not a plain file or folder",
        target.unit.display()
    );
    Ok(())
}

pub fn plan_rename(target: &Target, new_name: &str) -> Result<Change> {
    check_source(target)?;
    ensure!(
        valid_name(new_name),
        "A name uses letters, digits, '.', '_' and '-', starts with a letter or digit, and has at most {NAME_MAX} characters"
    );
    let to = match target.unit.extension() {
        Some(ext) if target.kind != Kind::Skill => target
            .unit
            .with_file_name(format!("{new_name}.{}", ext.to_string_lossy())),
        _ => target.unit.with_file_name(new_name),
    };
    ensure!(to != target.unit, "That is already the name");
    ensure!(
        !exists(&to),
        "{} already exists; nothing is overwritten",
        to.display()
    );
    Ok(Change {
        op: Op::Rename,
        from: target.unit.clone(),
        to,
        origin_note: None,
    })
}

pub fn plan_move(roots: &Roots, target: &Target, to_scope: Scope) -> Result<Change> {
    check_source(target)?;
    ensure!(
        matches!(to_scope, Scope::User | Scope::Project) && to_scope != target.scope,
        "Pick the other scope: user or project"
    );
    let destination_root =
        kind_root(roots, target.agent, target.kind, to_scope).ok_or_else(|| {
            anyhow!(
                "A {} has no {} scope for this agent",
                extensions_kind(target.kind),
                scope_label(to_scope)
            )
        })?;
    let to = destination_root.join(target.relative()?);
    ensure_no_links(&destination_root, &to)?;
    ensure!(
        !exists(&to),
        "{} already exists; nothing is overwritten",
        to.display()
    );
    Ok(Change {
        op: Op::Move,
        from: target.unit.clone(),
        to,
        origin_note: None,
    })
}

/// Trash folder inside the state directory.
pub fn trash_dir(state_dir: &Path) -> PathBuf {
    state_dir.join("extension-trash")
}

pub fn plan_delete(target: &Target, state_dir: &Path, stamp: u64) -> Result<Change> {
    check_source(target)?;
    plan_trash(
        &target.unit,
        &format!(
            "{}-{}",
            agent_label(target.agent),
            scope_label(target.scope)
        ),
        state_dir,
        stamp,
    )
}

/// Move `unit` into the trash. The caller has already checked it is a plain item it may remove.
pub fn plan_trash(unit: &Path, label: &str, state_dir: &Path, stamp: u64) -> Result<Change> {
    let base = unit
        .file_name()
        .context("Nothing to delete")?
        .to_string_lossy();
    Ok(Change {
        op: Op::Delete,
        from: unit.to_path_buf(),
        to: trash_dir(state_dir).join(format!("{stamp}-{label}-{base}")),
        origin_note: Some(format!("{}\n", unit.display())),
    })
}

/// Create `to` (a file, or a folder for a folder `from`) and copy `from` into it. An existing
/// `to` is refused; links inside `from` stop the copy and leave nothing behind.
pub fn install_copy(from: &Path, to: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(from)?;
    ensure!(
        !meta.file_type().is_symlink(),
        "{} is a link",
        from.display()
    );
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Cannot create {}", parent.display()))?;
    }
    let reserved = if meta.is_dir() {
        fs::create_dir(to)
    } else {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(to)
            .map(drop)
    };
    reserved.with_context(|| format!("{} already exists or cannot be created", to.display()))?;
    if let Err(error) = copy_unit(from, to) {
        let _ = if meta.is_dir() {
            fs::remove_dir_all(to)
        } else {
            fs::remove_file(to)
        };
        return Err(error);
    }
    Ok(())
}

fn agent_label(agent: Owner) -> &'static str {
    match agent {
        Owner::Claude => "claude",
        Owner::Codex => "codex",
        Owner::Yoke => "yoke",
    }
}

/// Carry out a validated change. A delete goes into `to/<name>` with `to/origin.txt` beside it.
pub fn apply(change: &Change) -> Result<()> {
    match change.op {
        Op::Rename => move_path(&change.from, &change.to),
        Op::Move => {
            let parent = change.to.parent().context("No destination folder")?;
            fs::create_dir_all(parent)
                .with_context(|| format!("Cannot create {}", parent.display()))?;
            move_path(&change.from, &change.to)
        }
        Op::Delete => {
            let mut holder = change.to.clone();
            let mut attempt = 1;
            loop {
                if let Some(parent) = holder.parent() {
                    fs::create_dir_all(parent)?;
                }
                match fs::create_dir(&holder) {
                    Ok(()) => break,
                    Err(error) if error.kind() == ErrorKind::AlreadyExists && attempt < 100 => {
                        attempt += 1;
                        holder = PathBuf::from(format!("{}-{attempt}", change.to.display()));
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            let name = change.from.file_name().context("Nothing to delete")?;
            let note = change.origin_note.as_deref().unwrap_or_default();
            fs::write(holder.join("origin.txt"), note)?;
            if let Err(error) = move_path(&change.from, &holder.join(name)) {
                let _ = fs::remove_file(holder.join("origin.txt"));
                let _ = fs::remove_dir(&holder);
                return Err(error);
            }
            Ok(())
        }
    }
}

/// Move without ever replacing an existing destination: the destination is created first
/// (atomically refusing an existing one), then the source is renamed over that empty
/// placeholder, or copied across when the two sit on different volumes.
fn move_path(from: &Path, to: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(from)
        .with_context(|| format!("{} is gone; reload the page", from.display()))?;
    ensure!(
        !meta.file_type().is_symlink(),
        "{} is a link; change it where it lives",
        from.display()
    );
    let directory = meta.is_dir();
    let reserved = if directory {
        fs::create_dir(to)
    } else {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(to)
            .map(drop)
    };
    reserved.with_context(|| format!("{} already exists or cannot be created", to.display()))?;
    let discard = |to: &Path| {
        let _ = if directory {
            fs::remove_dir_all(to)
        } else {
            fs::remove_file(to)
        };
    };
    match fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(error) if error.raw_os_error() == Some(libc::EXDEV) => {
            if let Err(error) = copy_unit(from, to) {
                discard(to);
                return Err(error);
            }
            if directory {
                fs::remove_dir_all(from)?;
            } else {
                fs::remove_file(from)?;
            }
            Ok(())
        }
        Err(error) => {
            discard(to);
            Err(error).with_context(|| format!("Cannot move {}", from.display()))
        }
    }
}

#[derive(Default)]
struct Budget {
    entries: usize,
    bytes: u64,
}

/// Copy a file, or a folder into the existing empty folder `to`; links and other special files
/// stop the copy, so nothing outside the item can be pulled in.
fn copy_unit(from: &Path, to: &Path) -> Result<()> {
    let mut budget = Budget::default();
    copy_node(from, to, 0, &mut budget)
}

fn copy_node(from: &Path, to: &Path, depth: usize, budget: &mut Budget) -> Result<()> {
    let meta = fs::symlink_metadata(from)?;
    budget.entries += 1;
    ensure!(
        depth <= COPY_MAX_DEPTH && budget.entries <= COPY_MAX_ENTRIES,
        "{} is too large to move across volumes",
        from.display()
    );
    if meta.is_file() {
        budget.bytes += meta.len();
        ensure!(
            budget.bytes <= COPY_MAX_BYTES,
            "{} is too large to move across volumes",
            from.display()
        );
        fs::copy(from, to).with_context(|| format!("Cannot copy {}", from.display()))?;
        return Ok(());
    }
    if !meta.is_dir() {
        bail!(
            "{} is a link or special file; it is not copied",
            from.display()
        );
    }
    if depth > 0 {
        fs::create_dir(to)?;
    }
    let mut children: Vec<_> = fs::read_dir(from)?.collect::<std::io::Result<_>>()?;
    children.sort_by_key(|entry| entry.file_name());
    for child in children {
        copy_node(
            &child.path(),
            &to.join(child.file_name()),
            depth + 1,
            budget,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    struct Fixture {
        _temp: tempfile::TempDir,
        roots: Roots,
        state: PathBuf,
    }

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn fixture() -> Fixture {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let home = base.join("home");
        let roots = Roots {
            project: base.join("project"),
            claude_dir: home.join(".claude"),
            claude_json: home.join(".claude.json"),
            claude_managed: base.join("managed"),
            codex_home: home.join(".codex"),
            codex_system: base.join("codex-system"),
            home: home.clone(),
        };
        write(&roots.claude_dir.join("commands/ship.md"), "abcd\nefgh\n");
        write(
            &roots.claude_dir.join("skills/review/SKILL.md"),
            "---\nname: review\n---\nBody\n",
        );
        write(
            &roots.claude_dir.join("skills/review/scripts/run.sh"),
            "echo hi\n",
        );
        write(
            &roots.project.join(".claude/agents/helper.md"),
            "---\nname: helper\n---\n",
        );
        write(
            &roots.claude_managed.join(".claude/skills/corp/SKILL.md"),
            "corp\n",
        );
        Fixture {
            state: home.join(".yoke"),
            roots,
            _temp: temp,
        }
    }

    fn target(f: &Fixture, relative: &str) -> Target {
        find(&f.roots, &f.roots.claude_dir.join(relative)).unwrap()
    }

    #[test]
    fn read_counts_chars_lines_and_approximate_tokens() {
        let f = fixture();
        let content = read(&target(&f, "commands/ship.md")).unwrap();
        assert_eq!((content.chars, content.lines, content.tokens), (10, 2, 3));
        assert_eq!(content.format, Format::Markdown);
        assert!(!content.truncated);
    }

    #[test]
    fn rename_keeps_content_and_never_overwrites() {
        let f = fixture();
        write(&f.roots.claude_dir.join("commands/taken.md"), "mine\n");
        let command = target(&f, "commands/ship.md");
        let error = plan_rename(&command, "taken").unwrap_err().to_string();
        assert!(error.contains("already exists"), "{error}");
        for bad in ["", "../x", "a/b", ".hidden", "x y", "a..b"] {
            assert!(
                plan_rename(&command, bad).is_err(),
                "{bad:?} must be refused"
            );
        }
        let change = plan_rename(&command, "release").unwrap();
        assert_eq!(change.to, f.roots.claude_dir.join("commands/release.md"));
        apply(&change).unwrap();
        assert_eq!(fs::read_to_string(&change.to).unwrap(), "abcd\nefgh\n");
        assert!(!exists(&command.unit));

        let skill = target(&f, "skills/review/SKILL.md");
        let change = plan_rename(&skill, "audit").unwrap();
        apply(&change).unwrap();
        assert!(f
            .roots
            .claude_dir
            .join("skills/audit/scripts/run.sh")
            .is_file());
        assert!(!exists(&f.roots.claude_dir.join("skills/review")));
    }

    #[test]
    fn move_carries_a_whole_skill_folder_to_the_other_scope() {
        let f = fixture();
        let skill = target(&f, "skills/review/SKILL.md");
        assert!(
            plan_move(&f.roots, &skill, Scope::User).is_err(),
            "same scope"
        );
        let change = plan_move(&f.roots, &skill, Scope::Project).unwrap();
        assert_eq!(change.to, f.roots.project.join(".claude/skills/review"));
        apply(&change).unwrap();
        assert!(f
            .roots
            .project
            .join(".claude/skills/review/SKILL.md")
            .is_file());
        assert!(f
            .roots
            .project
            .join(".claude/skills/review/scripts/run.sh")
            .is_file());
        assert!(!exists(&f.roots.claude_dir.join("skills/review")));
        // The destination now holds one, so moving another with the same name must refuse.
        write(
            &f.roots.claude_dir.join("skills/review/SKILL.md"),
            "again\n",
        );
        let again = target(&f, "skills/review/SKILL.md");
        assert!(plan_move(&f.roots, &again, Scope::Project)
            .unwrap_err()
            .to_string()
            .contains("already exists"));
    }

    #[test]
    fn delete_goes_to_the_trash_with_the_original_path_recorded() {
        let f = fixture();
        let agent = find(&f.roots, &f.roots.project.join(".claude/agents/helper.md")).unwrap();
        let change = plan_delete(&agent, &f.state, 1700000000).unwrap();
        assert!(change.to.starts_with(f.state.join("extension-trash")));
        apply(&change).unwrap();
        assert!(!exists(&agent.unit));
        assert_eq!(
            fs::read_to_string(change.to.join("helper.md")).unwrap(),
            "---\nname: helper\n---\n"
        );
        assert_eq!(
            fs::read_to_string(change.to.join("origin.txt")).unwrap(),
            format!("{}\n", agent.unit.display())
        );
    }

    #[test]
    fn links_managed_files_and_unknown_paths_are_refused() {
        let f = fixture();
        let outside = f.roots.home.join("elsewhere.md");
        write(&outside, "outside\n");
        symlink(&outside, f.roots.claude_dir.join("commands/linked.md")).unwrap();
        let linked = target(&f, "commands/linked.md");
        for result in [
            plan_rename(&linked, "other").map(drop),
            plan_move(&f.roots, &linked, Scope::Project).map(drop),
            plan_delete(&linked, &f.state, 1).map(drop),
        ] {
            assert!(result.unwrap_err().to_string().contains("link"));
        }
        assert!(outside.is_file(), "the link target is untouched");

        let managed = find(
            &f.roots,
            &f.roots.claude_managed.join(".claude/skills/corp/SKILL.md"),
        )
        .unwrap();
        assert!(plan_delete(&managed, &f.state, 1)
            .unwrap_err()
            .to_string()
            .contains("outside"));
        assert!(find(&f.roots, Path::new("/etc/passwd")).is_err());
        assert!(find(
            &f.roots,
            &f.roots.claude_dir.join("commands/../commands/ship.md")
        )
        .is_err());
    }

    #[test]
    fn copying_across_volumes_stops_at_links() {
        let f = fixture();
        let skill = f.roots.claude_dir.join("skills/review");
        let to = f.roots.home.join("copy");
        fs::create_dir(&to).unwrap();
        copy_unit(&skill, &to).unwrap();
        assert!(to.join("scripts/run.sh").is_file());
        symlink("/etc/passwd", skill.join("leak")).unwrap();
        let blocked = f.roots.home.join("blocked");
        fs::create_dir(&blocked).unwrap();
        assert!(copy_unit(&skill, &blocked)
            .unwrap_err()
            .to_string()
            .contains("link"));
    }
}
