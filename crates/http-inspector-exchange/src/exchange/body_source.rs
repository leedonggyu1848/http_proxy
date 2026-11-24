//! Port through which analysis reads complete captured bodies.
//!
//! Analysis code depends on [`BodySource`] instead of a concrete store, so
//! the store's format and read API can change without touching analysis,
//! and analysis can be tested without a proxy or a disk store.

use super::{BodySide, Exchange};

/// Outcome of reading and decoding a complete body within a byte limit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReadBodyOutcome {
    /// Complete decoded bytes within the requested size cap.
    Complete(Vec<u8>),
    /// Some bytes of the body are missing: the stream was interrupted or
    /// cancelled, is still arriving, or the store skipped later bytes.
    Incomplete,
    /// The decoded body is larger than the requested size cap.
    TooLarge,
    /// The body could not be decompressed due to invalid or unsupported `Content-Encoding`.
    DecodeFailed,
    /// The store could not be read (writer stopped or file I/O failed).
    Unavailable,
    /// No stored body was found for this exchange side.
    NotFound,
}

/// Reads one complete body of a finished exchange.
///
/// Implementations return the body with every `Content-Encoding` layer
/// removed and stop decoding once more than `max_decoded_bytes` would be
/// produced, reporting [`ReadBodyOutcome::TooLarge`] instead. Decoding
/// belongs to the implementation because only it knows where the bytes
/// are: a disk store streams and decodes them without loading the encoded
/// body into memory first.
///
/// A read may wait for a writer or read a file, so call it only from a
/// background thread.
pub trait BodySource: Send + Sync {
    /// Reads the `side` body of `record`, decoded and bounded by `max_decoded_bytes`.
    fn read(&self, record: &Exchange, side: BodySide, max_decoded_bytes: usize) -> ReadBodyOutcome;
}
