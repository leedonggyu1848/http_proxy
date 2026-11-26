//! The search controls above the request list and the state that decides
//! which records the list shows.

use crate::ui::style::tokens::Layout;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use http_inspector_analysis::filter::RecordList;
use http_inspector_analysis::search::document::{self, BodyCache, Checked};
use http_inspector_analysis::search::{MatchOptions, Matcher, Query, SearchScope, SearchWorker};
use http_inspector_exchange::{Exchange, RecordId};

use crate::ui::i18n::{Language, domain};
use crate::ui::style::indicator;
use crate::ui::style::tokens::Tone;

/// Wait after the last keystroke before a search starts, so typing a word
/// does not start a search for every letter.
const DEBOUNCE: Duration = Duration::from_millis(150);
/// Wait before retrying a locked preview or a full search queue.
const RETRY: Duration = Duration::from_millis(50);

/// What the user asked to search for.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct SearchSettings {
    text: String,
    scope: SearchScope,
    options: MatchOptions,
}

impl SearchSettings {
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
    settings: SearchSettings,
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
    matched: usize,
    partial: usize,
    binary: usize,
    undecodable: usize,
}

/// Background search worker, or an inline fallback when thread creation fails.
enum SearchBackend {
    Worker(SearchWorker),
    Inline { reason: String, cache: BodyCache },
}

