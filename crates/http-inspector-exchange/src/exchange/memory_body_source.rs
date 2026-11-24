//! A [`BodySource`] that answers from bodies kept in memory, for tests.
//!
//! Compiled only with the `test-support` feature, which release builds leave
//! off. It lets analysis be tested with exchanges from `ExchangeBuilder` and
//! without a proxy or a disk store.

use std::collections::HashMap;

use super::{BodySide, BodySource, Exchange, ReadBodyOutcome, RecordId};

/// Answers [`BodySource::read`] from outcomes registered per exchange side.
///
/// Bodies are registered already decoded, as every source returns them.
/// A registered body longer than the caller's cap reads as
/// [`ReadBodyOutcome::TooLarge`], and a side with nothing registered reads
/// as [`ReadBodyOutcome::NotFound`].
///
/// ```
/// use http_proxy::exchange::{BodySide, BodySource, MemoryBodySource, ReadBodyOutcome};
/// use http_proxy::interception::{ExchangeBuilder, FixtureError};
///
/// # fn main() -> Result<(), FixtureError> {
/// let exchange = ExchangeBuilder::http(http::Method::POST, "http://example.test/")?.cancel()?;
/// let bodies = MemoryBodySource::new().with_body(exchange.id(), BodySide::Request, "hello");
/// assert_eq!(
///     bodies.read(&exchange, BodySide::Request, 16),
///     ReadBodyOutcome::Complete(b"hello".to_vec())
/// );
/// assert_eq!(bodies.read(&exchange, BodySide::Request, 2), ReadBodyOutcome::TooLarge);
/// assert_eq!(bodies.read(&exchange, BodySide::Response, 16), ReadBodyOutcome::NotFound);
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Debug, Default)]
pub struct MemoryBodySource {
    bodies: HashMap<(RecordId, BodySide), ReadBodyOutcome>,
}

impl MemoryBodySource {
    /// Creates a source with no bodies.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers the complete decoded `body` of one side of `record`.
    #[must_use]
    pub fn with_body(self, record: RecordId, side: BodySide, body: impl Into<Vec<u8>>) -> Self {
        self.with_outcome(record, side, ReadBodyOutcome::Complete(body.into()))
    }

    /// Registers the outcome every read of one side of `record` returns,
    /// such as [`ReadBodyOutcome::DecodeFailed`].
    #[must_use]
    pub fn with_outcome(
        mut self,
        record: RecordId,
        side: BodySide,
        outcome: ReadBodyOutcome,
    ) -> Self {
        let _previous = self.bodies.insert((record, side), outcome);
        self
    }
}

impl BodySource for MemoryBodySource {
    fn read(&self, record: &Exchange, side: BodySide, max_decoded_bytes: usize) -> ReadBodyOutcome {
        match self.bodies.get(&(record.id(), side)) {
            None => ReadBodyOutcome::NotFound,
            Some(ReadBodyOutcome::Complete(bytes)) if bytes.len() > max_decoded_bytes => {
                ReadBodyOutcome::TooLarge
            }
            Some(outcome) => outcome.clone(),
        }
    }
}
