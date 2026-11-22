//! In-memory index from a body to the byte ranges that hold it in the data file.

use std::collections::HashMap;
use std::fmt;

use crate::engine::domain::BodyLifecycle;
use crate::engine::journal::BodySide;

/// Identifies one body: the record it belongs to and its direction.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(in crate::engine) struct BodyKey {
    pub(in crate::engine) record: u64,
    pub(in crate::engine) side: BodySide,
}

/// Why some bytes of a body are missing from the body store.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoreGap {
    /// The writer fell behind and its bounded queue was full.
    QueueFull,
    /// The session store limit was reached.
    SessionLimit,
    /// Free disk space dropped below the configured minimum.
    LowDiskSpace,
    /// Writing to the store failed.
    WriteFailed,
    /// The store was closed while the body was still arriving.
    StoreClosed,
}

impl fmt::Display for StoreGap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::QueueFull => "the body store queue was full",
            Self::SessionLimit => "the body store reached its session limit",
            Self::LowDiskSpace => "free disk space was below the minimum",
            Self::WriteFailed => "writing to the body store failed",
            Self::StoreClosed => "the body store was closed",
        })
    }
}

/// What the body store holds for one body.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StoredBody {
    stored_bytes: u64,
    lifecycle: BodyLifecycle,
    gap: Option<StoreGap>,
}

impl StoredBody {
    /// Returns the body bytes written to the store so far.
    #[must_use]
    pub const fn stored_bytes(&self) -> u64 {
        self.stored_bytes
    }

    /// Returns how the forwarded body stream ended, as far as it is known.
    #[must_use]
    pub const fn lifecycle(&self) -> BodyLifecycle {
        self.lifecycle
    }

    /// Returns why later bytes of the body were not stored, if any were lost.
    #[must_use]
    pub const fn gap(&self) -> Option<StoreGap> {
        self.gap
    }

    /// Reports whether the store holds every byte of a body that ended normally.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.lifecycle == BodyLifecycle::Complete && self.gap.is_none()
    }

    /// Explains why the stored body is incomplete, or `None` when it is complete.
    #[must_use]
    pub fn incomplete_reason(&self) -> Option<String> {
        if let Some(gap) = self.gap {
            return Some(format!("Later bytes are missing because {gap}."));
        }
        match self.lifecycle {
            BodyLifecycle::Complete => None,
            BodyLifecycle::Streaming => Some("The body is still being received.".to_owned()),
            BodyLifecycle::Interrupted => {
                Some("The sender stopped before the body ended.".to_owned())
            }
            BodyLifecycle::Cancelled => {
                Some("The receiver stopped reading before the body ended.".to_owned())
            }
        }
    }
}

/// A contiguous byte range in the data file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Segment {
    pub(super) offset: u64,
    pub(super) length: u64,
}

#[derive(Clone, Debug)]
struct Entry {
    segments: Vec<Segment>,
    body: StoredBody,
}

impl Entry {
    const fn new() -> Self {
        Self {
            segments: Vec::new(),
            body: StoredBody {
                stored_bytes: 0,
                lifecycle: BodyLifecycle::Streaming,
                gap: None,
            },
        }
    }
}

/// Byte ranges and state of every stored body in one session.
#[derive(Debug, Default)]
pub(super) struct Index {
    entries: HashMap<BodyKey, Entry>,
}

impl Index {
    fn entry(&mut self, key: BodyKey) -> &mut Entry {
        self.entries.entry(key).or_insert_with(Entry::new)
    }

    /// Registers a body that started streaming.
    pub(super) fn begin(&mut self, key: BodyKey) {
        self.entry(key);
    }

    /// Records that `length` bytes of the body were written at `offset`,
    /// merging the range into the previous one when they are adjacent.
    pub(super) fn append(&mut self, key: BodyKey, offset: u64, length: u64) {
        let entry = self.entry(key);
        entry.body.stored_bytes = entry.body.stored_bytes.saturating_add(length);
        if let Some(last) = entry.segments.last_mut()
            && last.offset.saturating_add(last.length) == offset
        {
            last.length = last.length.saturating_add(length);
            return;
        }
        entry.segments.push(Segment { offset, length });
    }

    /// Records how the body stream ended. Only the first end is kept.
    pub(super) fn end(&mut self, key: BodyKey, lifecycle: BodyLifecycle) {
        let entry = self.entry(key);
        if entry.body.lifecycle == BodyLifecycle::Streaming {
            entry.body.lifecycle = lifecycle;
        }
    }

    /// Records that later bytes of the body are missing. Only the first cause is kept.
    pub(super) fn mark_gap(&mut self, key: BodyKey, gap: StoreGap) {
        let entry = self.entry(key);
        entry.body.gap.get_or_insert(gap);
    }

    pub(super) fn info(&self, key: BodyKey) -> Option<StoredBody> {
        self.entries.get(&key).map(|entry| entry.body)
    }

    pub(super) fn segments(&self, key: BodyKey) -> Option<(Vec<Segment>, StoredBody)> {
        self.entries
            .get(&key)
            .map(|entry| (entry.segments.clone(), entry.body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: BodyKey = BodyKey {
        record: 7,
        side: BodySide::Response,
    };
    const OTHER: BodyKey = BodyKey {
        record: 7,
        side: BodySide::Request,
    };

    #[test]
    fn merges_adjacent_ranges_and_keeps_interleaved_ones_apart() {
        let mut index = Index::default();
        index.append(KEY, 0, 4);
        index.append(KEY, 4, 6);
        index.append(OTHER, 10, 3);
        index.append(KEY, 13, 2);
        let segments = index.segments(KEY).map(|(segments, _)| segments);
        let expected = [
            Segment {
                offset: 0,
                length: 10,
            },
            Segment {
                offset: 13,
                length: 2,
            },
        ];
        assert_eq!(segments.as_deref(), Some(&expected[..]));
        assert_eq!(index.info(KEY).map(|body| body.stored_bytes()), Some(12));
        assert_eq!(index.info(OTHER).map(|body| body.stored_bytes()), Some(3));
    }

    #[test]
    fn completeness_needs_a_normal_end_and_no_gap() {
        let mut index = Index::default();
        index.begin(KEY);
        let streaming = index.info(KEY);
        assert!(streaming.is_some_and(|body| !body.is_complete()));
        index.end(KEY, BodyLifecycle::Complete);
        index.end(KEY, BodyLifecycle::Cancelled);
        assert!(index.info(KEY).is_some_and(|body| body.is_complete()));
        assert_eq!(
            index.info(KEY).and_then(|body| body.incomplete_reason()),
            None
        );
        index.mark_gap(KEY, StoreGap::QueueFull);
        index.mark_gap(KEY, StoreGap::SessionLimit);
        let body = index.info(KEY);
        assert_eq!(body.and_then(|body| body.gap()), Some(StoreGap::QueueFull));
        assert!(
            body.and_then(|body| body.incomplete_reason())
                .is_some_and(|reason| reason.contains("queue was full"))
        );
        assert_eq!(index.info(OTHER), None);
    }

    #[test]
    fn interrupted_bodies_are_incomplete() {
        let mut index = Index::default();
        index.end(KEY, BodyLifecycle::Interrupted);
        assert!(
            index
                .info(KEY)
                .and_then(|body| body.incomplete_reason())
                .is_some_and(|reason| reason.contains("sender stopped"))
        );
    }
}
