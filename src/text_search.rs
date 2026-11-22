//! Byte search for one user query, prepared once and reused for every
//! haystack.
//!
//! The query is compiled into the fastest matcher for its options:
//!
//! - case-sensitive text uses [`memchr::memmem::Finder`], a Two-Way search
//!   with a SIMD prefilter;
//! - case-insensitive text lowercases ASCII letters of the haystack in
//!   8 KiB blocks of a reused per-thread buffer and searches them with a
//!   `Finder` of the lowercased query, so it allocates nothing per search;
//! - case-insensitive text with non-ASCII cased letters (for example `É`)
//!   and regular expressions use a [`regex_automata::meta::Regex`], which
//!   applies Unicode simple case folding and runs in linear time.
//!
//! Haystacks are bytes, so callers do not have to convert them to UTF-8.

use std::cell::RefCell;
use std::ops::Range;

use memchr::memmem;
use regex_automata::meta::Regex;
use regex_automata::util::syntax;

/// NFA size limit for a user-written regular expression, so a pathological
/// pattern fails to compile instead of using unbounded memory.
const REGEX_SIZE_LIMIT: usize = 4 * 1024 * 1024;
/// Size of the block a case-insensitive search folds the haystack into. Longer queries fold the whole haystack at once.
const FOLD_BLOCK: usize = 8 * 1024;

/// How a query is interpreted.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct MatchOptions {
    /// Matches letter case exactly when `true`.
    pub case_sensitive: bool,
    /// Reads the query as a regular expression when `true`.
    pub regex: bool,
}

/// Why a query could not be compiled.
#[derive(Debug, thiserror::Error)]
pub enum MatcherError {
    /// The query is empty.
    #[error("the search text is empty")]
    Empty,
    /// The query is not a valid regular expression.
    #[error("invalid regular expression: {0}")]
    Regex(String),
}

/// A compiled query.
#[derive(Clone, Debug)]
pub struct Matcher {
    kind: Kind,
}

