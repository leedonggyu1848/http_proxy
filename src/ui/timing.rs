//! Shared timing view model used by both the inspector's Timing tab and the
//! request list's Waterfall column so both views report identical phase spans
//! and durations.

use std::time::{Duration, Instant};

use http_inspector_exchange::{Exchange, Outcome, PhaseSpan};

use crate::ui::i18n::Language;
use crate::ui::requests::outcome_view;
use crate::ui::workbench::text;

/// One sequential timing phase that can appear in the Timing tab and on a
/// Waterfall bar.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TimingPhase {
    Connect,
    Send,
    Wait,
    Receive,
}

impl TimingPhase {
    /// Phases in chronological request lifecycle order.
    pub(crate) const ALL: [Self; 4] = [Self::Connect, Self::Send, Self::Wait, Self::Receive];

    /// Localized phase label shared by the Timing tab and Waterfall hover table.
    pub(crate) const fn label(self, lang: Language) -> &'static str {
        let requests = lang.requests();
        match self {
            Self::Connect => requests.connect,
            Self::Send => requests.send,
            Self::Wait => requests.wait,
            Self::Receive => requests.receive,
        }
    }
}

/// One displayed row of timing information (`Connect`, `Send`, `Wait (TTFB)`,
/// `Receive`, settlement, or `Total`). `millis` is `None` when unobserved.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TimingEntry {
    pub(crate) phase: Option<TimingPhase>,
    pub(crate) label: &'static str,
    pub(crate) millis: Option<f64>,
    /// True while the request is still in flight and the time is growing.
    pub(crate) running: bool,
}

impl TimingEntry {
    /// Formats this entry's numeric time, or `—` when unobserved.
    pub(crate) fn value_text(&self, lang: Language) -> String {
        self.millis.map_or_else(
            || lang.shared().not_observed.to_owned(),
            |millis| {
                if self.running {
                    text::milliseconds_running(millis)
                } else {
                    text::milliseconds(millis)
                }
            },
        )
    }
}

/// Consistent timing snapshot of one exchange at a frame timestamp.
///
/// Unobserved phases stay `None` rather than zero so both the Timing tab and
/// the Waterfall column render them as missing/blank intervals.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ExchangeTiming {
    connect: Option<PhaseSpan>,
    send: Option<PhaseSpan>,
    wait: Option<PhaseSpan>,
    receive: Option<PhaseSpan>,
    settled: Option<Duration>,
    complete: Option<Duration>,
    in_flight: bool,
    live_elapsed: Option<Duration>,
}

impl ExchangeTiming {
    /// Captures the exchange's timing state as of `now`.
    pub(crate) fn from_exchange(record: &Exchange, now: Instant) -> Self {
        let timing = record.timing();
        let in_flight = record.outcome().is_in_flight();
        let elapsed_since_start = now.saturating_duration_since(record.started());
        let live_elapsed = if in_flight {
            Some(elapsed_since_start)
        } else if timing.complete().is_none() && matches!(record.outcome(), Outcome::Response(_)) {
            Some(elapsed_since_start.max(timing.settled().unwrap_or_default()))
        } else {
            None
        };
        Self {
            connect: timing.connect_span(),
            send: timing.send_span(),
            wait: timing.wait_span(),
            receive: timing.receive_span(),
            settled: timing.settled(),
            complete: timing.complete(),
            in_flight,
            live_elapsed,
        }
    }

