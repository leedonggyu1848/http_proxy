//! Request filtering by outcome kind, Map scope, and search predicate, plus the
//! bounded [`RecordList`] of displayed records.

mod records;

use std::collections::HashSet;

pub use records::RecordList;

use crate::url_map::RequestScope;
use http_inspector_exchange::{Exchange, Outcome, RecordId};

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
        .filter(|record| in_scope(record, scope) && search_shows(record.id()))
        .collect()
}

/// Number of records each [`RecordFilter`] would show under the same Map
/// scope and search predicate.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FilterCounts {
    all: usize,
    responses: usize,
    failures: usize,
    tunnels: usize,
}

impl FilterCounts {
    /// Records `filter` would show.
    #[must_use]
    pub const fn get(self, filter: RecordFilter) -> usize {
        match filter {
            RecordFilter::All => self.all,
            RecordFilter::Responses => self.responses,
            RecordFilter::Failures => self.failures,
            RecordFilter::Tunnels => self.tunnels,
        }
    }

    const fn slot(&mut self, filter: RecordFilter) -> &mut usize {
        match filter {
            RecordFilter::All => &mut self.all,
            RecordFilter::Responses => &mut self.responses,
            RecordFilter::Failures => &mut self.failures,
            RecordFilter::Tunnels => &mut self.tunnels,
        }
    }
}

/// Counts the records every outcome filter would show, so that the choice of
/// filter can display its result before it is picked.
#[must_use]
pub fn filter_counts(
    records: &RecordList<Exchange>,
    scope: Option<ScopeFilter<'_>>,
    mut search_shows: impl FnMut(RecordId) -> bool,
) -> FilterCounts {
    let mut counts = FilterCounts::default();
    for record in records
        .iter()
        .filter(|record| in_scope(record, scope) && search_shows(record.id()))
    {
        for filter in RecordFilter::ALL {
            if record_matches(record, filter) {
                *counts.slot(filter) += 1;
            }
        }
    }
    counts
}

