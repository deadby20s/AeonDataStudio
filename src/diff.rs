//! Word-level diff (Myers' O(ND) algorithm) for the Inspector's before/after and duplicate views.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Equal,
    Delete,
    Insert,
}

/// Splits text into words and single whitespace characters, so a diff shows word changes
/// and the smallest whitespace changes (e.g. "two of four newlines removed").
pub fn tokenize(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut word_start: Option<usize> = None;
    for (i, c) in s.char_indices() {
        if c.is_whitespace() {
            if let Some(start) = word_start.take() {
                out.push(&s[start..i]);
            }
            out.push(&s[i..i + c.len_utf8()]);
        } else if word_start.is_none() {
            word_start = Some(i);
        }
    }
    if let Some(start) = word_start {
        out.push(&s[start..]);
    }
    out
}

/// Diff of two token lists, or `None` when they differ in more than `max_edits` tokens.
/// Consecutive tokens with the same operation are merged.
pub fn diff<'a>(a: &[&'a str], b: &[&'a str], max_edits: usize) -> Option<Vec<(Op, String)>> {
    // Common prefix and suffix are cheap to strip and usually most of the text.
    let prefix = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let suffix = a[prefix..].iter().rev().zip(b[prefix..].iter().rev()).take_while(|(x, y)| x == y).count();
    let (am, bm) = (&a[prefix..a.len() - suffix], &b[prefix..b.len() - suffix]);

    let mut ops: Vec<(Op, &str)> = a[..prefix].iter().map(|t| (Op::Equal, *t)).collect();
    ops.extend(myers(am, bm, max_edits)?);
    ops.extend(a[a.len() - suffix..].iter().map(|t| (Op::Equal, *t)));

    let mut merged: Vec<(Op, String)> = Vec::new();
    for (op, tok) in ops {
        match merged.last_mut() {
            Some((last, text)) if *last == op => text.push_str(tok),
            _ => merged.push((op, tok.to_string())),
        }
    }
    Some(merged)
}

fn myers<'a>(a: &[&'a str], b: &[&'a str], max_d: usize) -> Option<Vec<(Op, &'a str)>> {
    let (n, m) = (a.len() as isize, b.len() as isize);
    let max_d = max_d.min((n + m) as usize);
    let offset = max_d as isize + 1;
    let idx = |k: isize| (k + offset) as usize;
    let mut v = vec![0isize; 2 * max_d + 3];
    let mut trace: Vec<Vec<isize>> = Vec::new();

    for d in 0..=max_d as isize {
        trace.push(v.clone());
        let mut k = -d;
        while k <= d {
            let mut x = if k == -d || (k != d && v[idx(k - 1)] < v[idx(k + 1)]) { v[idx(k + 1)] } else { v[idx(k - 1)] + 1 };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[idx(k)] = x;
            if x >= n && y >= m {
                return Some(backtrack(a, b, &trace, d, idx));
            }
            k += 2;
        }
    }
    None
}

fn backtrack<'a>(
    a: &[&'a str],
    b: &[&'a str],
    trace: &[Vec<isize>],
    d_final: isize,
    idx: impl Fn(isize) -> usize,
) -> Vec<(Op, &'a str)> {
    let (mut x, mut y) = (a.len() as isize, b.len() as isize);
    let mut ops = Vec::new();
    for d in (0..=d_final).rev() {
        let v = &trace[d as usize];
        let k = x - y;
        let prev_k = if k == -d || (k != d && v[idx(k - 1)] < v[idx(k + 1)]) { k + 1 } else { k - 1 };
        let prev_x = v[idx(prev_k)];
        let prev_y = prev_x - prev_k;
        while x > prev_x && y > prev_y {
            ops.push((Op::Equal, a[(x - 1) as usize]));
            x -= 1;
            y -= 1;
        }
        if d > 0 {
            if x == prev_x {
                ops.push((Op::Insert, b[(y - 1) as usize]));
                y -= 1;
            } else {
                ops.push((Op::Delete, a[(x - 1) as usize]));
                x -= 1;
            }
        }
    }
    ops.reverse();
    ops
}

/// Makes whitespace visible inside changed spans.
pub fn show_whitespace(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            ' ' => "·".to_string(),
            '\t' => "⇥".to_string(),
            '\n' => "⏎\n".to_string(),
            '\r' => "␍".to_string(),
            '\u{00A0}' => "⍽".to_string(),
            c if c.is_whitespace() => "␣".to_string(),
            c => c.to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(ops: &[(Op, String)]) -> (String, String) {
        let before = ops.iter().filter(|(o, _)| *o != Op::Insert).map(|(_, t)| t.as_str()).collect();
        let after = ops.iter().filter(|(o, _)| *o != Op::Delete).map(|(_, t)| t.as_str()).collect();
        (before, after)
    }

    #[test]
    fn tokens_alternate_words_and_spaces() {
        assert_eq!(tokenize("a  bc\nd"), vec!["a", " ", " ", "bc", "\n", "d"]);
        assert_eq!(tokenize(""), Vec::<&str>::new());
    }

    #[test]
    fn diff_reconstructs_both_sides_and_marks_changes() {
        for (a, b) in [
            ("the quick brown fox", "the quick red fox"),
            ("a b c", "a b c d e"),
            ("x y z", "z"),
            ("same", "same"),
            ("", "new text"),
            ("Too    many  spaces", "Too many spaces"),
        ] {
            let (ta, tb) = (tokenize(a), tokenize(b));
            let ops = diff(&ta, &tb, 1000).unwrap();
            assert_eq!(apply(&ops), (a.to_string(), b.to_string()), "{a:?} → {b:?}");
        }
        let ops = diff(&tokenize("the quick brown fox"), &tokenize("the quick red fox"), 100).unwrap();
        assert!(ops.contains(&(Op::Delete, "brown".into())) && ops.contains(&(Op::Insert, "red".into())));
    }

    #[test]
    fn gives_up_beyond_max_edits() {
        let a: Vec<String> = (0..100).map(|i| format!("a{i}")).collect();
        let b: Vec<String> = (0..100).map(|i| format!("b{i}")).collect();
        let (a, b): (Vec<&str>, Vec<&str>) = (a.iter().map(String::as_str).collect(), b.iter().map(String::as_str).collect());
        assert!(diff(&a, &b, 50).is_none());
        assert!(diff(&a, &b, 200).is_some());
    }
}
