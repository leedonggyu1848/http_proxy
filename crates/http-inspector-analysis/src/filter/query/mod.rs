//! Parsed field filters. Matching reads the same captured metadata as the list.

mod parse;
mod predicate;

use std::ops::Range;

use http_inspector_exchange::Exchange;

use self::predicate::Predicate;

/// A searchable request or response field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FilterKey {
    /// Exact request method.
    Method,
    /// Entire request target.
    Url,
    /// Host and optional port.
    Host,
    /// Path and query string.
    Path,
    /// HTTP response code.
    Status,
    /// Short response media type.
    Type,
    /// Request or response headers.
    Header,
    /// Request headers.
    RequestHeader,
    /// Response headers.
    ResponseHeader,
    /// Observed response body bytes.
    Size,
    /// Complete exchange duration.
    Duration,
    /// Kind or capture source of an exchange.
    Is,
}

impl FilterKey {
    /// Supported keys and their accepted aliases.
    pub const ALL: [(&str, Self); 14] = [
        ("method", Self::Method),
        ("url", Self::Url),
        ("host", Self::Host),
        ("domain", Self::Host),
        ("path", Self::Path),
        ("status", Self::Status),
        ("type", Self::Type),
        ("header", Self::Header),
        ("req-header", Self::RequestHeader),
        ("res-header", Self::ResponseHeader),
        ("size", Self::Size),
        ("duration", Self::Duration),
        ("time", Self::Duration),
        ("is", Self::Is),
    ];

    /// Recognizes a complete key and colon at a standalone term's byte range.
    /// The returned flag indicates an exclusion.
    #[must_use]
    pub fn for_completion(input: &str, span: Range<usize>) -> Option<(Self, bool)> {
        parse::key_for_completion(input, span)
    }

    fn from_name(key: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(key))
            .map(|(_, key)| key)
    }

    /// Canonical spelling of the filter key.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Method => "method",
            Self::Url => "url",
            Self::Host => "host",
            Self::Path => "path",
            Self::Status => "status",
            Self::Type => "type",
            Self::Header => "header",
            Self::RequestHeader => "req-header",
            Self::ResponseHeader => "res-header",
            Self::Size => "size",
            Self::Duration => "duration",
            Self::Is => "is",
        }
    }
}

/// Why a term was skipped and where it occurs in the input.
#[derive(Debug)]
pub struct FilterDiagnostic {
    /// UTF-8 byte range of the skipped term.
    pub span: Range<usize>,
    /// Reason the term could not be used.
    pub message: String,
}

/// A free-text term to search using the current text search scope.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextTerm {
    /// The term without quotes or escapes.
    pub text: String,
    /// Whether to retain only exchanges where this term is absent.
    pub excluded: bool,
}

#[derive(Debug)]
struct FieldTerm {
    alternatives: Vec<Predicate>,
    excluded: bool,
}

/// Metadata predicates evaluated together with AND.
#[derive(Debug, Default)]
pub struct FilterQuery {
    fields: Vec<FieldTerm>,
    unclosed_term_start: Option<usize>,
    /// Free-text terms, matched by the text search worker.
    pub text: Vec<TextTerm>,
    /// Invalid terms to report to the user. Valid terms remain active.
    pub diagnostics: Vec<FilterDiagnostic>,
}

impl FilterQuery {
    /// Parses literal search input. Regex mode bypasses this grammar.
    #[must_use]
    pub fn parse(input: &str) -> Self {
        parse::query(input)
    }

    /// Byte offset of the trailing term that contains an unclosed quote, if any.
    #[must_use]
    pub const fn unclosed_term_start(&self) -> Option<usize> {
        self.unclosed_term_start
    }

    /// Reports whether all parsed metadata conditions match the exchange.
    #[must_use]
    pub fn matches(&self, record: &Exchange) -> bool {
        self.fields.iter().all(|term| {
            term.alternatives.iter().any(|value| value.matches(record)) != term.excluded
        })
    }

    /// Reports whether the query contains any valid metadata conditions.
    #[must_use]
    pub const fn has_fields(&self) -> bool {
        !self.fields.is_empty()
    }
}

#[cfg(test)]
mod tests;
