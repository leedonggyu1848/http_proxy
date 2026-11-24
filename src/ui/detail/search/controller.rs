//! The search controls above the request list and the state that decides
//! which records the list shows.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use http_proxy::analysis::filter::RecordList;
use http_proxy::analysis::search::document::{self, BodyCache, Checked};
use http_proxy::analysis::search::{MatchOptions, Matcher, Query, SearchScope, SearchWorker};
use http_proxy::exchange::{Exchange, RecordId};

use crate::ui::style::theme::Palette;

/// Wait after the last keystroke before a search starts, so typing a word
/// does not start a search for every letter.
const DEBOUNCE: Duration = Duration::from_millis(150);
/// Wait before retrying when the search queue is full.
const RETRY: Duration = Duration::from_millis(50);

/// What the user asked to search for.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct Settings {
    text: String,
    scope: SearchScope,
    options: MatchOptions,
}

impl Settings {
    /// Reports whether every record matching `self` also matches
    /// `previous`, as when letters are added to a word. Records that
    /// `previous` ruled out then need no new check.
    fn narrows(&self, previous: &Self) -> bool {
        if self.options.regex || self.options != previous.options || self.scope != previous.scope {
            return false;
        }
        if self.options.case_sensitive {
            self.text.contains(&previous.text)
        } else {
            self.text
                .to_lowercase()
                .contains(&previous.text.to_lowercase())
        }
    }
}

struct Active {
    settings: Settings,
    query: Arc<Query>,
}

/// Counts shown under the controls.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Summary {
    total: usize,
    checked: usize,
    matched: usize,
    partial: usize,
    binary: usize,
    undecodable: usize,
}

/// Background search worker, or an inline fallback when thread creation fails.
enum SearchBackend {
    Worker(SearchWorker),
    Inline { notice: String, cache: BodyCache },
}

pub(in crate::ui) struct RequestSearch {
    text: String,
    scope: SearchScope,
    options: MatchOptions,
    /// Settings seen last frame, to notice edits.
    seen: Settings,
    changed_at: Option<Instant>,
    active: Option<Active>,
    error: Option<String>,
    generation: u64,
    results: HashMap<RecordId, Checked>,
    /// Records sent to the worker for the active query and not reported yet.
    queued: HashSet<RecordId>,
    summary: Summary,
    backend: SearchBackend,
}

