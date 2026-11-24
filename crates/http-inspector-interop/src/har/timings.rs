//! HAR `startedDateTime`, `time`, and `timings` of one exchange.

use std::time::{Duration, SystemTime};

use http_inspector_exchange::{Exchange, Outcome};
use serde_json::{Value, json};

/// Phases of one entry that add up to its total time. `None` is written as
/// `-1`, the HAR value for a phase that was not observed.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct Phases {
    blocked: Option<Duration>,
    connect: Option<Duration>,
    send: Duration,
    wait: Duration,
    receive: Duration,
}

impl Phases {
    /// Splits the exchange's observed time as of `now` into HAR phases.
    ///
    /// Every phase is cut to what is left of the time until response headers
    /// (or settlement), so the phases never overlap and always add up to the
    /// total. Time that no measured phase covers goes to `blocked` for a
    /// response and to `wait` for every other outcome.
    pub(super) fn of(record: &Exchange, now: SystemTime) -> Self {
        let timing = record.timing();
        let so_far = now.duration_since(record.started_at()).unwrap_or_default();
        let headers = timing.settled().unwrap_or(so_far);
        let end = match record.outcome() {
            Outcome::Response(_) => timing.complete().unwrap_or(so_far).max(headers),
            Outcome::Tunnel(tunnel) => headers.saturating_add(tunnel.snapshot().duration()),
            Outcome::InFlight
            | Outcome::InspectionEstablished
            | Outcome::Failed { .. }
            | Outcome::Cancelled
            | Outcome::Incomplete => headers,
        };
        let connect = timing.connect().map(|connect| connect.min(headers));
        let mut left = headers.saturating_sub(connect.unwrap_or_default());
        let receive = end.saturating_sub(headers);
        if !matches!(record.outcome(), Outcome::Response(_)) {
            return Self {
                blocked: None,
                connect,
                send: Duration::ZERO,
                wait: left,
                receive,
            };
        }
        let send = timing.send().unwrap_or_default().min(left);
        left = left.saturating_sub(send);
        let wait = timing.wait().unwrap_or_default().min(left);
        Self {
            blocked: Some(left.saturating_sub(wait)),
            connect,
            send,
            wait,
            receive,
        }
    }

    /// Returns the sum of all observed phases, the HAR entry `time`.
    pub(super) fn total(&self) -> Duration {
        [self.blocked, self.connect].into_iter().flatten().fold(
            self.send + self.wait + self.receive,
            Duration::saturating_add,
        )
    }

    pub(super) fn to_json(self) -> Value {
        let optional =
            |phase: Option<Duration>| phase.map_or_else(|| json!(-1), |value| json!(millis(value)));
        json!({
            "blocked": optional(self.blocked),
            "dns": -1,
            "connect": optional(self.connect),
            "send": millis(self.send),
            "wait": millis(self.wait),
            "receive": millis(self.receive),
            "ssl": -1,
        })
    }
}

/// Converts a duration to milliseconds with microsecond precision, without
/// the rounding noise of floating-point multiplication.
pub(super) fn millis(duration: Duration) -> f64 {
    let micros = duration.as_micros();
    let whole = u32::try_from(micros / 1_000).unwrap_or(u32::MAX);
    let fraction = u32::try_from(micros % 1_000).unwrap_or_default();
    f64::from(whole) + f64::from(fraction) / 1_000.0
}

/// Formats a wall-clock time as ISO 8601 in UTC with milliseconds.
pub(super) fn iso8601(at: SystemTime) -> String {
    let at = time::OffsetDateTime::from(at);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        at.year(),
        u8::from(at.month()),
        at.day(),
        at.hour(),
        at.minute(),
        at.second(),
        at.millisecond(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_utc_time_with_milliseconds() {
        let at = SystemTime::UNIX_EPOCH + Duration::from_millis(1_700_000_000_123);
        assert_eq!(iso8601(at), "2023-11-14T22:13:20.123Z");
        assert_eq!(iso8601(SystemTime::UNIX_EPOCH), "1970-01-01T00:00:00.000Z");
    }

    #[test]
    fn converts_microseconds_exactly() {
        assert!((millis(Duration::from_micros(12_345)) - 12.345).abs() < f64::EPSILON);
        assert!(millis(Duration::ZERO).abs() < f64::EPSILON);
    }

    #[test]
    fn total_adds_only_observed_phases() {
        let phases = Phases {
            blocked: None,
            connect: Some(Duration::from_millis(2)),
            send: Duration::from_millis(1),
            wait: Duration::from_millis(3),
            receive: Duration::from_millis(4),
        };
        assert_eq!(phases.total(), Duration::from_millis(10));
        let json = phases.to_json();
        assert_eq!(json.get("blocked"), Some(&json!(-1)));
        assert_eq!(json.get("dns"), Some(&json!(-1)));
        assert_eq!(json.get("ssl"), Some(&json!(-1)));
    }
}
