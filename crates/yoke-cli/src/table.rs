//! Shared human record tables. JSON serializers never pass through this module.

use std::io::IsTerminal;
use unicode_width::UnicodeWidthStr;

/// Header, minimum readable width, maximum preview width, in terminal cells.
pub type Column<'a> = (&'a str, usize, usize);

/// Keep filenames useful in bounded cells by removing known project/home prefixes.
pub fn path(path: &std::path::Path, project: &std::path::Path, home: &std::path::Path) -> String {
    for (root, prefix) in [(project, "."), (home, "~")] {
        if root.is_absolute() {
            if let Ok(relative) = path.strip_prefix(root) {
                return if relative.as_os_str().is_empty() {
                    prefix.into()
                } else {
                    format!("{prefix}/{}", relative.display())
                };
            }
        }
    }
    path.display().to_string()
}

fn width() -> usize {
    if let Some(width) = std::env::var("COLUMNS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|n| (1..=4096).contains(n))
    {
        return width;
    }
    if std::io::stdout().is_terminal() {
        let mut size = std::mem::MaybeUninit::<libc::winsize>::zeroed();
        // SAFETY: ioctl writes to a valid winsize; inspect it only on success.
        if unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, size.as_mut_ptr()) } == 0 {
            let columns = usize::from(unsafe { size.assume_init() }.ws_col);
            if columns > 0 {
                return columns.min(4096);
            }
        }
    }
    120
}

