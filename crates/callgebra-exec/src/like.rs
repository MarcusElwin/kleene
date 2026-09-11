//! SQL `LIKE` matching with `%` and `_`.

/// Match `text` against `pattern` (`%` any run, `_` one char). Case-sensitive.
pub(crate) fn like(text: &str, pattern: &str) -> bool {
    let t: Vec<char> = text.chars().collect();
    let p: Vec<char> = pattern.chars().collect();
    matches(&t, &p)
}

/// Case-insensitive `LIKE`.
pub(crate) fn ilike(text: &str, pattern: &str) -> bool {
    like(&text.to_lowercase(), &pattern.to_lowercase())
}

fn matches(t: &[char], p: &[char]) -> bool {
    // Iterative matcher with backtracking only at the last '%'.
    let (mut ti, mut pi) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '_' || p[pi] == t[ti]) {
            ti += 1;
            pi += 1;
        } else if pi < p.len() && p[pi] == '%' {
            star = Some((pi, ti));
            pi += 1;
        } else if let Some((sp, st)) = star {
            pi = sp + 1;
            ti = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '%' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn like_semantics() {
        assert!(like("hello", "h%o"));
        assert!(like("hello", "_ello"));
        assert!(!like("hello", "_llo"));
        assert!(like("", "%"));
        assert!(!like("", "_"));
        assert!(like("a%b", "a%b"));
        assert!(like("abc", "%"));
        assert!(!like("abc", "abd"));
        assert!(ilike("HeLLo", "h%O"));
        assert!(like("aaab", "%aab"));
    }
}
