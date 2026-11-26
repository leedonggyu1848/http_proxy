//! The search controls above the request list and the state that decides
//! which records the list shows.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use http_inspector_analysis::filter::{self, FilterQuery, RecordList, TextTerm};
use http_inspector_analysis::search::document::{self, BodyCache, Checked};
use http_inspector_analysis::search::{MatchOptions, Query, SearchScope, SearchWorker, TextTerms};
use http_inspector_exchange::{Exchange, RecordId};

use crate::ui::style::tokens::Tone;
use crate::ui::workbench::status::StatusItem;
use crate::ui::workbench::text::{self, ShellText};

/// Wait after the last keystroke before a search starts, so typing a word
/// does not start a search for every letter.
const DEBOUNCE: Duration = Duration::from_millis(150);
/// Wait before retrying a locked preview or a full search queue.
const RETRY: Duration = Duration::from_millis(50);

/// What the user asked to search for.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct SearchSettings {
    pub(super) text: String,
    pub(super) scope: SearchScope,
    pub(super) options: MatchOptions,
}

impl SearchSettings {
    /// Reports whether every record matching `self` also matches
    /// `previous`, as when letters are added to a word. Records that
    /// `previous` ruled out then need no new check.
    fn narrows(&self, terms: &[TextTerm], previous: &Active) -> bool {
        if self.options.regex
            || self.options != previous.settings.options
            || self.scope != previous.settings.scope
        {
            return false;
        }
        previous.terms.iter().all(|old| {
            terms.iter().any(|new| {
                old == new
                    || (!old.excluded
                        && !new.excluded
                        && if self.options.case_sensitive {
                            new.text.contains(&old.text)
                        } else {
                            // Changing between the ASCII and Unicode matchers
                            // need not preserve the old match set.
                            old.text.is_ascii()
                                && new.text.is_ascii()
                                && new
                                    .text
                                    .to_ascii_lowercase()
                                    .contains(&old.text.to_ascii_lowercase())
                        })
            })
        })
    }
}

struct Active {
    settings: SearchSettings,
    terms: Vec<TextTerm>,
    query: Arc<Query>,
}

struct SearchResult {
    checked: Checked,
    retry_at: Option<Instant>,
}

impl SearchResult {
    fn new(checked: Checked) -> Self {
        Self {
            checked,
            retry_at: checked.retry.then(|| Instant::now() + RETRY),
        }
    }
}

/// Counts shown under the controls.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Summary {
    total: usize,
    checked: usize,
    partial: usize,
    binary: usize,
    undecodable: usize,
}

/// Background search worker, or an inline fallback when thread creation fails.
enum SearchBackend {
    Worker(SearchWorker),
    Inline { notice: String, cache: BodyCache },
}

pub(crate) struct RequestSearch {
    settings: SearchSettings,
    parsed: FilterQuery,
    changed_at: Option<Instant>,
    active: Option<Active>,
    error: Option<String>,
    generation: u64,
    results: HashMap<RecordId, SearchResult>,
    /// Records sent to the worker for the active query and not reported yet.
    queued: HashSet<RecordId>,
    summary: Summary,
    backend: SearchBackend,
}

impl RequestSearch {
    pub(crate) fn new(context: &egui::Context) -> Self {
        let context = context.clone();
        let backend = match SearchWorker::spawn(move || context.request_repaint()) {
            Ok(worker) => SearchBackend::Worker(worker),
            Err(error) => SearchBackend::Inline {
                notice: format!(
                    "Background search is unavailable ({error}); searching on the UI thread."
                ),
                cache: BodyCache::default(),
            },
        };
        Self {
            settings: SearchSettings::default(),
            parsed: FilterQuery::default(),
            changed_at: None,
            active: None,
            error: None,
            generation: 0,
            results: HashMap::new(),
            queued: HashSet::new(),
            summary: Summary::default(),
            backend,
        }
    }

    pub(super) const fn settings(&self) -> &SearchSettings {
        &self.settings
    }

    pub(super) fn regex_error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub(super) const fn parsed(&self) -> &FilterQuery {
        &self.parsed
    }

    pub(crate) const fn field_menu_enabled(&self) -> bool {
        !self.settings.options.regex
    }