impl RequestSearch {
    pub(in crate::ui) fn new(context: &egui::Context) -> Self {
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
            text: String::new(),
            scope: SearchScope::All,
            options: MatchOptions::default(),
            seen: Settings::default(),
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

    /// The search text box, scope, and options.
    pub(in crate::ui) fn controls(&mut self, ui: &mut egui::Ui) {
        let search_width = ui.available_width().clamp(120.0, 260.0);
        ui.add(
            egui::TextEdit::singleline(&mut self.text)
                .desired_width(search_width)
                .hint_text(scope_hint(self.scope)),
        );
        egui::ComboBox::from_id_salt("search-scope")
            .selected_text(self.scope.label())
            .show_ui(ui, |ui| {
                for scope in SearchScope::ALL {
                    ui.selectable_value(&mut self.scope, scope, scope.label());
                }
            })
            .response
            .on_hover_text("Where the filter text is looked for");
        ui.toggle_value(&mut self.options.case_sensitive, "Aa")
            .on_hover_text("Match case");
        ui.toggle_value(&mut self.options.regex, ".*")
            .on_hover_text("Use a regular expression");
    }

    /// Progress, match count, and what was not searched. Draws nothing
    /// while no search is active.
    pub(in crate::ui) fn status(&self, ui: &mut egui::Ui) {
        let palette = Palette::of(ui);
        if let SearchBackend::Inline { notice, .. } = &self.backend {
            ui.colored_label(palette.warning, notice);
        }
        if let Some(error) = &self.error {
            ui.colored_label(palette.error, error);
            return;
        }
        let Some(active) = &self.active else {
            return;
        };
        let summary = self.summary;
        ui.horizontal_wrapped(|ui| {
            if summary.checked < summary.total {
                ui.spinner();
                ui.label(format!(
                    "Searching… {} of {} checked · {} match",
                    summary.checked, summary.total, summary.matched
                ));
            } else {
                ui.label(format!(
                    "{} of {} requests match",
                    summary.matched, summary.total
                ));
            }
        });
        if active.settings.scope.covers_bodies()
            && let Some(text) = coverage_text(summary)
        {
            ui.colored_label(palette.warning, text);
        }
    }

    /// Applies edits after the debounce, collects results, and queues new
    /// or newly settled records. Call once per frame before [`Self::shows`].
    pub(in crate::ui) fn update(
        &mut self,
        records: &RecordList<Exchange>,
        context: &egui::Context,
    ) {
        let settings = Settings {
            text: self.text.clone(),
            scope: self.scope,
            options: self.options,
        };
        let now = Instant::now();
        if settings != self.seen {
            self.seen = settings.clone();
            self.changed_at = Some(now);
        }
        if let Some(changed_at) = self.changed_at {
            let wait = if settings.text.is_empty() {
                Duration::ZERO
            } else {
                DEBOUNCE
            };
            let elapsed = now.duration_since(changed_at);
            if elapsed >= wait {
                self.changed_at = None;
                self.apply(settings);
            } else {
                context.request_repaint_after(wait.saturating_sub(elapsed));
            }
        }
        self.receive();
        self.forget_missing(records);
        if !self.submit(records) {
            context.request_repaint_after(RETRY);
        }
        self.summary = self.summarize(records);
    }

    /// Reports whether the list shows the record with `id`.
    pub(in crate::ui) fn shows(&self, id: RecordId) -> bool {
        self.active.is_none() || self.results.get(&id).is_some_and(|checked| checked.matched)
    }

    /// The query whose matches the detail panel highlights.
    pub(in crate::ui) fn active_query(&self) -> Option<&Arc<Query>> {
        self.active.as_ref().map(|active| &active.query)
    }

    fn apply(&mut self, settings: Settings) {
        let finished = self.queued.is_empty();
        self.generation += 1;
        if let SearchBackend::Worker(worker) = &self.backend {
            worker.supersede(self.generation);
        }
        self.queued.clear();
        self.error = None;
        let previous = self.active.take();
        if settings.text.is_empty() {
            self.results.clear();
            return;
        }
        let matcher = match Matcher::new(&settings.text, settings.options) {
            Ok(matcher) => matcher,
            Err(error) => {
                self.error = Some(capitalized(&error.to_string()));
                self.results.clear();
                return;
            }
        };
        if finished && previous.is_some_and(|previous| settings.narrows(&previous.settings)) {
            self.results
                .retain(|_, checked| checked.settled && !checked.matched);
        } else {
            self.results.clear();
        }
        let query = Arc::new(Query {
            generation: self.generation,
            matcher,
            scope: settings.scope,
        });
        self.active = Some(Active { settings, query });
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
                self.results.insert(checked.id, checked);
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

    /// Queues records without a final result. Returns `false` when the
    /// queue was full and the records must be queued again later.
    fn submit(&mut self, records: &RecordList<Exchange>) -> bool {
        let Some(query) = self.active.as_ref().map(|active| Arc::clone(&active.query)) else {
            return true;
        };
        let pending: Vec<Exchange> = records
            .iter()
            .filter(|record| !self.queued.contains(&record.id()))
            .filter(|record| {
                self.results
                    .get(&record.id())
                    .is_none_or(|checked| !checked.settled && document::is_settled(record))
            })
            .cloned()
            .collect();
        if pending.is_empty() {
            return true;
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
                    let checked = document::check(record, &query.matcher, query.scope, cache);
                    self.results.insert(checked.id, checked);
                }
            }
        }
        true
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
            let Some(checked) = self.results.get(&record.id()) else {
                continue;
            };
            if !self.queued.contains(&checked.id) {
                summary.checked += 1;
            }
            if checked.matched {
                summary.matched += 1;
            } else {
                summary.partial += usize::from(checked.coverage.partial);
                summary.binary += usize::from(checked.coverage.binary);
                summary.undecodable += usize::from(checked.coverage.undecodable);
            }
        }
        summary
    }

    /// Records that did not match although part of a body was not searched.
    #[cfg(test)]
    pub(in crate::ui) const fn partially_searched(&self) -> usize {
        self.summary.partial
    }

    /// Runs a search to completion, skipping the debounce.
    #[cfg(test)]
    pub(in crate::ui) fn search_now(
        &mut self,
        (text, scope, options): (&str, SearchScope, MatchOptions),
        records: &RecordList<Exchange>,
        context: &egui::Context,
    ) {
        text.clone_into(&mut self.text);
        self.scope = scope;
        self.options = options;
        self.update(records, context);
        self.changed_at = None;
        self.apply(self.seen.clone());
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

/// Says which records that did not match were searched only in part.
fn coverage_text(summary: Summary) -> Option<String> {
    let mut parts = Vec::new();
    if summary.partial > 0 {
        parts.push(format!(
            "{} only in their first {} KiB",
            summary.partial,
            http_proxy::interception::BODY_PREVIEW_BYTES / 1024
        ));
    }
    if summary.binary > 0 {
        parts.push(format!("{} binary, skipped", summary.binary));
    }
    if summary.undecodable > 0 {
        parts.push(format!("{} could not be decoded", summary.undecodable));
    }
    (!parts.is_empty()).then(|| {
        format!(
            "Bodies are searched in the captured preview, after decompression. Requests that did not match but whose bodies were not fully searched: {}. The full-session log keeps the rest.",
            parts.join(", ")
        )
    })
}

const fn scope_hint(scope: SearchScope) -> &'static str {
    match scope {
        SearchScope::All => "Filter: method, URL, status, header, or body",
        SearchScope::Url => "Filter: URL",
        SearchScope::Headers => "Filter: header name or value",
        SearchScope::RequestBody => "Filter: request body",
        SearchScope::ResponseBody => "Filter: response body",
    }
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

    fn settings(text: &str, case_sensitive: bool, regex: bool) -> Settings {
        Settings {
            text: text.to_owned(),
            scope: SearchScope::All,
            options: MatchOptions {
                case_sensitive,
                regex,
            },
        }
    }

    #[test]
    fn adding_letters_narrows_but_removing_them_does_not() {
        let previous = settings("Ord", false, false);
        assert!(settings("xorder", false, false).narrows(&previous));
        assert!(!settings("Or", false, false).narrows(&previous));
        assert!(!settings("order", true, false).narrows(&previous));
        assert!(!settings("Order", false, true).narrows(&settings("Ord", false, true)));
        assert!(!settings("order", true, false).narrows(&settings("Ord", true, false)));
        let mut other_scope = settings("Order", false, false);
        other_scope.scope = SearchScope::Url;
        assert!(!other_scope.narrows(&previous));
    }

    #[test]
    fn coverage_lists_only_what_was_not_fully_searched() {
        assert_eq!(coverage_text(Summary::default()), None);
        let text = coverage_text(Summary {
            partial: 2,
            binary: 1,
            ..Summary::default()
        })
        .unwrap_or_default();
        assert!(
            text.contains("2 only in their first 8 KiB, 1 binary, skipped"),
            "{text}"
        );
    }
}