    #[cfg(test)]
    #[expect(
        clippy::too_many_arguments,
        reason = "test constructor builds deterministic timing fixtures directly"
    )]
    pub(crate) const fn from_parts(
        connect: Option<PhaseSpan>,
        send: Option<PhaseSpan>,
        wait: Option<PhaseSpan>,
        receive: Option<PhaseSpan>,
        settled: Option<Duration>,
        complete: Option<Duration>,
        in_flight: bool,
        live_elapsed: Option<Duration>,
    ) -> Self {
        Self {
            connect,
            send,
            wait,
            receive,
            settled,
            complete,
            in_flight,
            live_elapsed,
        }
    }

    /// Returns the observed span of `phase`, or `None` when unobserved.
    pub(crate) const fn span(&self, phase: TimingPhase) -> Option<PhaseSpan> {
        match phase {
            TimingPhase::Connect => self.connect,
            TimingPhase::Send => self.send,
            TimingPhase::Wait => self.wait,
            TimingPhase::Receive => self.receive,
        }
    }

    /// Reports whether the exchange is still progressing (`InFlight` or
    /// streaming its response body).
    pub(crate) const fn is_running(&self) -> bool {
        self.live_elapsed.is_some()
    }

    /// Returns the furthest observed offset from request start for timeline
    /// scaling and waterfall bar bounds.
    pub(crate) fn observed_end(&self) -> Duration {
        let phase_end = TimingPhase::ALL
            .into_iter()
            .filter_map(|phase| self.span(phase).map(|span| span.end()))
            .max()
            .unwrap_or_default();
        self.complete
            .into_iter()
            .chain(self.settled)
            .chain(self.live_elapsed)
            .fold(phase_end, Duration::max)
    }

    /// Builds the six display rows shared by the Timing tab and Waterfall
    /// hover tooltip.
    pub(crate) fn entries(&self, outcome: &Outcome, lang: Language) -> [TimingEntry; 6] {
        let millis = |duration: Option<Duration>| duration.map(outcome_view::elapsed_millis);
        let settled = if self.in_flight {
            millis(self.live_elapsed)
        } else {
            millis(self.settled)
        };
        [
            self.phase_entry(TimingPhase::Connect, lang),
            self.phase_entry(TimingPhase::Send, lang),
            self.phase_entry(TimingPhase::Wait, lang),
            self.phase_entry(TimingPhase::Receive, lang),
            TimingEntry {
                phase: None,
                label: outcome_view::timing_label(outcome, lang),
                millis: settled,
                running: self.in_flight,
            },
            TimingEntry {
                phase: None,
                label: lang.requests().total,
                millis: millis(self.complete),
                running: false,
            },
        ]
    }

    fn phase_entry(&self, phase: TimingPhase, lang: Language) -> TimingEntry {
        TimingEntry {
            phase: Some(phase),
            label: phase.label(lang),
            millis: self
                .span(phase)
                .map(|span| outcome_view::elapsed_millis(span.duration())),
            running: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::{Method, StatusCode};
    use http_inspector_exchange::{BodyLifecycle, ExchangeBuilder, FailureStage, ResponseFixture};

    #[test]
    fn keeps_unobserved_phases_none_in_entries_and_spans() -> Result<(), Box<dyn std::error::Error>>
    {
        let failed = ExchangeBuilder::http(Method::GET, "http://example.test/fail")?
            .fail(FailureStage::Connect, "refused")?;
        let now = Instant::now();
        let timing = ExchangeTiming::from_exchange(&failed, now);
        for phase in TimingPhase::ALL {
            assert_eq!(timing.span(phase), None);
        }
        let entries = timing.entries(failed.outcome(), Language::English);
        for entry in entries.iter().take(4) {
            assert_eq!(entry.millis, None);
            assert_eq!(entry.value_text(Language::English), "—");
        }
        assert!(entries.get(4).is_some_and(|entry| entry.millis.is_some()));
        assert!(entries.get(5).is_some_and(|entry| entry.millis.is_some()));
        assert!(!timing.is_running());
        Ok(())
    }

    #[test]
    fn tracks_in_flight_and_streaming_responses_as_running()
    -> Result<(), Box<dyn std::error::Error>> {
        let pending =
            ExchangeBuilder::http(Method::GET, "http://example.test/pending")?.in_flight()?;
        let future = pending.started() + Duration::from_millis(45);
        let pending_timing = ExchangeTiming::from_exchange(&pending, future);
        assert!(pending_timing.is_running());
        assert_eq!(pending_timing.observed_end(), Duration::from_millis(45));
        let entries = pending_timing.entries(pending.outcome(), Language::English);
        assert_eq!(
            entries
                .get(4)
                .map(|entry| entry.value_text(Language::English)),
            Some("45 ms…".to_owned())
        );
        assert_eq!(entries.get(5).and_then(|entry| entry.millis), None);

        let streaming = ExchangeBuilder::http(Method::GET, "http://example.test/stream")?
            .connect_time(Duration::from_millis(5))
            .respond(
                ResponseFixture::new(StatusCode::OK).body("chunk", BodyLifecycle::Streaming),
            )?;
        let streaming_future = streaming.started() + Duration::from_millis(80);
        let streaming_timing = ExchangeTiming::from_exchange(&streaming, streaming_future);
        assert!(streaming_timing.is_running());
        assert_eq!(streaming_timing.span(TimingPhase::Receive), None);
        assert_eq!(streaming_timing.observed_end(), Duration::from_millis(80));
        Ok(())
    }

    #[test]
    fn observed_end_takes_furthest_of_complete_settled_and_phase_spans() {
        let timing = ExchangeTiming::from_parts(
            Some(PhaseSpan::new(Duration::ZERO, Duration::from_millis(10))),
            Some(PhaseSpan::new(
                Duration::from_millis(12),
                Duration::from_millis(4),
            )),
            None,
            Some(PhaseSpan::new(
                Duration::from_millis(20),
                Duration::from_millis(15),
            )),
            Some(Duration::from_millis(20)),
            Some(Duration::from_millis(35)),
            false,
            None,
        );
        assert_eq!(timing.observed_end(), Duration::from_millis(35));
        assert_eq!(
            timing.span(TimingPhase::Connect),
            Some(PhaseSpan::new(Duration::ZERO, Duration::from_millis(10)))
        );
        assert_eq!(timing.span(TimingPhase::Wait), None);
    }
}