    pub(crate) fn set_settings(&mut self, settings: SearchSettings) {
        if self.settings == settings {
            return;
        }
        if self.settings.text != settings.text
            || self.settings.options.regex != settings.options.regex
        {
            self.parsed = if settings.options.regex {
                FilterQuery::default()
            } else {
                FilterQuery::parse(&settings.text)
            };
        }
        self.settings = settings;
        if self.parsed.text.is_empty()
            && (!self.settings.options.regex || self.settings.text.is_empty())
        {
            self.apply();
        } else {
            self.changed_at = Some(Instant::now());
        }
    }

    pub(crate) fn append_filter(&mut self, term: &str) {
        let mut settings = self.settings.clone();
        if let Some(start) = self.parsed.unclosed_term_start() {
            // A later term would become part of the open quote.
            settings.text.insert_str(start, &format!("{term} "));
        } else {
            if !settings.text.is_empty() && !settings.text.ends_with(char::is_whitespace) {
                settings.text.push(' ');
            }
            settings.text.push_str(term);
        }
        self.set_settings(settings);
    }

    /// Search progress and what was not searched, for the status bar.
    /// Matches are counted there with the rest of the list; parser errors
    /// show in the filter input. Empty while no search is active.
    pub(crate) fn status_items(&self) -> Vec<StatusItem> {
        let mut items = Vec::new();
        if let SearchBackend::Inline { notice, .. } = &self.backend {
            items.push(StatusItem::marked(
                Tone::Warning,
                ShellText::SearchOnUiThread.text(),
                notice.clone(),
            ));
        }
        let Some(active) = &self.active else {
            return items;
        };
        let summary = self.summary;
        if summary.checked < summary.total {
            items.push(StatusItem::progress(text::search_progress(
                summary.checked,
                summary.total,
            )));
        }
        if active.settings.scope.covers_bodies()
            && let Some(coverage) = coverage_text(summary)
        {
            items.push(StatusItem::marked(
                Tone::Warning,
                coverage.short,
                coverage.detail,
            ));
        }
        items
    }

    /// Applies edits after the debounce, collects results, and queues new
    /// or newly settled records. Call once per frame before [`Self::shows`].
    pub(crate) fn update(&mut self, records: &RecordList<Exchange>, context: &egui::Context) {
        if let Some(changed_at) = self.changed_at {
            let elapsed = changed_at.elapsed();
            if elapsed >= DEBOUNCE {
                self.apply();
            } else {
                context.request_repaint_after(DEBOUNCE.saturating_sub(elapsed));
            }
        }
        self.receive();
        self.forget_missing(records);
        if !self.submit(records, Instant::now()) {
            context.request_repaint_after(RETRY);
        }
        self.summary = self.summarize(records);
    }

    /// Reports whether the list shows the record with `id`.
    pub(crate) fn shows(&self, id: RecordId) -> bool {
        self.active.is_none()
            || self
                .results
                .get(&id)
                .is_some_and(|result| result.checked.matched)
    }

    /// The query whose matches the detail panel highlights.
    pub(crate) fn active_query(&self) -> Option<&Arc<Query>> {
        self.active.as_ref().map(|active| &active.query)
    }

    pub(crate) fn matches(&self, record: &Exchange) -> bool {
        self.parsed.matches(record) && self.shows(record.id())
    }

    fn apply(&mut self) {
        self.changed_at = None;
        let finished = self.queued.is_empty();
        self.generation += 1;
        if let SearchBackend::Worker(worker) = &self.backend {
            worker.supersede(self.generation);
        }
        self.queued.clear();
        self.error = None;
        let previous = self.active.take();
        let settings = self.settings.clone();
        let terms = filter::text_terms(&settings.text, settings.options, &self.parsed);
        if terms.is_empty() {
            self.results.clear();
            return;
        }
        let compiled = match TextTerms::compile(&terms, settings.options) {
            Ok(compiled) => compiled,
            Err(error) => {
                self.error = Some(capitalized(&error.to_string()));
                self.results.clear();
                return;
            }
        };
        if finished && previous.is_some_and(|previous| settings.narrows(&terms, &previous)) {
            self.results
                .retain(|_, result| result.checked.settled && !result.checked.matched);
        } else {
            self.results.clear();
        }
        let query = Arc::new(Query {
            generation: self.generation,
            terms: compiled,
            scope: settings.scope,
        });
        self.active = Some(Active {
            settings,
            terms,
            query,
        });
    }

    fn receive(&mut self) {
        let SearchBackend::Worker(worker) = &self.backend else {
            return;
        };
        while let Some(report) = worker.try_report() {
            if report.generation != self.generation {
                continue;
            }
            for checked in report.checked {
                self.queued.remove(&checked.id);
                self.results.insert(checked.id, SearchResult::new(checked));
            }
        }
    }

