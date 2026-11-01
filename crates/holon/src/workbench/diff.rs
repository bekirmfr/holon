//! Two texts line by line: what a deploy changes in a flow's text.

/// A line of a diff: kept (' '), taken out ('-') or put in ('+').
pub(crate) type Line = (char, String);

/// `then` to `now`, by the longest run of lines they share.
pub(crate) fn lines(then: &str, now: &str) -> Vec<Line> {
    let (a, b): (Vec<&str>, Vec<&str>) = (then.lines().collect(), now.lines().collect());
    let (n, m) = (a.len(), b.len());
    let mut t = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            t[i][j] = if a[i] == b[j] { t[i + 1][j + 1] + 1 } else { t[i + 1][j].max(t[i][j + 1]) };
        }
    }
    let (mut out, mut i, mut j) = (vec![], 0, 0);
    while i < n && j < m {
        if a[i] == b[j] {
            out.push((' ', a[i].to_string()));
            (i, j) = (i + 1, j + 1);
        } else if t[i + 1][j] >= t[i][j + 1] {
            out.push(('-', a[i].to_string()));
            i += 1;
        } else {
            out.push(('+', b[j].to_string()));
            j += 1;
        }
    }
    out.extend(a[i..].iter().map(|l| ('-', l.to_string())));
    out.extend(b[j..].iter().map(|l| ('+', l.to_string())));
    out
}

/// How many lines it puts in and takes out.
pub(crate) fn counts(d: &[Line]) -> (usize, usize) {
    (d.iter().filter(|(k, _)| *k == '+').count(), d.iter().filter(|(k, _)| *k == '-').count())
}

/// The changed lines with `around` kept lines either side; a gap between is one `…` line.
pub(crate) fn hunks(d: &[Line], around: usize) -> Vec<Line> {
    let near: Vec<bool> = (0..d.len()).map(|i| d[i.saturating_sub(around)..(i + around + 1).min(d.len())].iter().any(|(k, _)| *k != ' ')).collect();
    let mut out = vec![];
    for (i, l) in d.iter().enumerate() {
        if near[i] {
            out.push(l.clone());
        } else if i > 0 && near[i - 1] {
            out.push(('…', String::new()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_diff_keeps_what_both_share_and_marks_the_rest() {
        let d = lines("a\nb\nc\nd\n", "a\nc\nd\ne\n");
        assert_eq!(d, [(' ', "a".into()), ('-', "b".into()), (' ', "c".into()), (' ', "d".into()), ('+', "e".into())]);
        assert_eq!(counts(&d), (1, 1));
        let long = lines(&"x\n".repeat(10), &format!("{}y\n", "x\n".repeat(10)));
        assert_eq!(hunks(&long, 1), [(' ', "x".into()), ('+', "y".into())]);
    }
}