/// Flatten stored text and neutralize terminal controls before sizing or printing it.
fn clean(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_control() || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}') {
                ' '
            } else {
                c
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn clip(text: &str, width: usize, ascii: bool) -> String {
    if text.width() <= width {
        return text.to_owned();
    }
    let mark = if ascii {
        &"..."[..width.min(3)]
    } else if width > 0 {
        "…"
    } else {
        ""
    };
    let room = width.saturating_sub(mark.width());
    let end = text
        .char_indices()
        .find_map(|(at, c)| (text[..at + c.len_utf8()].width() > room).then_some(at))
        .unwrap_or(text.len());
    format!("{}{mark}", &text[..end])
}

/// A standalone preview follows the tables' single-line, ellipsis and display-width rules.
pub fn preview(text: &str, width: usize) -> String {
    clip(
        &clean(text),
        width,
        std::env::var("YOKE_ASCII").as_deref() == Ok("1"),
    )
}

/// Render aligned columns; when their minimum widths cannot fit, show labeled records.
pub fn render(name: &str, columns: &[Column<'_>], rows: &[Vec<String>]) -> String {
    render_at(
        name,
        columns,
        rows,
        width(),
        std::env::var("YOKE_ASCII").as_deref() == Ok("1"),
    )
}

fn render_at(
    name: &str,
    columns: &[Column<'_>],
    rows: &[Vec<String>],
    width: usize,
    ascii: bool,
) -> String {
    let mut text = clip(
        &format!(
            "{name}: {}{}",
            rows.len(),
            if rows.is_empty() { " (none)" } else { "" }
        ),
        width,
        ascii,
    ) + "\n";
    if rows.is_empty() {
        return text;
    }
    assert!(!columns.is_empty() && rows.iter().all(|row| row.len() == columns.len()));
    let rows: Vec<Vec<_>> = rows
        .iter()
        .map(|row| {
            row.iter()
                .map(|cell| {
                    let cell = clean(cell);
                    if cell.is_empty() {
                        "-".into()
                    } else {
                        cell
                    }
                })
                .collect()
        })
        .collect();
    let minima: Vec<_> = columns
        .iter()
        .map(|(label, min, _)| (*min).max(label.width()))
        .collect();
    let mut widths: Vec<_> = columns
        .iter()
        .enumerate()
        .map(|(i, (_, _, max))| {
            rows.iter()
                .map(|row| row[i].width())
                .max()
                .unwrap_or(0)
                .min(*max)
                .max(minima[i])
        })
        .collect();
    let separators = 2 * columns.len(); // indent plus two spaces between columns
    let mut shortened = false;
    if minima.iter().sum::<usize>() + separators > width {
        for (index, row) in rows.iter().enumerate() {
            text.push_str(&clip(&format!("  [{}]", index + 1), width, ascii));
            text.push('\n');
            for ((label, _, _), value) in columns.iter().zip(row) {
                let line = format!("  {label}: {value}");
                shortened |= line.width() > width;
                text.push_str(&clip(&line, width, ascii));
                text.push('\n');
            }
        }
    } else {
        while widths.iter().sum::<usize>() + separators > width {
            let i = widths
                .iter()
                .enumerate()
                .filter(|(i, w)| **w > minima[*i])
                .max_by_key(|(_, w)| **w)
                .map(|(i, _)| i)
                .expect("minimum widths fit");
            widths[i] -= 1;
        }
        let headers = columns
            .iter()
            .map(|(label, _, _)| (*label).to_owned())
            .collect::<Vec<_>>();
        for row in std::iter::once(&headers).chain(&rows) {
            text.push_str("  ");
            for (i, (cell, width)) in row.iter().zip(&widths).enumerate() {
                shortened |= cell.width() > *width;
                let value = clip(cell, *width, ascii);
                text.push_str(&value);
                if i + 1 < columns.len() {
                    text.push_str(&" ".repeat(width.saturating_sub(value.width()) + 2));
                }
            }
            text.push('\n');
        }
    }
    if shortened {
        text.push_str(&clip(
            if ascii {
                "  ... shortened; full records: --json"
            } else {
                "  … shortened; full records: --json"
            },
            width,
            ascii,
        ));
        text.push('\n');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn tables_align_columns_and_keep_values_on_one_line() {
        let text = render_at(
            "items",
            &[("ID", 2, 8), ("DETAIL", 6, 30)],
            &[
                vec!["1".into(), "first\nline\tvalue".into()],
                vec!["22".into(), "界e\u{301}".into()],
            ],
            80,
            false,
        );
        assert_eq!(
            text,
            "items: 2\n  ID  DETAIL\n  1   first line value\n  22  界e\u{301}\n"
        );
        assert!(!text.contains('"'));
    }

    #[test]
    fn narrow_tables_preserve_fields_and_bound_unicode_and_control_text() {
        let rows = vec![vec![
            "2026-10-25 09:12:03".into(),
            "界🙂e\u{301}\n\x1b[31m\rtext\u{202e}".repeat(20),
        ]];
        for width in [1, 5, 20, 40, 80, 120] {
            for ascii in [false, true] {
                let text = render_at(
                    "records",
                    &[("TIME", 19, 19), ("DETAIL", 6, 180)],
                    &rows,
                    width,
                    ascii,
                );
                assert!(
                    text.lines()
                        .all(|line| UnicodeWidthStr::width(line) <= width),
                    "{width}: {text}"
                );
                assert!(
                    !text.contains('\x1b') && !text.contains('\r') && !text.contains('\u{202e}')
                );
                if width == 40 {
                    assert!(text.contains("2026-10-25 09:12:03"), "{text}");
                    assert!(text.contains(if ascii { "..." } else { "…" }), "{text}");
                }
                if width == 20 {
                    assert!(text.contains("TIME:") && text.contains("DETAIL:"), "{text}");
                }
            }
        }
    }

    #[test]
    fn empty_and_missing_values_are_explicit() {
        assert_eq!(
            render_at("items", &[("VALUE", 5, 10)], &[], 80, false),
            "items: 0 (none)\n"
        );
        assert!(render_at(
            "items",
            &[("VALUE", 5, 10)],
            &[vec![String::new()]],
            80,
            false
        )
        .contains("  -\n"));
        assert_eq!(preview("  alpha\n beta  ", 10), "alpha beta");
        assert!(matches!(preview("abcdef", 4).as_str(), "abc…" | "a..."));
        let project = std::path::Path::new("/home/u/project");
        let home = std::path::Path::new("/home/u");
        for (input, expected) in [
            ("/home/u/project/check.sh", "./check.sh"),
            ("/home/u/.claude/settings.json", "~/.claude/settings.json"),
            ("/home/user/check.sh", "/home/user/check.sh"),
            ("check.sh", "check.sh"),
        ] {
            assert_eq!(path(std::path::Path::new(input), project, home), expected);
        }
    }
}
