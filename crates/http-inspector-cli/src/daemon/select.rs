//! Which records a filter selects, and the changes a watch reports.

use std::collections::HashMap;

use http_inspector_analysis::filter::{RecordFilter, RequestQuery, record_matches};
use http_inspector_analysis::search::BodyCache;
use http_inspector_exchange::{Exchange, RecordId};
use serde_json::{Value, json};

use super::session::Session;
use crate::error::{CliError, ErrorCode};
use crate::protocol::{FilterSpec, RecordRef};
use crate::view;

/// A compiled filter: the outcome kind plus the request filter shared with
/// the desktop list.
pub(super) struct Selection {
    kind: RecordFilter,
    query: RequestQuery,
    cache: BodyCache,
}

impl Selection {
    /// Compiles `spec`. Unlike the desktop input, which skips unusable
    /// terms while the user types, the command line refuses them so a
    /// script never runs a broader filter than it wrote.
    pub(super) fn compile(spec: &FilterSpec) -> Result<Self, CliError> {
        let query =
            RequestQuery::compile(&spec.text, spec.scope, spec.options).map_err(|error| {
                CliError::new(ErrorCode::InvalidFilter, format!("Invalid filter: {error}"))
            })?;
        let fields = query.fields();
        if let Some(diagnostic) = fields.diagnostics.first() {
            let term = spec.text.get(diagnostic.span.clone()).unwrap_or_default();
            return Err(CliError::new(
                ErrorCode::InvalidFilter,
                format!("Invalid filter term {term:?}: {}", diagnostic.message),
            )
            .with_hint("see the filter syntax in docs/cli.md"));
        }
        Ok(Self {
            kind: spec.kind.0,
            query,
            cache: BodyCache::default(),
        })
    }

    pub(super) fn matches(&mut self, record: &Exchange) -> bool {
        record_matches(record, self.kind) && self.query.check(record, &mut self.cache).matched
    }
}

type StateKey = (&'static str, Option<u16>, bool);

/// What a watch has already reported.
pub(super) struct WatchState {
    selection: Selection,
    since: u64,
    generation: u64,
    shown: HashMap<RecordId, StateKey>,
    full: bool,
}

impl WatchState {
    /// Starts after `since`, or after the newest retained record.
    pub(super) fn new(
        selection: Selection,
        session: &Session,
        since: Option<RecordRef>,
        full: bool,
    ) -> Self {
        let newest = session
            .records()
            .iter()
            .next_back()
            .map_or(0, |record| record.id().value());
        Self {
            selection,
            since: since.map_or(newest, RecordRef::value),
            generation: session.generation(),
            shown: HashMap::new(),
            full,
        }
    }

    pub(super) const fn since(&self) -> u64 {
        self.since
    }

    /// Events for records that started matching or changed state since the
    /// last call, plus `cleared` after `records clear`.
    pub(super) fn events(&mut self, session: &Session) -> Vec<Value> {
        let mut events = Vec::new();
        if session.generation() != self.generation {
            self.generation = session.generation();
            self.shown.clear();
            events.push(json!({ "event": "cleared" }));
        }
        for record in session.records().iter() {
            if record.id().value() <= self.since || !self.selection.matches(record) {
                continue;
            }
            let key = view::state_key(record);
            let previous = self.shown.insert(record.id(), key);
            if previous != Some(key) {
                let event = if previous.is_some() {
                    "updated"
                } else {
                    "added"
                };
                events.push(json!({ "event": event, "record": view::listed(record, self.full) }));
            }
        }
        let oldest = session.records().iter().next().map(Exchange::id);
        if let Some(oldest) = oldest {
            self.shown.retain(|id, _| id.value() >= oldest.value());
        }
        events
    }
}