fn in_scope(record: &Exchange, scope: Option<ScopeFilter<'_>>) -> bool {
    match scope {
        None => true,
        Some(ScopeFilter::Cached(ids)) => ids.contains(&record.id()),
        Some(ScopeFilter::Live(key)) => key.matches_exchange(record),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::error::Error;

    use http::{Method, StatusCode};

    use super::*;
    use crate::url::{OriginKey, PathSegmentPattern};
    use crate::url_map::ViewNodeKey;
    use http_inspector_exchange::{BodyLifecycle, FailureStage, TunnelState};
    use http_inspector_exchange::{ExchangeBuilder, ResponseFixture};

    type TestResult = Result<(), Box<dyn Error>>;

    fn check_filters(record: &Exchange, expected: [bool; 4]) {
        for (filter, expected) in RecordFilter::ALL.into_iter().zip(expected) {
            assert_eq!(
                record_matches(record, filter),
                expected,
                "{filter:?}: {:?}",
                record.outcome()
            );
        }
    }

    #[test]
    fn filters_exchanges_by_outcome_kind() -> TestResult {
        let pending = ExchangeBuilder::http(Method::GET, "http://api.example.test/items")?
            .request_body(Vec::new(), BodyLifecycle::Complete)
            .in_flight()?;
        let response = ExchangeBuilder::http(Method::GET, "http://api.example.test/items")?
            .respond(ResponseFixture::new(StatusCode::CREATED))?;
        let tunnel =
            ExchangeBuilder::connect("api.example.test:443").tunnel(4, 5, TunnelState::Closed)?;
        let inspected =
            ExchangeBuilder::connect("api.example.test:443").inspection_established()?;
        let failed = ExchangeBuilder::http(Method::GET, "http://api.example.test/items")?
            .fail(FailureStage::Connect, "connection refused")?;
        let cancelled =
            ExchangeBuilder::http(Method::GET, "http://api.example.test/items")?.cancel()?;
        let incomplete =
            ExchangeBuilder::http(Method::GET, "http://api.example.test/items")?.stop()?;

        check_filters(&pending, [true, false, false, false]);
        check_filters(&response, [true, true, false, false]);
        check_filters(&tunnel, [true, false, false, true]);
        check_filters(&inspected, [true, false, false, true]);
        check_filters(&failed, [true, false, true, false]);
        check_filters(&cancelled, [true, false, true, false]);
        check_filters(&incomplete, [true, false, true, false]);
        Ok(())
    }

    #[test]
    fn visible_records_combines_outcome_scope_and_search_in_arrival_order() -> TestResult {
        let users_ok = ExchangeBuilder::http(Method::GET, "http://api.example.test/api/users/1")?
            .respond(ResponseFixture::new(StatusCode::OK))?;
        let users_fail = ExchangeBuilder::http(Method::GET, "http://api.example.test/api/users/2")?
            .fail(FailureStage::Connect, "refused")?;
        let orders_ok = ExchangeBuilder::http(Method::GET, "http://api.example.test/api/orders")?
            .respond(ResponseFixture::new(StatusCode::OK))?;

        let (u1, u2, o1) = (users_ok.id(), users_fail.id(), orders_ok.id());
        let mut list = RecordList::new(8);
        let _ = list.push(users_ok);
        let _ = list.push(users_fail);
        let _ = list.push(orders_ok);

        let users_scope = RequestScope::subtree(ViewNodeKey {
            origin: OriginKey::new("http", "api.example.test", None),
            path: vec![
                PathSegmentPattern::Literal("api".to_owned()),
                PathSegmentPattern::Literal("users".to_owned()),
            ],
        });

        let all_ids: Vec<RecordId> = visible_records(&list, RecordFilter::All, None, |_| true)
            .into_iter()
            .map(Exchange::id)
            .collect();
        assert_eq!(all_ids, vec![u1, u2, o1]);

        let live_users: Vec<RecordId> = visible_records(
            &list,
            RecordFilter::All,
            Some(ScopeFilter::Live(&users_scope)),
            |_| true,
        )
        .into_iter()
        .map(Exchange::id)
        .collect();
        assert_eq!(live_users, vec![u1, u2]);

        let cached: HashSet<RecordId> = [u1, o1].into_iter().collect();
        let cached_filtered: Vec<RecordId> = visible_records(
            &list,
            RecordFilter::Responses,
            Some(ScopeFilter::Cached(&cached)),
            |id| id != o1,
        )
        .into_iter()
        .map(Exchange::id)
        .collect();
        assert_eq!(cached_filtered, vec![u1]);
        Ok(())
    }

    #[test]
    fn filter_counts_match_the_rows_each_filter_shows() -> TestResult {
        let ok = ExchangeBuilder::http(Method::GET, "http://api.example.test/api/users/1")?
            .respond(ResponseFixture::new(StatusCode::OK))?;
        let failed = ExchangeBuilder::http(Method::GET, "http://api.example.test/api/users/2")?
            .fail(FailureStage::Connect, "refused")?;
        let tunnel =
            ExchangeBuilder::connect("api.example.test:443").tunnel(4, 5, TunnelState::Closed)?;
        let hidden = ExchangeBuilder::http(Method::GET, "http://api.example.test/hidden")?
            .respond(ResponseFixture::new(StatusCode::OK))?;
        let hidden_id = hidden.id();
        let mut list = RecordList::new(8);
        for record in [ok, failed, tunnel, hidden] {
            let _ = list.push(record);
        }

        let counts = filter_counts(&list, None, |id| id != hidden_id);
        for filter in RecordFilter::ALL {
            let shown = visible_records(&list, filter, None, |id| id != hidden_id).len();
            assert_eq!(counts.get(filter), shown, "{filter:?}");
        }
        assert_eq!(counts.get(RecordFilter::All), 3);
        assert_eq!(counts.get(RecordFilter::Responses), 1);
        assert_eq!(counts.get(RecordFilter::Failures), 1);
        assert_eq!(counts.get(RecordFilter::Tunnels), 1);
        Ok(())
    }
}