    /// Drops results of records the list no longer holds.
    fn forget_missing(&mut self, records: &RecordList<Exchange>) {
        if self.results.is_empty() && self.queued.is_empty() {
            return;
        }
        let present: HashSet<RecordId> = records.iter().map(Exchange::id).collect();
        self.results.retain(|id, _| present.contains(id));
        self.queued.retain(|id| present.contains(id));
    }

    /// Queues ready records. Returns `false` if a locked preview or a full
    /// queue needs a timed retry.
    fn submit(&mut self, records: &RecordList<Exchange>, now: Instant) -> bool {
        let Some(query) = self.active.as_ref().map(|active| Arc::clone(&active.query)) else {
            return true;
        };
        let mut deferred = false;
        let pending: Vec<Exchange> = records
            .iter()
            .filter(|record| !self.queued.contains(&record.id()))
            .filter(|record| {
                self.results.get(&record.id()).is_none_or(|result| {
                    if let Some(deadline) = result.retry_at {
                        deferred |= now < deadline;
                        return now >= deadline;
                    }
                    !result.checked.settled && document::is_settled(record)
                })
            })
            .cloned()
            .collect();
        if pending.is_empty() {
            return !deferred;
        }
        match &mut self.backend {
            SearchBackend::Worker(worker) => {
                let ids: Vec<RecordId> = pending.iter().map(Exchange::id).collect();
                let oldest = records.iter().next().map(Exchange::id);
                if worker.submit(&query, pending, oldest).is_err() {
                    return false;
                }
                self.queued.extend(ids);
            }
            SearchBackend::Inline { cache, .. } => {
                for record in &pending {
                    let checked = query.terms.check(record, query.scope, cache);
                    deferred |= checked.retry;
                    self.results.insert(checked.id, SearchResult::new(checked));
                }
            }
        }
        !deferred
    }

    fn summarize(&self, records: &RecordList<Exchange>) -> Summary {
        let mut summary = Summary {
            total: records.len(),
            ..Summary::default()
        };
        if self.active.is_none() {
            return summary;
        }
        for record in records.iter() {
            let Some(result) = self.results.get(&record.id()) else {
                continue;
            };
            let checked = result.checked;
            if !self.queued.contains(&checked.id) && !checked.retry {
                summary.checked += 1;
            }
            if checked.coverage != document::Coverage::default() && self.parsed.matches(record) {
                summary.partial += usize::from(checked.coverage.partial);
                summary.binary += usize::from(checked.coverage.binary);
                summary.undecodable += usize::from(checked.coverage.undecodable);
            }
        }
        summary
    }

    /// Records that did not match although part of a body was not searched.
    #[cfg(test)]
    pub(crate) const fn partially_searched(&self) -> usize {
        self.summary.partial
    }

    /// Runs a search to completion, skipping the debounce.
    #[cfg(test)]
    pub(crate) fn search_now(
        &mut self,
        (text, scope, options): (&str, SearchScope, MatchOptions),
        records: &RecordList<Exchange>,
        context: &egui::Context,
    ) {
        self.set_settings(SearchSettings {
            text: text.to_owned(),
            scope,
            options,
        });
        self.apply();
        self.update(records, context);
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.active.is_some()
            && (self.summary.checked < self.summary.total || !self.queued.is_empty())
            && Instant::now() < deadline
        {
            std::thread::yield_now();
            self.update(records, context);
        }
    }
}

/// Bodies that were not fully searched: a short
/// count for the status bar and the breakdown for its tooltip.
struct Coverage {
    short: String,
    detail: String,
}

fn coverage_text(summary: Summary) -> Option<Coverage> {
    let mut parts = Vec::new();
    if summary.partial > 0 {
        parts.push(format!(
            "{} only in their first {} KiB",
            summary.partial,
            http_inspector_exchange::capture::BODY_PREVIEW_BYTES / 1024
        ));
    }
    if summary.binary > 0 {
        parts.push(format!("{} binary, skipped", summary.binary));
    }
    if summary.undecodable > 0 {
        parts.push(format!("{} could not be decoded", summary.undecodable));
    }
    let unsearched = summary.partial + summary.binary + summary.undecodable;
    (!parts.is_empty()).then(|| Coverage {
        short: text::partly_searched(unsearched),
        detail: format!(
            "Captured previews were searched after decompression ({}); the full-session log keeps the rest.",
            parts.join(", ")
        ),
    })
}

