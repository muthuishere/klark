//! What is new in a poll run: `--diff lines` and `--diff unified`.

use std::collections::HashMap;

/// The lines of `curr` that `prev` does not account for, in output order.
/// Duplicates count: a line is new when `curr` has more of it than `prev`.
/// Removed and reordered lines send nothing.
pub fn lines(prev: &str, curr: &str) -> Vec<String> {
    let mut seen: HashMap<&str, usize> = HashMap::new();
    for l in prev.lines() {
        *seen.entry(l).or_default() += 1;
    }
    let mut out = Vec::new();
    for l in curr.lines() {
        match seen.get_mut(l) {
            Some(n) if *n > 0 => *n -= 1,
            _ => out.push(l.to_string()),
        }
    }
    out
}

const CONTEXT: usize = 3;
/// Above this many cells the LCS table is not built; the diff becomes
/// "everything removed, everything added".
const MAX_CELLS: usize = 4_000_000;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Op {
    Same,
    Del,
    Add,
}

/// A unified diff of `prev` and `curr`, or `None` when they are equal.
/// `prev = None` is the first run: it only records, so nothing is sent.
pub fn unified(prev: Option<&str>, curr: &str) -> Option<String> {
    let prev = prev?;
    if prev == curr {
        return None;
    }
    let a: Vec<&str> = prev.lines().collect();
    let b: Vec<&str> = curr.lines().collect();
    let ops = script(&a, &b);
    if ops.iter().all(|(op, _, _)| *op == Op::Same) {
        return None; // differs only in a trailing newline
    }
    Some(render(&a, &b, &ops))
}

/// Edit script as (op, index in a, index in b).
fn script(a: &[&str], b: &[&str]) -> Vec<(Op, usize, usize)> {
    let (n, m) = (a.len(), b.len());
    if n.saturating_mul(m) > MAX_CELLS {
        let mut ops: Vec<_> = (0..n).map(|i| (Op::Del, i, 0)).collect();
        ops.extend((0..m).map(|j| (Op::Add, n, j)));
        return ops;
    }
    // lcs[i][j] = LCS length of a[i..], b[j..]
    let mut lcs = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let (mut i, mut j, mut ops) = (0, 0, Vec::new());
    while i < n || j < m {
        if i < n && j < m && a[i] == b[j] {
            ops.push((Op::Same, i, j));
            i += 1;
            j += 1;
        } else if j < m && (i == n || lcs[i][j + 1] >= lcs[i + 1][j]) {
            ops.push((Op::Add, i, j));
            j += 1;
        } else {
            ops.push((Op::Del, i, j));
            i += 1;
        }
    }
    ops
}

fn render(a: &[&str], b: &[&str], ops: &[(Op, usize, usize)]) -> String {
    let mut out = String::from("--- previous\n+++ current\n");
    let changed: Vec<usize> = (0..ops.len()).filter(|k| ops[*k].0 != Op::Same).collect();
    let mut k = 0;
    while k < changed.len() {
        let start = changed[k].saturating_sub(CONTEXT);
        let mut end = changed[k];
        while k < changed.len() && changed[k] <= end + 2 * CONTEXT + 1 {
            end = changed[k];
            k += 1;
        }
        let end = (end + CONTEXT + 1).min(ops.len());
        let hunk = &ops[start..end];
        let a_start = hunk[0].1;
        let b_start = hunk[0].2;
        let a_len = hunk.iter().filter(|o| o.0 != Op::Add).count();
        let b_len = hunk.iter().filter(|o| o.0 != Op::Del).count();
        out.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            a_start + usize::from(a_len > 0),
            a_len,
            b_start + usize::from(b_len > 0),
            b_len
        ));
        for (op, i, j) in hunk {
            match op {
                Op::Same => out.push_str(&format!(" {}\n", a[*i])),
                Op::Del => out.push_str(&format!("-{}\n", a[*i])),
                Op::Add => out.push_str(&format!("+{}\n", b[*j])),
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_output_sends_nothing() {
        assert!(lines("a\nb\n", "a\nb\n").is_empty());
    }

    #[test]
    fn one_new_line_among_old_ones() {
        assert_eq!(lines("a\nb\n", "a\nnew\nb\n"), vec!["new"]);
    }

    #[test]
    fn removed_line_sends_nothing() {
        assert!(lines("a\nb\nc\n", "a\nc\n").is_empty());
    }

    #[test]
    fn second_identical_line_is_sent() {
        assert_eq!(lines("x\n", "x\nx\n"), vec!["x"]);
    }

    #[test]
    fn reordered_output_sends_nothing() {
        assert!(lines("a\nb\nc\n", "c\na\nb\n").is_empty());
    }

    #[test]
    fn first_run_sends_everything() {
        assert_eq!(lines("", "a\nb\n"), vec!["a", "b"]);
    }

    #[test]
    fn unified_first_run_records_and_sends_nothing() {
        assert_eq!(unified(None, "a\n"), None);
    }

    #[test]
    fn unified_equal_sends_nothing() {
        assert_eq!(unified(Some("a\nb\n"), "a\nb\n"), None);
        assert_eq!(unified(Some("a\nb"), "a\nb\n"), None);
    }

    #[test]
    fn unified_changed_line_shows_removed_and_added() {
        let prev = "l1\nl2\nl3\nport 80\nl5\nl6\nl7\nl8\n";
        let curr = "l1\nl2\nl3\nport 8080\nl5\nl6\nl7\nl8\n";
        let d = unified(Some(prev), curr).unwrap();
        assert!(d.contains("\n-port 80\n"), "{d}");
        assert!(d.contains("\n+port 8080\n"), "{d}");
        assert!(d.contains("@@ -1,7 +1,7 @@"), "{d}");
        assert!(!d.contains("l8"), "context is 3 lines: {d}");
    }

    #[test]
    fn unified_far_apart_changes_make_two_hunks() {
        let prev: String = (0..30).map(|i| format!("{i}\n")).collect();
        let curr: String = (0..30)
            .map(|i| match i {
                2 => "two\n".to_string(),
                25 => "twenty-five\n".to_string(),
                _ => format!("{i}\n"),
            })
            .collect();
        let d = unified(Some(&prev), &curr).unwrap();
        assert_eq!(d.matches("@@ ").count(), 2, "{d}");
    }
}
