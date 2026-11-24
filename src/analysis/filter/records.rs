//! Displayed capture records and the counts of records the user cannot see.

use std::collections::VecDeque;

/// Bounded list of displayed records with loss counters measured since the
/// last Clear.
///
/// Two kinds of loss are tracked separately: records the engine dropped
/// because its event queue was full, and records this list evicted to stay
/// within its capacity.
pub struct RecordList<T> {
    records: VecDeque<T>,
    capacity: usize,
    evicted: u64,
    dropped_carried: u64,
    dropped_baseline: u64,
    revision: u64,
}

impl<T> RecordList<T> {
    /// Creates an empty list that holds at most `capacity` records (at least 1).
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            records: VecDeque::with_capacity(capacity),
            capacity: capacity.max(1),
            evicted: 0,
            dropped_carried: 0,
            dropped_baseline: 0,
            revision: 0,
        }
    }

    /// Appends a record and returns the oldest one when it had to be evicted.
    pub fn push(&mut self, record: T) -> Option<T> {
        let removed = if self.records.len() >= self.capacity {
            self.evicted = self.evicted.saturating_add(1);
            self.records.pop_front()
        } else {
            None
        };
        self.records.push_back(record);
        self.revision = self.revision.wrapping_add(1);
        removed
    }

    /// Removes all records and restarts both loss counters.
    ///
    /// `engine_dropped` is the running proxy's cumulative drop count, or zero
    /// when no proxy runs.
    pub fn clear(&mut self, engine_dropped: u64) {
        self.records.clear();
        self.evicted = 0;
        self.dropped_carried = 0;
        self.dropped_baseline = engine_dropped;
        self.revision = self.revision.wrapping_add(1);
    }

    /// Keeps the drops of a proxy that is being stopped, because the next
    /// proxy starts counting from zero.
    pub const fn proxy_stopped(&mut self, engine_dropped: u64) {
        self.dropped_carried = self.dropped(engine_dropped);
        self.dropped_baseline = 0;
    }

    /// Returns engine drops since the last Clear.
    #[must_use]
    pub const fn dropped(&self, engine_dropped: u64) -> u64 {
        self.dropped_carried
            .saturating_add(engine_dropped.saturating_sub(self.dropped_baseline))
    }

    /// Returns records evicted for capacity since the last Clear.
    #[must_use]
    pub const fn evicted(&self) -> u64 {
        self.evicted
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Reports whether no records are retained.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Returns the record at `index`, if present.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<&T> {
        self.records.get(index)
    }

    /// Monotonic counter incremented on every `push` or `clear`.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Iterates over retained records from oldest to newest.
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &T> {
        self.records.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evicts_the_oldest_record_and_counts_it() {
        let mut list = RecordList::new(2);
        assert_eq!(list.push(1), None);
        assert_eq!(list.push(2), None);
        assert_eq!(list.push(3), Some(1));
        assert_eq!(list.iter().copied().collect::<Vec<_>>(), vec![2, 3]);
        assert_eq!(list.evicted(), 1);
    }

    #[test]
    fn clear_restarts_both_counters() {
        let mut list = RecordList::new(1);
        let _evicted = list.push(1);
        let _evicted = list.push(2);
        assert_eq!(list.dropped(7), 7);

        list.clear(7);
        assert_eq!(list.len(), 0);
        assert_eq!(list.evicted(), 0);
        assert_eq!(list.dropped(7), 0);
        assert_eq!(list.dropped(9), 2);
    }

    #[test]
    fn keeps_drops_across_a_proxy_restart() {
        let mut list = RecordList::<u8>::new(4);
        list.clear(3);
        list.proxy_stopped(5);
        assert_eq!(list.dropped(0), 2);
        assert_eq!(list.dropped(4), 6);
    }
}
