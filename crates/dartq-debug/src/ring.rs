//! A bounded ring buffer with a monotonic cursor: events and captured
//! output stream in faster than any consumer drains them, and the
//! session's memory discipline forbids unbounded growth — old entries
//! are evicted and the consumer learns about the hole through the
//! sequence space (a cursor whose entries are gone reads as dropped).

use std::collections::VecDeque;

/// Bounded FIFO keyed by a monotonic sequence number.
///
/// Every entry carries its sequence number, so the sequence space may
/// contain holes: [`Ring::skip`] advances the cursor space for entries
/// that were lost upstream (a full channel, a full pipe queue) without
/// occupying a slot — the retained window is never shrunk by a gap.
///
/// Storage grows on demand up to `capacity`: an idle ring costs no
/// slot memory, a full one exactly `capacity` entries.
#[derive(Debug)]
pub struct Ring<T> {
    entries: VecDeque<(u64, T)>,
    capacity: usize,
    /// Next sequence number (== total sequence numbers ever issued,
    /// pushed or skipped).
    next: u64,
}

impl<T> Ring<T> {
    /// Creates a ring retaining at most `capacity` entries. A capacity
    /// of zero degrades to a counter-only ring (every push is dropped).
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            capacity,
            next: 0,
        }
    }

    /// Appends one entry, returning its sequence number; evicts the
    /// oldest entry when full.
    pub fn push(&mut self, entry: T) -> u64 {
        let seq = self.next;
        self.next += 1;
        if self.capacity == 0 {
            return seq;
        }
        if self.entries.len() == self.capacity {
            self.entries.pop_front();
        }
        self.entries.push_back((seq, entry));
        seq
    }

    /// Advances the sequence space by `count` numbers that will never
    /// hold an entry — they read as dropped to every cursor below them.
    pub fn skip(&mut self, count: u64) {
        self.next += count;
    }

    /// The next sequence number (the sequence space end).
    #[must_use]
    pub fn pushed(&self) -> u64 {
        self.next
    }

    /// Number of entries currently retained.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the ring retains nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Sequence number of the oldest retained entry (the sequence end
    /// when empty).
    #[must_use]
    pub fn oldest_seq(&self) -> u64 {
        self.entries.front().map_or(self.next, |(seq, _)| *seq)
    }

    /// Sequence numbers issued but not retained: evicted plus skipped.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.next - self.entries.len() as u64
    }

    /// Retained entries with sequence numbers `>= cursor`, oldest
    /// first. O(log n) to locate the cursor, no allocation.
    pub fn since(&self, cursor: u64) -> impl Iterator<Item = &T> {
        let start = self.entries.partition_point(|(seq, _)| *seq < cursor);
        self.entries.range(start..).map(|(_, entry)| entry)
    }

    /// Retained `(sequence, entry)` pairs with sequence `>= cursor`,
    /// oldest first.
    pub fn entries_since(&self, cursor: u64) -> impl Iterator<Item = (u64, &T)> {
        let start = self.entries.partition_point(|(seq, _)| *seq < cursor);
        self.entries.range(start..).map(|(seq, entry)| (*seq, entry))
    }

    /// How many sequence numbers in `[start, end)` cannot be served
    /// (evicted, cleared, or skipped) — the exact gap a page covering
    /// that span has to report. Bounds clamp to the issued space.
    #[must_use]
    pub fn missing_in(&self, start: u64, end: u64) -> u64 {
        let end = end.min(self.next);
        let start = start.min(end);
        let first = self.entries.partition_point(|(seq, _)| *seq < start);
        let last = self.entries.partition_point(|(seq, _)| *seq < end);
        (end - start) - (last - first) as u64
    }

    /// All retained entries, oldest first.
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.entries.iter().map(|(_, entry)| entry)
    }

    /// Removes every retained entry while keeping sequence numbers
    /// meaningful (consumers see the removed span as dropped).
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::Ring;

    fn since<T: Copy>(ring: &Ring<T>, cursor: u64) -> Vec<T> {
        ring.since(cursor).copied().collect()
    }

    #[test]
    fn push_assigns_monotonic_sequences() {
        let mut ring = Ring::new(4);
        assert_eq!(ring.push("a"), 0);
        assert_eq!(ring.push("b"), 1);
        assert_eq!(ring.pushed(), 2);
        assert_eq!(ring.len(), 2);
        assert!(!ring.is_empty());
    }

    #[test]
    fn since_returns_entries_from_cursor() {
        let mut ring = Ring::new(4);
        for value in 0..3 {
            ring.push(value);
        }
        assert_eq!(since(&ring, 0), [0, 1, 2]);
        assert_eq!(since(&ring, 1), [1, 2]);
        assert_eq!(since(&ring, 3), Vec::<i32>::new());
        assert_eq!(ring.iter().copied().collect::<Vec<_>>(), [0, 1, 2]);
    }

    #[test]
    fn overflow_drops_oldest_and_reports_gap() {
        let mut ring = Ring::new(3);
        for value in 0..5 {
            ring.push(value);
        }
        assert_eq!(ring.oldest_seq(), 2);
        assert_eq!(ring.dropped(), 2);
        assert_eq!(since(&ring, 0), [2, 3, 4]);
        assert_eq!(since(&ring, 2), [2, 3, 4]);
        assert_eq!(since(&ring, 4), [4]);
        assert_eq!(ring.len(), 3);
        assert_eq!(ring.missing_in(0, u64::MAX), 2);
        assert_eq!(ring.missing_in(2, u64::MAX), 0);
    }

    #[test]
    fn cursor_before_retained_start_is_clamped() {
        let mut ring = Ring::new(2);
        for value in 0..4 {
            ring.push(value);
        }
        assert_eq!(since(&ring, 1), [2, 3]);
        assert_eq!(ring.missing_in(1, u64::MAX), 1);
        assert_eq!(ring.dropped(), 2);
    }

    #[test]
    fn zero_capacity_ring_counts_everything_dropped() {
        let mut ring = Ring::new(0);
        assert_eq!(ring.push("a"), 0);
        assert_eq!(ring.push("b"), 1);
        assert!(ring.is_empty());
        assert_eq!(ring.pushed(), 2);
        assert_eq!(ring.oldest_seq(), 2);
        assert_eq!(ring.dropped(), 2);
        assert_eq!(ring.since(0).count(), 0);
        assert_eq!(ring.missing_in(0, u64::MAX), 2);
        assert_eq!(ring.iter().count(), 0);
    }

    #[test]
    fn clear_marks_entries_dropped_and_keeps_cursor_space() {
        let mut ring = Ring::new(4);
        ring.push("a");
        ring.push("b");
        ring.clear();
        assert!(ring.is_empty());
        assert_eq!(ring.pushed(), 2);
        assert_eq!(ring.oldest_seq(), 2);
        assert_eq!(ring.dropped(), 2);
        assert_eq!(ring.since(0).count(), 0);
        ring.push("c");
        assert_eq!(since(&ring, 2), ["c"]);
        assert_eq!(ring.push("d"), 3);
    }

    #[test]
    fn skipped_sequences_read_as_dropped_without_evicting() {
        let mut ring = Ring::new(3);
        ring.push(0);
        ring.push(1);
        ring.skip(5); // seqs 2..7 lost upstream
        assert_eq!(ring.push(7), 7);
        // The gap occupies no slot: all three real entries survive.
        assert_eq!(since(&ring, 0), [0, 1, 7]);
        assert_eq!(ring.missing_in(0, u64::MAX), 5);
        assert_eq!(ring.missing_in(2, u64::MAX), 5);
        assert_eq!(ring.missing_in(7, u64::MAX), 0);
        assert_eq!(since(&ring, 3), [7]);
        assert_eq!(ring.dropped(), 5);
        // A page ending inside the gap reports only its own share.
        assert_eq!(ring.missing_in(0, 4), 2);
        let pairs: Vec<(u64, i32)> = ring.entries_since(1).map(|(seq, v)| (seq, *v)).collect();
        assert_eq!(pairs, [(1, 1), (7, 7)]);
    }

    #[test]
    fn storage_grows_lazily_up_to_capacity() {
        let mut ring: Ring<u8> = Ring::new(1_000_000);
        assert_eq!(ring.entries.capacity(), 0, "an idle ring holds no slots");
        ring.push(1);
        assert!(ring.entries.capacity() < 1_000_000);
    }
}
