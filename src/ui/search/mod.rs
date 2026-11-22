//! Text search over captured requests, including their decoded body
//! previews, run on a background thread.
//!
//! Search only decides which records the list shows. It does not change
//! what the proxy forwards or what the full-session log records.

mod controller;
pub(in crate::ui) mod document;
pub(in crate::ui) mod highlight;
mod worker;

pub(in crate::ui) use controller::RequestSearch;
pub(in crate::ui) use worker::Query;

use crate::ui::body_display::BodySide;

/// Which parts of a request the search text is looked for in.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub(in crate::ui) enum SearchScope {
    /// Method, URL, headers, status or failure, and both bodies.
    #[default]
    All,
    Url,
    Headers,
    RequestBody,
    ResponseBody,
}

impl SearchScope {
    pub(in crate::ui) const ALL: [Self; 5] = [
        Self::All,
        Self::Url,
        Self::Headers,
        Self::RequestBody,
        Self::ResponseBody,
    ];

    pub(in crate::ui) const fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Url => "URL",
            Self::Headers => "Headers",
            Self::RequestBody => "Request body",
            Self::ResponseBody => "Response body",
        }
    }

    const fn hint(self) -> &'static str {
        match self {
            Self::All => "Filter: method, URL, status, header, or body",
            Self::Url => "Filter: URL",
            Self::Headers => "Filter: header name or value",
            Self::RequestBody => "Filter: request body",
            Self::ResponseBody => "Filter: response body",
        }
    }

    pub(in crate::ui) const fn covers_url(self) -> bool {
        matches!(self, Self::All | Self::Url)
    }

    pub(in crate::ui) const fn covers_headers(self) -> bool {
        matches!(self, Self::All | Self::Headers)
    }

    pub(in crate::ui) const fn covers_body(self, side: BodySide) -> bool {
        matches!(
            (self, side),
            (Self::All, _)
                | (Self::RequestBody, BodySide::Request)
                | (Self::ResponseBody, BodySide::Response)
        )
    }

    const fn covers_bodies(self) -> bool {
        matches!(self, Self::All | Self::RequestBody | Self::ResponseBody)
    }
}
