//! Fuzzy path matching, backed by [`nucleo-matcher`] — the fzf/skim-style
//! matcher written by the Helix maintainers.
//!
//! We use its dedicated *path* matching configuration (extra bonus for matches
//! after `/`) which is exactly what deep path completion wants. A single
//! [`PathMatcher`] owns the reusable [`Matcher`] scratch buffers and the parsed
//! query, so scoring a whole directory walk is one cheap pass per candidate
//! rather than re-allocating for each one.

use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

/// A reusable fuzzy matcher for a single query, tuned for filesystem paths.
pub struct PathMatcher {
    matcher: Matcher,
    pattern: Pattern,
    buf: Vec<char>,
    empty: bool,
}

impl PathMatcher {
    /// Build a matcher for `query`. An empty query matches every candidate with
    /// a neutral score.
    pub fn new(query: &str) -> Self {
        let matcher = Matcher::new(Config::DEFAULT.match_paths());
        // `Pattern::new` (rather than `parse`) so characters common in file
        // names — `.`, `^`, `$` — are matched literally instead of being
        // treated as special operators.
        let pattern = Pattern::new(
            query,
            CaseMatching::Ignore,
            Normalization::Smart,
            AtomKind::Fuzzy,
        );
        Self {
            matcher,
            pattern,
            buf: Vec::new(),
            empty: query.trim().is_empty(),
        }
    }

    /// Whether the query was empty (so everything matches).
    pub fn is_empty(&self) -> bool {
        self.empty
    }

    /// Score `candidate` against the query. Returns `None` when it does not
    /// match; higher scores are better. An empty query yields `Some(0)`.
    pub fn score(&mut self, candidate: &str) -> Option<u32> {
        if self.empty {
            return Some(0);
        }
        let haystack = Utf32Str::new(candidate, &mut self.buf);
        self.pattern.score(haystack, &mut self.matcher)
    }
}

/// One-shot convenience wrapper (builds a throwaway [`PathMatcher`]). Prefer
/// reusing a [`PathMatcher`] when scoring many candidates.
pub fn fuzzy_match(query: &str, candidate: &str) -> Option<u32> {
    PathMatcher::new(query).score(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_matches() {
        assert_eq!(fuzzy_match("", "anything"), Some(0));
    }

    #[test]
    fn subsequence_matches_and_nonmatch() {
        assert!(fuzzy_match("k", "test/k.md").is_some());
        assert!(fuzzy_match("kmd", "k.md").is_some());
        assert!(fuzzy_match("tkmd", "test/k.md").is_some());
        assert!(fuzzy_match("xyz", "k.md").is_none());
        assert!(fuzzy_match("md", "k.m").is_none());
    }

    #[test]
    fn boundary_after_slash_ranks_higher() {
        // Path-aware config: a match right after `/` beats a mid-word match.
        let boundary = fuzzy_match("k", "test/k.md").unwrap();
        let midword = fuzzy_match("k", "broken.md").unwrap();
        assert!(boundary > midword, "{boundary} !> {midword}");
    }

    #[test]
    fn consecutive_beats_scattered() {
        let consecutive = fuzzy_match("abc", "abc.md").unwrap();
        let scattered = fuzzy_match("abc", "a1b2c3.md").unwrap();
        assert!(consecutive > scattered, "{consecutive} !> {scattered}");
    }

    #[test]
    fn case_insensitive() {
        assert!(fuzzy_match("KMD", "k.md").is_some());
        assert!(fuzzy_match("kmd", "K.MD").is_some());
    }

    #[test]
    fn reuse_matcher_across_candidates() {
        let mut m = PathMatcher::new("k");
        assert!(m.score("test/k.md").is_some());
        assert!(m.score("k.md").is_some());
        assert!(m.score("zzz.md").is_none());
    }
}