fn capitalized(text: &str) -> String {
    let mut characters = text.chars();
    characters.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(characters).collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(text: &str, case_sensitive: bool, regex: bool) -> SearchSettings {
        SearchSettings {
            text: text.to_owned(),
            scope: SearchScope::All,
            options: MatchOptions {
                case_sensitive,
                regex,
            },
        }
    }

    #[test]
    fn adding_letters_narrows_but_removing_them_does_not() -> Result<(), Box<dyn std::error::Error>>
    {
        let previous_settings = settings("Ord", false, false);
        let previous_terms = FilterQuery::parse(&previous_settings.text).text;
        let terms = TextTerms::compile(&previous_terms, previous_settings.options)?;
        let previous = Active {
            settings: previous_settings,
            terms: previous_terms,
            query: Arc::new(Query {
                generation: 0,
                terms,
                scope: SearchScope::All,
            }),
        };
        let narrows =
            |next: &SearchSettings| next.narrows(&FilterQuery::parse(&next.text).text, &previous);
        assert!(narrows(&settings("xorder", false, false)));
        assert!(!narrows(&settings("Or", false, false)));
        assert!(!narrows(&settings("order", true, false)));
        assert!(!narrows(&settings("Order", false, true)));
        assert!(!narrows(&settings("-Order", false, false)));
        assert!(!narrows(&settings("Or der", false, false)));
        let mut other_scope = settings("Order", false, false);
        other_scope.scope = SearchScope::Url;
        assert!(!narrows(&other_scope));
        Ok(())
    }

    #[test]
    fn adds_cell_filters_before_an_unclosed_term() -> Result<(), Box<dyn std::error::Error>> {
        use http::{Method, StatusCode};
        use http_inspector_exchange::{ExchangeBuilder, ResponseFixture};

        let captured = |status| {
            ExchangeBuilder::http(Method::POST, "http://localhost:8080/api")?
                .respond(ResponseFixture::new(status))
        };
        let forbidden = captured(StatusCode::FORBIDDEN)?;
        let not_found = captured(StatusCode::NOT_FOUND)?;
        // Start after a multi-byte separator so the insertion offset is a byte index.
        let original = "method:POST\u{2003}\"unfinished text";
        let context = egui::Context::default();
        let mut search = RequestSearch::new(&context);
        for (term, keeps_forbidden) in [("status:403", true), ("-status:403", false)] {
            search.set_settings(settings(original, false, false));
            search.append_filter(term);
            assert_eq!(
                (search.matches(&forbidden), search.matches(&not_found)),
                (keeps_forbidden, !keeps_forbidden),
                "{term}"
            );
            let input = &search.settings().text;
            assert_eq!(
                input,
                &format!("method:POST\u{2003}{term} \"unfinished text")
            );
            let warnings: Vec<_> = search
                .parsed()
                .diagnostics
                .iter()
                .filter_map(|warning| input.get(warning.span.clone()))
                .collect();
            assert_eq!(warnings, ["\"unfinished text"]);
        }
        for text in [
            "method:POST",
            "method:POST \"finished text\"",
            "status:abc ",
        ] {
            search.set_settings(settings(text, false, false));
            search.append_filter("status:403");
            assert_eq!(
                search.settings().text,
                format!("{} status:403", text.trim_end())
            );
        }
        Ok(())
    }

    #[test]
    fn coverage_lists_only_what_was_not_fully_searched() {
        assert!(coverage_text(Summary::default()).is_none());
        let text = coverage_text(Summary {
            partial: 2,
            binary: 1,
            ..Summary::default()
        })
        .map_or_default(|text| text.detail);
        assert!(
            text.contains("2 only in their first 8 KiB, 1 binary, skipped"),
            "{text}"
        );
    }

    #[test]
    fn a_locked_request_preview_is_rechecked_while_the_response_streams()
    -> Result<(), Box<dyn std::error::Error>> {
        use http::{Method, StatusCode};
        use http_inspector_exchange::{BodyLifecycle, ExchangeBuilder, ResponseFixture};

        let record = ExchangeBuilder::http(Method::POST, "http://api.example.test/")?
            .request_body("search needle", BodyLifecycle::Complete)
            .respond(
                ResponseFixture::new(StatusCode::OK).body(Vec::new(), BodyLifecycle::Streaming),
            )?;
        assert!(!document::is_settled(&record));
        let id = record.id();
        let preview = record
            .request()
            .body()
            .ok_or("missing request body")?
            .clone();
        let mut records = RecordList::new(1);
        let _evicted = records.push(record);
        let mut search = RequestSearch::new(&egui::Context::default());
        search.backend = SearchBackend::Inline {
            notice: String::new(),
            cache: BodyCache::default(),
        };
        let mut query = settings("needle", false, false);
        query.scope = SearchScope::RequestBody;
        search.set_settings(query);
        search.apply();
        let active_query = search.active_query().ok_or("missing active query")?.clone();
        let now = Instant::now();

        preview.with_locked_prefix_for_test(|| {
            assert!(!search.submit(&records, now));
        });
        let deadline = search
            .results
            .get(&id)
            .and_then(|result| result.retry_at)
            .ok_or("missing retry deadline")?;
        assert!(!search.shows(id));
        assert!(deadline >= now + RETRY);
        assert!(!search.submit(&records, now + RETRY / 2));
        assert!(!search.shows(id));
        assert!(search.submit(&records, deadline));
        assert!(search.shows(id));
        assert!(
            search
                .results
                .get(&id)
                .is_some_and(|result| result.retry_at.is_none() && !result.checked.settled)
        );
        assert!(
            search
                .active_query()
                .is_some_and(|query| Arc::ptr_eq(query, &active_query))
        );
        Ok(())
    }

    /// The list filter and the shared [`filter::RequestQuery`] used by the
    /// command line select the same records for the same text.
    #[test]
    fn list_filter_matches_the_shared_request_query() -> Result<(), Box<dyn std::error::Error>> {
        use http::header::CONTENT_TYPE;
        use http::{HeaderValue, Method, StatusCode};
        use http_inspector_analysis::filter::RequestQuery;
        use http_inspector_exchange::{BodyLifecycle, ExchangeBuilder, ResponseFixture};

        let json = HeaderValue::from_static("application/json");
        let fixtures = [
            ExchangeBuilder::http(Method::GET, "http://api.example.test/api/users?page=2")?
                .respond(
                    ResponseFixture::new(StatusCode::OK)
                        .header(CONTENT_TYPE, json.clone())
                        .body("{\"name\":\"alice\"}", BodyLifecycle::Complete),
                )?,
            ExchangeBuilder::http(Method::POST, "http://api.example.test/api/users")?
                .request_body("name=bob", BodyLifecycle::Complete)
                .respond(ResponseFixture::new(StatusCode::CREATED).header(CONTENT_TYPE, json))?,
            ExchangeBuilder::http(Method::DELETE, "http://cdn.example.test/img/logo.png")?
                .respond(ResponseFixture::new(StatusCode::NOT_FOUND))?,
            ExchangeBuilder::http(Method::GET, "http://cdn.example.test/down")?
                .fail(http_inspector_exchange::FailureStage::Connect, "refused")?,
            ExchangeBuilder::connect("secure.example.test:443").inspection_established()?,
        ];
        let mut records = RecordList::new(fixtures.len());
        for record in fixtures {
            let _evicted = records.push(record);
        }
        let context = egui::Context::default();
        let mut search = RequestSearch::new(&context);
        let plain = MatchOptions::default();
        let regex = MatchOptions {
            case_sensitive: false,
            regex: true,
        };
        for (text, scope, options) in [
            ("", SearchScope::All, plain),
            ("method:POST", SearchScope::All, plain),
            ("status:4xx", SearchScope::All, plain),
            ("-status:2xx", SearchScope::All, plain),
            ("host:api.example.test alice", SearchScope::All, plain),
            ("bob", SearchScope::RequestBody, plain),
            ("bob", SearchScope::ResponseBody, plain),
            ("type:json -method:GET", SearchScope::All, plain),
            ("is:tunnel", SearchScope::All, plain),
            ("refused", SearchScope::All, plain),
            (
                "ALICE",
                SearchScope::All,
                MatchOptions {
                    case_sensitive: true,
                    regex: false,
                },
            ),
            ("users/?$", SearchScope::Url, regex),
        ] {
            search.search_now((text, scope, options), &records, &context);
            let shown: Vec<_> = records
                .iter()
                .filter(|record| search.matches(record))
                .map(Exchange::id)
                .collect();
            let query = RequestQuery::compile(text, scope, options)?;
            let mut cache = BodyCache::default();
            let shared: Vec<_> = records
                .iter()
                .filter(|record| query.check(record, &mut cache).matched)
                .map(Exchange::id)
                .collect();
            assert_eq!(shown, shared, "{text:?} in {scope:?}");
        }
        Ok(())
    }
}
