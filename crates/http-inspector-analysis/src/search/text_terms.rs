use std::ops::Range;

use http_inspector_exchange::Exchange;

use super::document::{self, BodyCache, Checked};
use super::{MatchOptions, Matcher, MatcherError, SearchScope};
use crate::filter::TextTerm;

/// Compiled free-text terms. An exchange must satisfy each term.
#[derive(Debug)]
pub struct TextTerms(Vec<(Matcher, bool)>);

impl TextTerms {
    /// Compiles the terms with the current text search options.
    pub fn compile(terms: &[TextTerm], options: MatchOptions) -> Result<Self, MatcherError> {
        terms
            .iter()
            .map(|term| Matcher::new(&term.text, options).map(|matcher| (matcher, term.excluded)))
            .collect::<Result<_, _>>()
            .map(Self)
    }

    /// Wraps a single included matcher.
    #[must_use]
    pub fn single(matcher: Matcher) -> Self {
        Self(vec![(matcher, false)])
    }

    /// Checks all terms against the exchange, reusing its decoded body previews.
    #[must_use]
    pub fn check(&self, record: &Exchange, scope: SearchScope, cache: &mut BodyCache) -> Checked {
        let mut result = Checked {
            id: record.id(),
            matched: true,
            settled: document::is_settled(record),
            retry: false,
            coverage: document::Coverage::default(),
        };
        for (matcher, excluded) in &self.0 {
            let checked = document::check(record, matcher, scope, cache);
            if !checked.retry && checked.matched == *excluded {
                return Checked {
                    matched: false,
                    ..checked
                };
            }
            result.retry |= checked.retry;
            if *excluded {
                result.coverage.partial |= checked.coverage.partial;
                result.coverage.binary |= checked.coverage.binary;
                result.coverage.undecodable |= checked.coverage.undecodable;
            }
        }
        result.matched = !result.retry;
        result.settled &= !result.retry;
        result
    }

    /// Finds the non-overlapping ranges of included terms that match the text.
    #[must_use]
    pub fn find_all(&self, haystack: &[u8]) -> Vec<Range<usize>> {
        let mut ranges: Vec<_> = self
            .0
            .iter()
            .filter(|(_, excluded)| !excluded)
            .flat_map(|(matcher, _)| matcher.find_all(haystack))
            .collect();
        ranges.sort_unstable_by_key(|range| (range.start, range.end));
        let mut merged: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
        for range in ranges {
            if let Some(last) = merged.last_mut()
                && range.start < last.end
            {
                last.end = last.end.max(range.end);
            } else {
                merged.push(range);
            }
        }
        merged
    }
}

#[cfg(test)]
mod tests {
    use http::{Method, StatusCode};
    use http_inspector_exchange::{BodyLifecycle, ExchangeBuilder, ResponseFixture};

    use super::{BodyCache, Exchange, MatchOptions, SearchScope, TextTerms};
    use crate::filter::FilterQuery;

    fn terms(input: &str) -> Result<TextTerms, crate::search::MatcherError> {
        TextTerms::compile(&FilterQuery::parse(input).text, MatchOptions::default())
    }

    #[test]
    fn matches_terms_across_fields_and_bodies() -> Result<(), Box<dyn std::error::Error>> {
        let exchange: Exchange = ExchangeBuilder::http(Method::POST, "http://localhost:8080/api")?
            .request_body("alpha beta", BodyLifecycle::Complete)
            .respond(
                ResponseFixture::new(StatusCode::OK).body("cookie delta", BodyLifecycle::Complete),
            )?;
        let mut cache = BodyCache::default();
        let check = |input, cache: &mut BodyCache| {
            terms(input).map(|terms| terms.check(&exchange, SearchScope::All, cache).matched)
        };
        assert!(check(
            r#"localhost:8080 "alpha beta" delta -secret"#,
            &mut cache
        )?);
        assert!(!check("alpha -cookie", &mut cache)?);
        assert!(!check(r#""alpha delta""#, &mut cache)?);
        Ok(())
    }

    #[test]
    fn defers_negative_matches_while_the_preview_is_locked()
    -> Result<(), Box<dyn std::error::Error>> {
        let body = format!(
            "{} hidden",
            "x".repeat(http_inspector_exchange::capture::BODY_PREVIEW_BYTES)
        );
        let exchange = ExchangeBuilder::http(Method::POST, "http://localhost:8080/api")?
            .request_body(body, BodyLifecycle::Complete)
            .respond(ResponseFixture::new(StatusCode::OK))?;
        let preview = exchange.request().body().ok_or("no request body")?;
        let terms = terms("-hidden")?;
        let mut cache = BodyCache::default();
        preview.with_locked_prefix_for_test(|| {
            let result = terms.check(&exchange, SearchScope::RequestBody, &mut cache);
            assert!(result.retry);
            assert!(!result.matched);
        });
        let result = terms.check(&exchange, SearchScope::RequestBody, &mut cache);
        assert!(result.matched);
        assert!(result.coverage.partial);
        Ok(())
    }

    #[test]
    fn highlights_only_included_terms_without_duplicate_ranges()
    -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            terms("api api users -token")?.find_all(b"API users token"),
            [0..3, 4..9]
        );
        assert_eq!(terms("or order")?.find_all(b"order"), vec![0..5]);
        Ok(())
    }
}
