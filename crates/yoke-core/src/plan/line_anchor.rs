//! `file:<path>:<line>` anchors: a note on one line of a file in the plan's project.
//! Unlike plan-text anchors these point at the working tree, so they are checked against it.
use std::io::Read;
use std::path::{Component, Path};

use anyhow::{ensure, Context, Result};

const PREFIX: &str = "file:";
/// Larger files are accepted by name and line number alone.
const COUNT_LIMIT: u64 = 4 * 1024 * 1024;

pub fn is_line_anchor(anchor: &str) -> bool {
    anchor.starts_with(PREFIX)
}

/// The path and 1-based line of a well-formed anchor.
pub fn parse(anchor: &str) -> Option<(&str, u32)> {
    let (path, line) = anchor.strip_prefix(PREFIX)?.rsplit_once(':')?;
    let line: u32 = line.parse().ok().filter(|line| *line >= 1)?;
    let ok = !path.is_empty()
        && path.len() <= 1024
        && !path.chars().any(char::is_control)
        && Path::new(path)
            .components()
            .all(|part| matches!(part, Component::Normal(_)));
    ok.then_some((path, line))
}

/// Build the anchor for `line` of `path`, refusing paths an anchor could not name.
pub fn format(path: &str, line: u32) -> Result<String> {
    let anchor = format!("{PREFIX}{path}:{line}");
    ensure!(
        parse(&anchor) == Some((path, line)),
        "{path:?} line {line} cannot be anchored; use a project-relative file path and a line from 1"
    );
    Ok(anchor)
}

/// The anchored line exists now: a regular, non-binary file inside `project` with that many lines.
pub fn check(project: &Path, anchor: &str) -> Result<()> {
    let (path, line) = parse(anchor).with_context(|| {
        format!("{anchor:?} is not a file anchor; use file:<project-relative path>:<line number from 1>")
    })?;
    let project = project
        .canonicalize()
        .context("resolve the plan's project")?;
    let full = project.join(path);
    let meta = std::fs::symlink_metadata(&full)
        .with_context(|| format!("{path} does not exist in the project"))?;
    ensure!(
        meta.is_file(),
        "{path} is not a regular file, so a line of it cannot be anchored"
    );
    ensure!(
        full.canonicalize()?.starts_with(&project),
        "{path} resolves outside the project"
    );
    if meta.len() > COUNT_LIMIT {
        return Ok(());
    }
    let mut bytes = Vec::new();
    std::fs::File::open(&full)?
        .take(COUNT_LIMIT)
        .read_to_end(&mut bytes)?;
    ensure!(
        !bytes[..bytes.len().min(8192)].contains(&0),
        "{path} is a binary file, so a line of it cannot be anchored"
    );
    let lines = bytes.iter().filter(|byte| **byte == b'\n').count()
        + usize::from(bytes.last().is_some_and(|byte| *byte != b'\n'));
    ensure!(
        line as usize <= lines,
        "{path} has {lines} line(s), so line {line} does not exist"
    );
    Ok(())
}

pub fn exists(project: &Path, anchor: &str) -> bool {
    check(project, anchor).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_well_formed_anchors_pointing_at_real_lines_are_accepted() {
        assert_eq!(parse("file:src/a.rs:12"), Some(("src/a.rs", 12)));
        assert_eq!(parse("file:dir/we:ird.txt:3"), Some(("dir/we:ird.txt", 3)));
        for bad in [
            "file:a.rs",
            "file:a.rs:0",
            "file:a.rs:-1",
            "file:a.rs:x",
            "file::5",
            "file:/etc/passwd:1",
            "file:../up.rs:1",
            "file:a/../b.rs:1",
            "file:./a.rs:1",
            "file:a\n.rs:1",
            "plan",
        ] {
            assert_eq!(parse(bad), None, "{bad:?}");
        }
        assert_eq!(format("src/a.rs", 3).unwrap(), "file:src/a.rs:3");
        assert!(format("../a.rs", 3).is_err() && format("a.rs", 0).is_err());

        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        std::fs::create_dir(&project).unwrap();
        std::fs::write(project.join("two.txt"), "one\ntwo").unwrap();
        std::fs::write(project.join("bin.dat"), [1_u8, 0, 2]).unwrap();
        std::fs::write(temp.path().join("outside.txt"), "x\n").unwrap();
        std::os::unix::fs::symlink(temp.path().join("outside.txt"), project.join("link.txt"))
            .unwrap();
        assert!(check(&project, "file:two.txt:1").is_ok());
        assert!(
            check(&project, "file:two.txt:2").is_ok(),
            "last line without newline counts"
        );
        for bad in [
            "file:two.txt:3",
            "file:missing.txt:1",
            "file:bin.dat:1",
            "file:link.txt:1",
        ] {
            assert!(check(&project, bad).is_err(), "{bad}");
        }
    }
}
