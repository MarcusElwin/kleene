//! Near-name suggestions for error hints.

/// Levenshtein edit distance over characters.
pub(crate) fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// Up to three candidates close to `name`, rendered as a hint, or `None`.
pub(crate) fn suggest<'a>(name: &str, candidates: impl Iterator<Item = &'a str>) -> Option<String> {
    let lower = name.to_ascii_lowercase();
    let mut scored: Vec<(usize, &str)> = candidates
        .map(|c| (edit_distance(&lower, &c.to_ascii_lowercase()), c))
        .filter(|(d, c)| *d <= 3 || c.to_ascii_lowercase().contains(&lower))
        .collect();
    scored.sort();
    scored.dedup_by(|a, b| a.1 == b.1);
    if scored.is_empty() {
        return None;
    }
    let names: Vec<String> = scored.iter().take(3).map(|(_, c)| c.to_string()).collect();
    Some(format!("did you mean {}?", names.join(", ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_and_suggestions() {
        assert_eq!(edit_distance("kitten", "sitting"), 3);
        let s = suggest("uppr", ["upper", "lower", "generate_series"].into_iter()).unwrap();
        assert!(s.starts_with("did you mean upper"));
        assert!(suggest("zzzzzzzz", ["upper"].into_iter()).is_none());
    }
}