pub(crate) struct RequestSearch {
    text: String,
    scope: SearchScope,
    options: MatchOptions,
    /// Settings seen last frame, to notice edits.
    seen: SearchSettings,
    changed_at: Option<Instant>,
    active: Option<Active>,
    error: Option<http_inspector_analysis::search::MatcherError>,
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
                reason: error.to_string(),
                cache: BodyCache::default(),
            },
        };
        Self {
            text: String::new(),
            scope: SearchScope::All,
            options: MatchOptions::default(),
            seen: SearchSettings::default(),
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
    pub(crate) fn controls(&self, ui: &mut egui::Ui, lang: Language) -> Option<SearchSettings> {
        let mut settings = self.settings();
        let search_width = Layout::REQUEST_SEARCH_WIDTH.fit(ui.available_width());
        ui.add(
            egui::TextEdit::singleline(&mut settings.text)
                .desired_width(search_width)
                .hint_text(scope_hint(self.scope, lang)),
        );
        egui::ComboBox::from_id_salt("search-scope")
            .selected_text(domain::search_scope(lang, settings.scope))
            .show_ui(ui, |ui| {
                for scope in SearchScope::ALL {
                    ui.selectable_value(
                        &mut settings.scope,
                        scope,
                        domain::search_scope(lang, scope),
                    );
                }
            })
            .response
            .on_hover_text(lang.pick(
                "Where the filter text is looked for",
                "필터 검색어를 찾을 범위",
            ));
        ui.toggle_value(&mut settings.options.case_sensitive, "Aa")
            .on_hover_text(lang.pick("Match case", "대소문자 구분"));
        ui.toggle_value(&mut settings.options.regex, ".*")
            .on_hover_text(lang.pick("Use a regular expression", "정규식 사용"));
        (settings.text != self.text
            || settings.scope != self.scope
            || settings.options != self.options)
            .then_some(settings)
    }

    pub(crate) fn set_settings(&mut self, settings: SearchSettings) {
        self.text = settings.text;
        self.scope = settings.scope;
        self.options = settings.options;
    }

    fn settings(&self) -> SearchSettings {
        SearchSettings {
            text: self.text.clone(),
            scope: self.scope,
            options: self.options,
        }
    }

    /// Progress, match count, and what was not searched. Draws nothing
    /// while no search is active.
    pub(crate) fn status(&self, ui: &mut egui::Ui, lang: Language) {
        if let SearchBackend::Inline { reason, .. } = &self.backend {
            let notice = match lang {
                Language::English => {
                    format!(
                        "Background search is unavailable ({reason}); searching on the UI thread."
                    )
                }
                Language::Korean => {
                    format!("백그라운드 검색을 사용할 수 없어({reason}) UI 스레드에서 검색합니다.")
                }
            };
            indicator::dot_label(ui, Tone::Warning, notice);
        }
        if let Some(error) = &self.error {
            indicator::dot_label(ui, Tone::Danger, domain::matcher_error(lang, error));
            return;
        }
        let Some(active) = &self.active else {
            return;
        };
        let summary = self.summary;
        ui.horizontal_wrapped(|ui| {
            if summary.checked < summary.total {
                ui.spinner();
                let label = match lang {
                    Language::English => format!(
                        "Searching… {} of {} checked · {} match",
                        summary.checked, summary.total, summary.matched
                    ),
                    Language::Korean => format!(
                        "검색 중… {}/{}건 확인 · {}건 일치",
                        summary.checked, summary.total, summary.matched
                    ),
                };
                ui.label(label);
            } else {
                let label = match lang {
                    Language::English => {
                        format!("{} of {} requests match", summary.matched, summary.total)
                    }
                    Language::Korean => {
                        format!("요청 {}건 중 {}건 일치", summary.total, summary.matched)
                    }
                };
                ui.label(label);
            }
        });
        if active.settings.scope.covers_bodies()
            && let Some(text) = coverage_text(summary, lang)
        {
            indicator::dot_label(ui, Tone::Warning, text);
        }
    }

    /// Applies edits after the debounce, collects results, and queues new
    /// or newly settled records. Call once per frame before [`Self::shows`].
    pub(crate) fn update(&mut self, records: &RecordList<Exchange>, context: &egui::Context) {
        let settings = self.settings();
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

    fn apply(&mut self, settings: SearchSettings) {
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
                self.error = Some(error);
                self.results.clear();
                return;
            }
        };
        if finished && previous.is_some_and(|previous| settings.narrows(&previous.settings)) {
            self.results
                .retain(|_, result| result.checked.settled && !result.checked.matched);
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
                    let checked = document::check(record, &query.matcher, query.scope, cache);
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
fn coverage_text(summary: Summary, lang: Language) -> Option<String> {
    let mut parts = Vec::new();
    let preview_kib = http_inspector_exchange::capture::BODY_PREVIEW_BYTES / 1024;
    if summary.partial > 0 {
        parts.push(match lang {
            Language::English => {
                format!("{} only in their first {preview_kib} KiB", summary.partial)
            }
            Language::Korean => format!("{}건은 처음 {preview_kib} KiB만 검색", summary.partial),
        });
    }
    if summary.binary > 0 {
        parts.push(match lang {
            Language::English => format!("{} binary, skipped", summary.binary),
            Language::Korean => format!("{}건은 바이너리여서 건너뜀", summary.binary),
        });
    }
    if summary.undecodable > 0 {
        parts.push(match lang {
            Language::English => format!("{} could not be decoded", summary.undecodable),
            Language::Korean => format!("{}건은 디코딩할 수 없음", summary.undecodable),
        });
    }
    (!parts.is_empty()).then(|| match lang {
        Language::English => format!(
            "Bodies are searched in the captured preview, after decompression. Requests that did not match but whose bodies were not fully searched: {}. The full-session log keeps the rest.",
            parts.join(", ")
        ),
        Language::Korean => format!(
            "본문은 압축 해제 후 캡처된 미리보기 범위에서 검색합니다. 일치하지 않았지만 전체 본문을 검색하지 못한 요청: {}. 나머지 내용은 전체 세션 로그에 보관됩니다.",
            parts.join(", ")
        ),
    })
}

const fn scope_hint(scope: SearchScope, lang: Language) -> &'static str {
    match scope {
        SearchScope::All => lang.pick(
            "Filter: method, URL, status, header, or body",
            "필터: 메서드, URL, 상태, 헤더 또는 본문",
        ),
        SearchScope::Url => lang.pick("Filter: URL", "필터: URL"),
        SearchScope::Headers => {
            lang.pick("Filter: header name or value", "필터: 헤더 이름 또는 값")
        }
        SearchScope::RequestBody => lang.pick("Filter: request body", "필터: 요청 본문"),
        SearchScope::ResponseBody => lang.pick("Filter: response body", "필터: 응답 본문"),
    }
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
        assert_eq!(coverage_text(Summary::default(), Language::English), None);
        let text = coverage_text(
            Summary {
                partial: 2,
                binary: 1,
                ..Summary::default()
            },
            Language::English,
        )
        .unwrap_or_default();
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
            reason: String::new(),
            cache: BodyCache::default(),
        };
        let mut query = settings("needle", false, false);
        query.scope = SearchScope::RequestBody;
        search.apply(query);
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
}
