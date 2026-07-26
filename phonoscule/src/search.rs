//! Fuzzy text ranking, for searching a library by typing at it.
//!
//! Plain text in, a rank out: what a browser filters album titles by, and a picker its genres and
//! artists. Nothing here knows about albums.

/// Ranks `candidate` against `query`: `None` unless it contains every whitespace-split word of the
/// query (case-insensitively); otherwise the length of the longest common substring with the whole
/// query, so contiguous hits ("dark side" as a phrase) outrank scattered ones. An empty query matches
/// everything, at rank 0.
pub fn rank(candidate: &str, query: &str) -> Option<usize> {
    let query = query.to_lowercase();
    if query.split_whitespace().next().is_none() {
        return Some(0);
    }
    let candidate = candidate.to_lowercase();
    query.split_whitespace().all(|word| candidate.contains(word)).then(|| longest_common_substring(&candidate, &query))
}

/// The length in bytes of the longest common substring of `a` and `b`: the classic quadratic table,
/// one rolling row. Both inputs are short -- titles and queries -- so this is microseconds.
fn longest_common_substring(a: &str, b: &str) -> usize {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut row = vec![0usize; b.len() + 1];
    let mut best = 0;
    for &ca in a {
        // Walk right-to-left so `row[j - 1]` still holds the previous row's value.
        for j in (1..=b.len()).rev() {
            row[j] = if ca == b[j - 1] { row[j - 1] + 1 } else { 0 };
            best = best.max(row[j]);
        }
    }
    best
}

/// The candidates `query` matches, best first. Ties keep the order they came in, so a caller's own
/// ordering survives where the search has nothing to say.
pub fn matches<T: AsRef<str>>(candidates: impl IntoIterator<Item = T>, query: &str) -> Vec<T> {
    let mut scored: Vec<(T, usize)> = candidates
        .into_iter()
        .filter_map(|value| {
            let rank = rank(value.as_ref(), query)?;
            Some((value, rank))
        })
        .collect();
    scored.sort_by(|(_, a), (_, b)| b.cmp(a));
    scored.into_iter().map(|(value, _)| value).collect()
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn ranks_contiguous_matches_higher() {
        assert_eq!(rank("The Dark Side of the Moon", ""), Some(0), "an empty query matches everything");
        assert_eq!(rank("The Dark Side of the Moon", "dark side"), Some(9), "a phrase hit scores its full length");
        assert_eq!(rank("Darkness on the Far Side", "dark side"), Some(5), "scattered words score the longest run");
        assert_eq!(rank("The Wall", "dark side"), None, "every word must be contained");
        assert_eq!(rank("MONO no aware", "mono"), Some(4), "matching is case-insensitive");
    }

    /// Best first, and candidates the query rules out are gone rather than ranked last.
    #[test]
    fn matches_are_ordered_best_first() {
        let candidates = ["The Wall", "Darkness on the Far Side", "The Dark Side of the Moon"];
        assert_eq!(matches(candidates, "dark side"), ["The Dark Side of the Moon", "Darkness on the Far Side"]);
    }

    /// An empty query keeps everything, in the order it arrived: the caller's sort is not disturbed
    /// by a search that says nothing.
    #[test]
    fn an_empty_query_keeps_the_given_order() {
        let candidates = ["Zoo", "Apple", "Mango"];
        assert_eq!(matches(candidates, ""), candidates);
    }
}
