//! Inert line diffs for installation previews and trusted snapshot review.

use serde_json::Value;

const CONTEXT: usize = 2;
/// Above this many table cells the exact diff is skipped and the whole change is shown.
const MAX_CELLS: usize = 4_000_000;

/// `- old`, `+ new` and `  context` lines. JSON files are pretty-printed first so a one-line
/// settings file diffs per hook, and control characters are escaped so file text stays inert.
pub fn change_lines(before: Option<&str>, after: Option<&str>) -> Vec<String> {
    let (before, after) = (lines(before), lines(after));
    context_lines(&before, &after)
}

/// Exact snapshot text, without JSON normalization or newline loss.
pub fn snapshot_lines(before: Option<&str>, after: &str) -> Vec<String> {
    let exact = |text: &str| {
        text.split_inclusive('\n')
            .map(|line| {
                let mut quoted = String::new();
                for c in crate::commands::quote(line).chars() {
                    // JSON leaves C1 controls literal; keep them inert and JSON-decodable.
                    if c.is_control() {
                        quoted.push_str(&format!("\\u{:04x}", u32::from(c)));
                    } else {
                        quoted.push(c);
                    }
                }
                quoted
            })
            .collect::<Vec<_>>()
    };
    context_lines(&exact(before.unwrap_or("")), &exact(after))
}

fn context_lines(before: &[String], after: &[String]) -> Vec<String> {
    let ops = diff(before, after);
    let mut shown = vec![false; ops.len()];
    for (index, op) in ops.iter().enumerate() {
        if !matches!(op, Op::Same(_)) {
            let from = index.saturating_sub(CONTEXT);
            let to = (index + CONTEXT + 1).min(ops.len());
            shown[from..to].fill(true);
        }
    }
    let mut out = Vec::new();
    let mut skipping = false;
    for (op, shown) in ops.iter().zip(shown) {
        if !shown {
            if !skipping {
                out.push("  ...".to_owned());
                skipping = true;
            }
            continue;
        }
        skipping = false;
        out.push(match op {
            Op::Same(line) => format!("  {}", printable(line)),
            Op::Removed(line) => format!("- {}", printable(line)),
            Op::Added(line) => format!("+ {}", printable(line)),
        });
    }
    out
}

