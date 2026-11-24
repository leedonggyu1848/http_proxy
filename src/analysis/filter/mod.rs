//! Request filtering by outcome kind, Map scope, and search predicate, plus the
//! bounded [`RecordList`] of displayed records.

mod records;

use std::collections::HashSet;

pub use records::RecordList;

use crate::analysis::url_map::RequestScope;
use crate::{Exchange, Outcome, RecordId};

/// Outcome category filter for the request list.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecordFilter {
    /// Show every exchange regardless of outcome.
    All,
    /// Show exchanges that received an HTTP response.
    Responses,
    /// Show exchanges that failed.
    Failures,
    /// Show `CONNECT` tunnels and established HTTPS inspection sessions.
    Tunnels,
}

impl RecordFilter {
    /// All outcome filters in display order.
    pub const ALL: [Self; 4] = [Self::All, Self::Responses, Self::Failures, Self::Tunnels];

    /// Short display label for this filter.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Responses => "Responses",
            Self::Failures => "Failures",
            Self::Tunnels => "Tunnels",
        }
    }
}

/// Optional Map scope constraint for [`visible_records`].
#[derive(Clone, Copy, Debug)]
pub enum ScopeFilter<'a> {
    /// Evaluate [`RequestScope::matches_exchange`] for each exchange.
    Live(&'a RequestScope),
    /// Use a precomputed set of matching record ids for the current list revision.
    Cached(&'a HashSet<RecordId>),
}

/// Reports whether `record` has the outcome kind `filter` selects.
#[must_use]
pub fn record_matches(record: &Exchange, filter: RecordFilter) -> bool {
    let outcome = record.outcome();
    match filter {
        RecordFilter::All => true,
        RecordFilter::Responses => matches!(outcome, Outcome::Response(_)),
        RecordFilter::Failures => outcome.is_failure(),
        RecordFilter::Tunnels => {
            matches!(outcome, Outcome::Tunnel(_) | Outcome::InspectionEstablished)
        }
    }
}

/// Records that match the outcome filter, optional Map scope, and search
/// predicate, in arrival order.
#[must_use]
pub fn visible_records<'a>(
    records: &'a RecordList<Exchange>,
    filter: RecordFilter,
    scope: Option<ScopeFilter<'_>>,
    mut search_shows: impl FnMut(RecordId) -> bool,
) -> Vec<&'a Exchange> {
    records
        .iter()
        .filter(|record| record_matches(record, filter))
        .filter(|record| match scope {
            None => true,
            Some(ScopeFilter::Cached(ids)) => ids.contains(&record.id()),
            Some(ScopeFilter::Live(key)) => key.matches_exchange(record),
        })
        .filter(|record| search_shows(record.id()))
        .collect()
}
