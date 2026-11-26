//! A request filter as the user typed it, compiled once: field conditions
//! such as `method:POST status:4xx`, plus free text searched in the chosen
//! scope. The desktop filter input and the command line share it, so the
//! same text selects the same requests in both.

use http_inspector_exchange::Exchange;

use super::{FilterQuery, TextTerm};
use crate::search::{BodyCache, Checked, MatchOptions, MatcherError, SearchScope, TextTerms};

/// Free-text terms of `text`: the whole text as one term in regex mode,
/// otherwise the free-text terms of the parsed query.
#[must_use]
pub fn text_terms(text: &str, options: MatchOptions, parsed: &FilterQuery) -> Vec<TextTerm> {
    if options.regex {
        if text.is_empty() {
            Vec::new()
        } else {
            vec![TextTerm {
                text: text.to_owned(),
                excluded: false,
            }]
        }
    } else {
        parsed.text.clone()
    }
}

/// Field conditions and compiled free text of one filter.
#[derive(Debug, Default)]
pub struct RequestQuery {
    fields: FilterQuery,
    terms: Option<TextTerms>,
    scope: SearchScope,
}

impl RequestQuery {
    /// Parses `text` and compiles its free-text terms. Regex mode reads the
    /// whole text as one expression and has no field conditions. Invalid
    /// field terms are kept in [`FilterQuery::diagnostics`] and skipped.
    pub fn compile(
        text: &str,
        scope: SearchScope,
        options: MatchOptions,
    ) -> Result<Self, MatcherError> {
        let fields = if options.regex {
            FilterQuery::default()
        } else {
            FilterQuery::parse(text)
        };
        let terms = text_terms(text, options, &fields);
        let terms = if terms.is_empty() {
            None
        } else {
            Some(TextTerms::compile(&terms, options)?)
        };
        Ok(Self {
            fields,
            terms,
            scope,
        })
    }

    /// The parsed field conditions and diagnostics.
    #[must_use]
    pub const fn fields(&self) -> &FilterQuery {
        &self.fields
    }

    /// Reports whether the filter has nothing to check.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        !self.fields.has_fields() && self.terms.is_none()
    }

    /// Checks field conditions and free text against `record`. Field
    /// conditions are checked first, so a record they rule out never has its
    /// body decoded.
    #[must_use]
    pub fn check(&self, record: &Exchange, cache: &mut BodyCache) -> Checked {
        let fields_match = self.fields.matches(record);
        match &self.terms {
            Some(terms) if fields_match => terms.check(record, self.scope, cache),
            _ => Checked {
                id: record.id(),
                matched: fields_match,
                settled: crate::search::is_settled(record),
                retry: false,
                coverage: crate::search::Coverage::default(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use http::{Method, StatusCode};
    use http_inspector_exchange::{BodyLifecycle, ExchangeBuilder, ResponseFixture};

    use super::*;

    #[test]
    fn combines_fields_and_free_text() -> Result<(), Box<dyn Error>> {
        let ok = ExchangeBuilder::http(Method::POST, "http://api.example.test/users")?.respond(
            ResponseFixture::new(StatusCode::OK)
                .body("{\"name\":\"alice\"}", BodyLifecycle::Complete),
        )?;
        let missing = ExchangeBuilder::http(Method::GET, "http://api.example.test/users")?
            .respond(ResponseFixture::new(StatusCode::NOT_FOUND))?;
        let mut cache = BodyCache::default();
        let query = RequestQuery::compile(
            "method:POST alice",
            SearchScope::All,
            MatchOptions::default(),
        )?;
        assert!(query.check(&ok, &mut cache).matched);
        assert!(!query.check(&missing, &mut cache).matched);

        let excluded =
            RequestQuery::compile("-status:2xx", SearchScope::All, MatchOptions::default())?;
        assert!(!excluded.check(&ok, &mut cache).matched);
        assert!(excluded.check(&missing, &mut cache).matched);

        let regex = RequestQuery::compile(
            "al+ice",
            SearchScope::ResponseBody,
            MatchOptions {
                case_sensitive: false,
                regex: true,
            },
        )?;
        assert!(!regex.fields().has_fields());
        assert!(regex.check(&ok, &mut cache).matched);
        assert!(RequestQuery::compile("", SearchScope::All, MatchOptions::default())?.is_empty());
        Ok(())
    }
}
