//! Text search over captured requests, including their decoded body previews,
//! run on a background thread.
//!
//! Search only decides which records the list shows. It does not change what
//! the proxy forwards or what the full-session log records.

pub mod document;
pub mod text_search;
pub mod worker;

pub use document::{
    BodyCache, Checked, Coverage, check, headers_match, is_settled, outcome_matches,
};
pub use text_search::{MatchOptions, Matcher, MatcherError};
pub use worker::{Query, Report, SearchWorker};

use http_inspector_exchange::BodySide;

/// Which parts of a request the search text is looked for in.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum SearchScope {
    /// Method, URL, headers, status or failure, and both bodies.
    #[default]
    All,
    /// Request target URL only.
    Url,
    /// Request and response header names and retained values.
    Headers,
    /// Decoded request body preview only.
    RequestBody,
    /// Decoded response body preview only.
    ResponseBody,
}

impl SearchScope {
    /// All search scopes in display order.
    pub const ALL: [Self; 5] = [
        Self::All,
        Self::Url,
        Self::Headers,
        Self::RequestBody,
        Self::ResponseBody,
    ];

    /// Short display label for this scope.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Url => "URL",
            Self::Headers => "Headers",
            Self::RequestBody => "Request body",
            Self::ResponseBody => "Response body",
        }
    }

    /// Reports whether this scope searches the request target URL.
    #[must_use]
    pub const fn covers_url(self) -> bool {
        matches!(self, Self::All | Self::Url)
    }

    /// Reports whether this scope searches request and response headers.
    #[must_use]
    pub const fn covers_headers(self) -> bool {
        matches!(self, Self::All | Self::Headers)
    }

    /// Reports whether this scope searches the body on `side`.
    #[must_use]
    pub const fn covers_body(self, side: BodySide) -> bool {
        matches!(
            (self, side),
            (Self::All, _)
                | (Self::RequestBody, BodySide::Request)
                | (Self::ResponseBody, BodySide::Response)
        )
    }

    /// Reports whether this scope searches at least one body side.
    #[must_use]
    pub const fn covers_bodies(self) -> bool {
        matches!(self, Self::All | Self::RequestBody | Self::ResponseBody)
    }
}
