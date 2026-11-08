//! Line-by-line comparison of two texts, paired into rows for a side-by-side view.

/// Longest text compared, in lines; a longer one is not diffed at all.
const MAX_LINES: usize = 2000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    Same,
    /// Both sides have a line here and the text differs.
    Changed,
    /// Only the left side has this line.
    Removed,
    /// Only the right side has this line.
    Added,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub kind: RowKind,
    /// Line number (from 1) and text on the left.
    pub left: Option<(usize, String)>,
    pub right: Option<(usize, String)>,
}

enum Step {
    Equal(usize, usize),
    Left(usize),
    Right(usize),
}

/// Compare `left` with `right`; `None` when either is longer than 2000 lines.
pub fn side_by_side(left: &str, right: &str) -> Option<Vec<Row>> {
    let a: Vec<&str> = left.lines().collect();
    let b: Vec<&str> = right.lines().collect();
    if a.len() > MAX_LINES || b.len() > MAX_LINES {
        return None;
    }
    let prefix = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let suffix = a[prefix..]
        .iter()
        .rev()
        .zip(b[prefix..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (n, m) = (a.len() - prefix - suffix, b.len() - prefix - suffix);
    let (ma, mb) = (&a[prefix..prefix + n], &b[prefix..prefix + m]);
    // longest[i][j]: length of the longest common subsequence of ma[i..] and mb[j..].
    let mut longest = vec![0u16; (n + 1) * (m + 1)];
    let at = |i: usize, j: usize| i * (m + 1) + j;
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            longest[at(i, j)] = if ma[i] == mb[j] {
                longest[at(i + 1, j + 1)] + 1
            } else {
                longest[at(i + 1, j)].max(longest[at(i, j + 1)])
            };
        }
    }
    let mut steps: Vec<Step> = (0..prefix).map(|i| Step::Equal(i, i)).collect();
    let (mut i, mut j) = (0, 0);
    while i < n || j < m {
        if i < n && j < m && ma[i] == mb[j] {
            steps.push(Step::Equal(prefix + i, prefix + j));
            i += 1;
            j += 1;
        } else if j == m || (i < n && longest[at(i + 1, j)] >= longest[at(i, j + 1)]) {
            steps.push(Step::Left(prefix + i));
            i += 1;
        } else {
            steps.push(Step::Right(prefix + j));
            j += 1;
        }
    }
    steps.extend((0..suffix).map(|k| Step::Equal(a.len() - suffix + k, b.len() - suffix + k)));

    let line = |lines: &[&str], index: usize| Some((index + 1, lines[index].to_owned()));
    let mut rows = Vec::new();
    let (mut removed, mut added): (Vec<usize>, Vec<usize>) = (Vec::new(), Vec::new());
    let flush = |rows: &mut Vec<Row>, removed: &mut Vec<usize>, added: &mut Vec<usize>| {
        for k in 0..removed.len().max(added.len()) {
            let (left, right) = (removed.get(k).copied(), added.get(k).copied());
            rows.push(Row {
                kind: match (left, right) {
                    (Some(_), Some(_)) => RowKind::Changed,
                    (Some(_), None) => RowKind::Removed,
                    _ => RowKind::Added,
                },
                left: left.and_then(|index| line(&a, index)),
                right: right.and_then(|index| line(&b, index)),
            });
        }
        removed.clear();
        added.clear();
    };
    for step in steps {
        match step {
            Step::Left(index) => removed.push(index),
            Step::Right(index) => added.push(index),
            Step::Equal(x, y) => {
                flush(&mut rows, &mut removed, &mut added);
                rows.push(Row {
                    kind: RowKind::Same,
                    left: line(&a, x),
                    right: line(&b, y),
                });
            }
        }
    }
    flush(&mut rows, &mut removed, &mut added);
    Some(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changed_removed_and_added_lines_pair_up_with_their_numbers() {
        let rows = side_by_side("one\ntwo\nthree\nfour\n", "one\n2\nthree\nfour\nfive\n").unwrap();
        let shape: Vec<_> = rows
            .iter()
            .map(|row| {
                (
                    row.kind,
                    row.left.as_ref().map(|l| l.0),
                    row.right.as_ref().map(|r| r.0),
                )
            })
            .collect();
        assert_eq!(
            shape,
            [
                (RowKind::Same, Some(1), Some(1)),
                (RowKind::Changed, Some(2), Some(2)),
                (RowKind::Same, Some(3), Some(3)),
                (RowKind::Same, Some(4), Some(4)),
                (RowKind::Added, None, Some(5)),
            ]
        );
        assert_eq!(rows[1].left.as_ref().unwrap().1, "two");
        assert_eq!(rows[1].right.as_ref().unwrap().1, "2");
        assert!(side_by_side(&"x\n".repeat(2001), "").is_none());
    }
}