#[derive(Clone, Debug)]
enum Kind {
    Exact(memmem::Finder<'static>),
    /// Finds the ASCII-lowercased query in an ASCII-lowercased haystack.
    /// Lowercasing ASCII keeps every byte offset.
    AsciiCaseless(memmem::Finder<'static>),
    Regex(Regex),
}

impl Matcher {
    /// Compiles `query` with `options`.
    pub fn new(query: &str, options: MatchOptions) -> Result<Self, MatcherError> {
        if query.is_empty() {
            return Err(MatcherError::Empty);
        }
        let kind = if options.regex {
            Kind::Regex(regex(query, options.case_sensitive)?)
        } else if options.case_sensitive {
            Kind::Exact(memmem::Finder::new(query.as_bytes()).into_owned())
        } else if has_non_ascii_case(query) {
            Kind::Regex(regex(&escape(query), false)?)
        } else {
            Kind::AsciiCaseless(memmem::Finder::new(&query.to_ascii_lowercase()).into_owned())
        };
        Ok(Self { kind })
    }

    /// Reports whether `haystack` contains a match.
    #[must_use]
    pub fn is_match(&self, haystack: &[u8]) -> bool {
        match &self.kind {
            Kind::Exact(finder) => finder.find(haystack).is_some(),
            Kind::AsciiCaseless(finder) => folded_match(finder, haystack),
            Kind::Regex(regex) => regex.is_match(haystack),
        }
    }

    /// Returns the byte ranges of non-overlapping, non-empty matches in
    /// order.
    #[must_use]
    pub fn find_all(&self, haystack: &[u8]) -> Vec<Range<usize>> {
        let ranges: Vec<Range<usize>> = match &self.kind {
            Kind::Exact(finder) => starts(finder, haystack),
            Kind::AsciiCaseless(finder) => starts(finder, &haystack.to_ascii_lowercase()),
            Kind::Regex(regex) => regex
                .find_iter(haystack)
                .map(|found| found.range())
                .collect(),
        };
        ranges
            .into_iter()
            .filter(|range| !range.is_empty())
            .collect()
    }
}

fn starts(finder: &memmem::Finder<'_>, haystack: &[u8]) -> Vec<Range<usize>> {
    let length = finder.needle().len();
    finder
        .find_iter(haystack)
        .map(|start| start..start + length)
        .collect()
}

thread_local! {
    /// Reused for every case-insensitive search on a thread, so a search
    /// neither allocates nor clears a buffer.
    static FOLD_BUFFER: RefCell<Vec<u8>> = RefCell::new(Vec::with_capacity(FOLD_BLOCK));
}

/// Folds `haystack` block by block into a reused buffer, keeping the last
/// `needle length - 1` bytes of each block so a match across blocks is
/// found.
fn folded_match(finder: &memmem::Finder<'_>, haystack: &[u8]) -> bool {
    let overlap = finder.needle().len().saturating_sub(1);
    if overlap >= FOLD_BLOCK / 2 {
        return finder.find(&haystack.to_ascii_lowercase()).is_some();
    }
    FOLD_BUFFER.with_borrow_mut(|block| {
        block.clear();
        for chunk in haystack.chunks(FOLD_BLOCK - overlap) {
            block.extend(chunk.iter().map(|byte| fold(*byte)));
            if finder.find(block).is_some() {
                return true;
            }
            let keep = overlap.min(block.len());
            block.drain(..block.len() - keep);
        }
        false
    })
}

/// ASCII lowercase without a branch, so folding loops vectorize.
const fn fold(byte: u8) -> u8 {
    byte | (((byte.wrapping_sub(b'A') < 26) as u8) << 5)
}

fn regex(pattern: &str, case_sensitive: bool) -> Result<Regex, MatcherError> {
    Regex::builder()
        .syntax(syntax::Config::new().case_insensitive(!case_sensitive))
        .configure(
            Regex::config()
                .nfa_size_limit(Some(REGEX_SIZE_LIMIT))
                .onepass_size_limit(Some(REGEX_SIZE_LIMIT))
                .dfa_size_limit(Some(REGEX_SIZE_LIMIT)),
        )
        .build(pattern)
        .map_err(|error| MatcherError::Regex(error.to_string()))
}

/// Reports whether `query` has a letter whose case only Unicode folding
/// can ignore. Hangul and other scripts without case need no folding.
fn has_non_ascii_case(query: &str) -> bool {
    query.chars().any(|character| {
        !character.is_ascii() && (character.is_lowercase() || character.is_uppercase())
    })
}

/// Escapes regular-expression syntax so `literal` matches itself.
fn escape(literal: &str) -> String {
    let mut escaped = String::with_capacity(literal.len());
    for character in literal.chars() {
        if matches!(
            character,
            '\\' | '.'
                | '+'
                | '*'
                | '?'
                | '('
                | ')'
                | '|'
                | '['
                | ']'
                | '{'
                | '}'
                | '^'
                | '$'
                | '#'
                | '&'
                | '-'
                | '~'
        ) {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spans(matcher: &Matcher, haystack: &[u8]) -> Vec<(usize, usize)> {
        matcher
            .find_all(haystack)
            .into_iter()
            .map(|range| (range.start, range.end))
            .collect()
    }

    fn matcher(query: &str, case_sensitive: bool, regex: bool) -> Result<Matcher, MatcherError> {
        Matcher::new(
            query,
            MatchOptions {
                case_sensitive,
                regex,
            },
        )
    }

    #[test]
    fn case_options_choose_how_letters_compare() -> Result<(), MatcherError> {
        let caseless = matcher("token", false, false)?;
        assert!(caseless.is_match(b"x-TOKEN: 1"));
        let exact = matcher("token", true, false)?;
        assert!(!exact.is_match(b"x-TOKEN: 1"));
        assert!(exact.is_match(b"a token"));
        Ok(())
    }

    #[test]
    fn non_ascii_letters_fold_case_and_hangul_matches_as_is() -> Result<(), MatcherError> {
        assert!(matcher("café", false, false)?.is_match("CAFÉ".as_bytes()));
        assert!(!matcher("café", true, false)?.is_match("CAFÉ".as_bytes()));
        assert!(matcher("주문", false, false)?.is_match("새 주문 번호".as_bytes()));
        Ok(())
    }

    #[test]
    fn literal_text_with_regex_syntax_is_not_a_pattern() -> Result<(), MatcherError> {
        let literal = matcher("a.b(É)", false, false)?;
        assert!(literal.is_match("x A.B(é) y".as_bytes()));
        assert!(!literal.is_match("axb(é)".as_bytes()));
        Ok(())
    }

    #[test]
    fn regular_expressions_match_and_report_errors() -> Result<(), MatcherError> {
        let pattern = matcher(r"id=\d+", false, true)?;
        assert!(pattern.is_match(b"ID=42"));
        assert!(!pattern.is_match(b"id=x"));
        assert!(matches!(
            matcher("(", false, true),
            Err(MatcherError::Regex(_))
        ));
        assert!(matches!(
            matcher("", false, false),
            Err(MatcherError::Empty)
        ));
        Ok(())
    }

    #[test]
    fn finds_every_non_empty_match_in_order() -> Result<(), MatcherError> {
        let haystack = b"Ab ab AB";
        for matcher in [matcher("ab", false, false)?, matcher("ab", false, true)?] {
            assert_eq!(spans(&matcher, haystack), [(0, 2), (3, 5), (6, 8)]);
        }
        assert_eq!(spans(&matcher("ab", true, false)?, haystack), [(3, 5)]);
        assert_eq!(spans(&matcher("x*", false, true)?, b"axxb"), [(1, 3)]);
        Ok(())
    }

    #[test]
    fn caseless_search_finds_matches_across_fold_blocks() -> Result<(), MatcherError> {
        let needle = "Boundary-Needle";
        let caseless = matcher(needle, false, false)?;
        for start in [
            0,
            FOLD_BLOCK - 20,
            FOLD_BLOCK - 7,
            FOLD_BLOCK,
            3 * FOLD_BLOCK - 1,
        ] {
            let mut haystack = vec![b'x'; 4 * FOLD_BLOCK];
            haystack
                .get_mut(start..start + needle.len())
                .unwrap_or_default()
                .copy_from_slice(b"BOUNDARY-needle");
            assert!(caseless.is_match(&haystack), "{start}");
            assert_eq!(spans(&caseless, &haystack), [(start, start + needle.len())]);
        }
        assert!(!caseless.is_match(&vec![b'x'; 4 * FOLD_BLOCK]));
        let long = "q".repeat(FOLD_BLOCK);
        assert!(matcher(&long, false, false)?.is_match(long.to_uppercase().as_bytes()));
        Ok(())
    }

    #[test]
    fn searches_bytes_that_are_not_utf8() -> Result<(), MatcherError> {
        let haystack = b"\xff\xfeneedle\x00";
        assert!(matcher("NEEDLE", false, false)?.is_match(haystack));
        assert!(matcher("needle", true, false)?.is_match(haystack));
        assert!(matcher("ne+dle", false, true)?.is_match(haystack));
        Ok(())
    }
}