enum Op<'a> {
    Same(&'a str),
    Removed(&'a str),
    Added(&'a str),
}

fn lines(text: Option<&str>) -> Vec<String> {
    let text = text.unwrap_or("");
    match serde_json::from_str::<Value>(text) {
        Ok(value) => serde_json::to_string_pretty(&value)
            .unwrap_or_else(|_| text.to_owned())
            .lines()
            .map(str::to_owned)
            .collect(),
        Err(_) => text.lines().map(str::to_owned).collect(),
    }
}

/// Longest-common-subsequence diff; settings files are small, so the quadratic table is fine.
fn diff<'a>(before: &'a [String], after: &'a [String]) -> Vec<Op<'a>> {
    if before.len().saturating_mul(after.len()) > MAX_CELLS {
        return before
            .iter()
            .map(|line| Op::Removed(line))
            .chain(after.iter().map(|line| Op::Added(line)))
            .collect();
    }
    let width = after.len() + 1;
    let mut common = vec![0u32; (before.len() + 1) * width];
    for i in (0..before.len()).rev() {
        for j in (0..after.len()).rev() {
            common[i * width + j] = if before[i] == after[j] {
                common[(i + 1) * width + j + 1] + 1
            } else {
                common[(i + 1) * width + j].max(common[i * width + j + 1])
            };
        }
    }
    let (mut i, mut j, mut ops) = (0, 0, Vec::new());
    while i < before.len() && j < after.len() {
        if before[i] == after[j] {
            ops.push(Op::Same(&before[i]));
            (i, j) = (i + 1, j + 1);
        } else if common[(i + 1) * width + j] >= common[i * width + j + 1] {
            ops.push(Op::Removed(&before[i]));
            i += 1;
        } else {
            ops.push(Op::Added(&after[j]));
            j += 1;
        }
    }
    ops.extend(before[i..].iter().map(|line| Op::Removed(line)));
    ops.extend(after[j..].iter().map(|line| Op::Added(line)));
    ops
}

fn printable(line: &str) -> String {
    line.chars()
        .flat_map(|c| {
            if c.is_control() {
                c.escape_default().collect::<Vec<_>>()
            } else {
                vec![c]
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_diff_keeps_context_and_exact_line_bytes() {
        let before = "# first\n# second\n# third\nmode = 'observe'\n# fourth\n# fifth\n# sixth\n";
        let after = before.replace("observe", "enforce");
        let shown = snapshot_lines(Some(before), &after);
        assert!(
            shown.contains(&r#"- "mode = 'observe'\n""#.to_owned()),
            "{shown:?}"
        );
        assert!(
            shown.contains(&r#"+ "mode = 'enforce'\n""#.to_owned()),
            "{shown:?}"
        );
        assert!(
            shown.contains(&r##"  "# third\n""##.to_owned()),
            "{shown:?}"
        );
        assert!(
            !shown.iter().any(|line| line.contains("first")),
            "{shown:?}"
        );
        assert_eq!(shown.iter().filter(|line| *line == "  ...").count(), 2);

        // Neither JSON formatting nor line endings may hide a byte change.
        for (old, new) in [("{}", "{ }"), ("line\r\n", "line\n"), ("line\n", "line")] {
            let shown = snapshot_lines(Some(old), new);
            assert!(
                shown.contains(&format!("- {}", crate::commands::quote(old))),
                "{shown:?}"
            );
            assert!(
                shown.contains(&format!("+ {}", crate::commands::quote(new))),
                "{shown:?}"
            );
        }
        assert_eq!(
            snapshot_lines(None, "\u{1b}[31m\n"),
            [r#"+ "\u001b[31m\n""#]
        );
        assert_eq!(snapshot_lines(Some("old\n"), ""), [r#"- "old\n""#]);
        let controls = "\u{85}\u{9b}31m\n";
        let shown = snapshot_lines(None, controls);
        assert_eq!(
            serde_json::from_str::<String>(&shown[0][2..]).unwrap(),
            controls
        );
        assert!(!shown[0].chars().any(char::is_control));

        // Above the table cap, every old and new byte must remain reviewable.
        let old = "# old\n".repeat(2001);
        let new = "# new\n".repeat(2001);
        let shown = snapshot_lines(Some(&old), &new);
        assert_eq!(shown.len(), 4002);
        assert!(shown[..2001].iter().all(|line| line == r##"- "# old\n""##));
        assert!(shown[2001..].iter().all(|line| line == r##"+ "# new\n""##));
    }

    #[test]
    fn a_change_shows_only_changed_lines_with_context_and_inert_control_characters() {
        let before = r#"{"a":1,"b":2,"c":3,"d":4,"e":5,"f":6,"g":7}"#;
        let after = r#"{"a":1,"b":2,"c":3,"d":40,"e":5,"f":6,"g":7}"#;
        let shown = change_lines(Some(before), Some(after));
        assert!(shown.contains(&"-   \"d\": 4,".to_owned()), "{shown:?}");
        assert!(shown.contains(&"+   \"d\": 40,".to_owned()), "{shown:?}");
        assert!(
            shown.contains(&"    \"c\": 3,".to_owned()),
            "context kept: {shown:?}"
        );
        assert!(
            !shown.iter().any(|line| line.contains("\"a\"")),
            "{shown:?}"
        );
        assert_eq!(shown.iter().filter(|line| *line == "  ...").count(), 2);

        let hostile = change_lines(None, Some("\u{1b}[31mred"));
        assert_eq!(hostile, vec!["+ \\u{1b}[31mred".to_owned()]);
    }
}
