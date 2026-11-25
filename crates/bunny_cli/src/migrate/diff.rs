//! The unified diff a migration shows before it writes: the shape `git
//! diff` and `diff -u` print, three lines of context, hunks that touch
//! or nearly touch joined into one.

/// Lines of context around a change.
const CONTEXT: usize = 3;

/// One line of the edit script.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Same(usize, usize),
    Delete(usize),
    Insert(usize),
}

/// The unified diff of `before` into `after`, labelled `from` and `to`;
/// empty when they are equal.
pub fn unified(before: &str, after: &str, from: &str, to: &str) -> String {
    if before == after {
        return String::new();
    }
    let old: Vec<&str> = before.split_inclusive('\n').collect();
    let new: Vec<&str> = after.split_inclusive('\n').collect();
    let ops = script(&old, &new);
    // how many lines of each side come before each op
    let mut before = Vec::with_capacity(ops.len() + 1);
    let (mut a, mut b) = (0, 0);
    for op in &ops {
        before.push((a, b));
        match op {
            Op::Same(..) => {
                a += 1;
                b += 1;
            }
            Op::Delete(_) => a += 1,
            Op::Insert(_) => b += 1,
        }
    }
    let mut out = format!("--- {from}\n+++ {to}\n");
    for (first, end) in hunks(&ops) {
        let ops = &ops[first..end];
        let old_count = ops.iter().filter(|op| !matches!(op, Op::Insert(_))).count();
        let new_count = ops.iter().filter(|op| !matches!(op, Op::Delete(_))).count();
        let (old_before, new_before) = before[first];
        out.push_str(&format!("@@ -{} +{} @@\n", range(old_before, old_count), range(new_before, new_count)));
        for op in ops {
            let (mark, line) = match *op {
                Op::Same(a, _) => (' ', old[a]),
                Op::Delete(a) => ('-', old[a]),
                Op::Insert(b) => ('+', new[b]),
            };
            out.push(mark);
            out.push_str(line);
            if !line.ends_with('\n') {
                out.push_str("\n\\ No newline at end of file\n");
            }
        }
    }
    out
}

/// `start,count` as `diff -u` writes it: 1-based, `,count` left out
/// for one line, and an empty range named by the line before it.
fn range(before: usize, count: usize) -> String {
    match count {
        0 => format!("{before},0"),
        1 => format!("{}", before + 1),
        _ => format!("{},{count}", before + 1),
    }
}

/// The hunks of `ops`: ranges of ops around the changes, with context,
/// changes closer than twice the context joined.
fn hunks(ops: &[Op]) -> Vec<(usize, usize)> {
    let changed: Vec<usize> = ops.iter().enumerate().filter(|(_, op)| !matches!(op, Op::Same(..))).map(|(i, _)| i).collect();
    let mut out: Vec<(usize, usize)> = Vec::new();
    for i in changed {
        let start = i.saturating_sub(CONTEXT);
        let end = (i + 1 + CONTEXT).min(ops.len());
        match out.last_mut() {
            Some(last) if start <= last.1 => last.1 = end,
            _ => out.push((start, end)),
        }
    }
    out
}

/// The shortest edit script from `old` to `new`: Myers' O(ND) greedy
/// walk, the frontier of every round kept to read the path back.
fn script(old: &[&str], new: &[&str]) -> Vec<Op> {
    let (n, m) = (old.len() as isize, new.len() as isize);
    let max = n + m;
    let at = |k: isize| (k + max + 1) as usize;
    let mut v = vec![0isize; 2 * max as usize + 3];
    let mut trace: Vec<Vec<isize>> = Vec::new();
    'search: for d in 0..=max {
        trace.push(v.clone());
        for k in (-d..=d).step_by(2) {
            let mut x = if k == -d || (k != d && v[at(k - 1)] < v[at(k + 1)]) { v[at(k + 1)] } else { v[at(k - 1)] + 1 };
            let mut y = x - k;
            while x < n && y < m && old[x as usize] == new[y as usize] {
                x += 1;
                y += 1;
            }
            v[at(k)] = x;
            if x >= n && y >= m {
                break 'search;
            }
        }
    }
    let mut ops = Vec::new();
    let (mut x, mut y) = (n, m);
    for (d, v) in trace.iter().enumerate().rev() {
        let d = d as isize;
        let k = x - y;
        let previous_k = if k == -d || (k != d && v[at(k - 1)] < v[at(k + 1)]) { k + 1 } else { k - 1 };
        let previous_x = v[at(previous_k)];
        let previous_y = previous_x - previous_k;
        while x > previous_x && y > previous_y {
            x -= 1;
            y -= 1;
            ops.push(Op::Same(x as usize, y as usize));
        }
        if d > 0 {
            if x == previous_x {
                y -= 1;
                ops.push(Op::Insert(y as usize));
            } else {
                x -= 1;
                ops.push(Op::Delete(x as usize));
            }
        }
        x = previous_x;
        y = previous_y;
    }
    ops.reverse();
    ops
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_script_is_shortest_and_ordered() {
        let old = ["a\n", "b\n", "c\n"];
        let new = ["a\n", "x\n", "c\n", "d\n"];
        assert_eq!(script(&old, &new), [Op::Same(0, 0), Op::Delete(1), Op::Insert(1), Op::Same(2, 2), Op::Insert(3)]);
        assert_eq!(script(&[], &["a\n"]), [Op::Insert(0)]);
        assert_eq!(script(&["a\n"], &[]), [Op::Delete(0)]);
    }

    #[test]
    fn an_empty_side_is_named_by_the_line_before() {
        assert_eq!(unified("", "a\n", "a", "b"), "--- a\n+++ b\n@@ -0,0 +1 @@\n+a\n");
        assert_eq!(unified("a\nb\n", "a\n", "a", "b"), "--- a\n+++ b\n@@ -1,2 +1 @@\n a\n-b\n");
    }
}
