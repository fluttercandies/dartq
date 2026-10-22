//! A bounded ring buffer with a monotonic cursor: events and captured
//! output stream in faster than any consumer drains them, and the
//! session's memory discipline forbids unbounded growth — old entries
//! are overwritten and the consumer learns about the hole via
//! [`Ring::dropped`].

/// Bounded FIFO: `capacity` slots; pushing past the end overwrites the
/// oldest slot.
///
/// A consumer holds a `u64` cursor (the sequence number of the next
/// entry it wants — `0` means "from the first entry ever pushed") and
/// calls [`Ring::since`]. Entries evicted before the cursor ever got
/// read surface through [`Ring::dropped`] so the consumer can mark a
/// gap rather than silently lose them.
#[derive(Debug)]
pub struct Ring<T> {
    slots: Vec<Option<T>>,
    /// Total entries ever pushed (== next sequence number).
    pushed: u64,
    /// Sequence watermark below which nothing can be served: pushed
    /// entries overwritten by wrap-around, plus anything dropped by
    /// [`Ring::clear`].
    cleared_to: u64,
}

impl<T> Ring<T> {
    /// Creates a ring holding at most `capacity` entries. `capacity`
    /// of zero degrades to a counter-only ring (every push counts as
    /// dropped).
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            slots: std::iter::repeat_with(|| None).take(capacity).collect(),
            pushed: 0,
            cleared_to: 0,
        }
    }

    /// Appends one entry, returning its sequence number.
    pub fn push(&mut self, entry: T) -> u64 {
        let seq = self.pushed;
        self.pushed += 1;
        if !self.slots.is_empty() {
            let slot = (seq % self.slots.len() as u64) as usize;
            self.slots[slot] = Some(entry);
        }
        seq
    }

    /// Total entries ever pushed (the sequence space end).
    #[must_use]
    pub fn pushed(&self) -> u64 {
        self.pushed
    }

    /// Number of entries currently retained.
    #[must_use]
    pub fn len(&self) -> usize {
        (self.pushed - self.oldest_seq()) as usize
    }

    /// Whether the ring is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Sequence number of the oldest retained entry — the smallest
    /// cursor position that can still be served.
    #[must_use]
    pub fn oldest_seq(&self) -> u64 {
        let retained = self.slots.len() as u64;
        self.pushed.saturating_sub(retained).max(self.cleared_to)
    }

    /// Entries dropped before ever being read — pushed minus retained.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.oldest_seq()
    }

    /// Returns retained entries with sequence numbers `>= start`,
    /// oldest first. A cursor below the tail is clamped (callers see
    /// [`Ring::dropped`] for the gap size).
    pub fn since(&self, cursor: u64) -> Vec<&T> {
        self.range(cursor.max(self.oldest_seq())).collect()
    }

    /// All retained entries, oldest first.
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.range(self.oldest_seq())
    }

    /// Sequence-to-slot mapping shared by `since` and `iter`; empty
    /// for a zero-capacity ring.
    fn range(&self, from: u64) -> impl Iterator<Item = &T> {
        let slot_count = self.slots.len() as u64;
        (from..self.pushed).filter_map(move |seq| {
            if slot_count == 0 {
                return None;
            }
            self.slots[(seq % slot_count) as usize].as_ref()
        })
    }

    /// Removes every retained entry while keeping sequence numbers
    /// meaningful (consumers see the removed span as dropped).
    pub fn clear(&mut self) {
        for slot in &mut self.slots {
            *slot = None;
        }
        self.cleared_to = self.pushed;
    }
}

#[cfg(test)]
mod tests {
    use super::Ring;

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
        assert_eq!(ring.since(0), [&0, &1, &2]);
        assert_eq!(ring.since(1), [&1, &2]);
        assert_eq!(ring.since(3), Vec::<&i32>::new());
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
        assert_eq!(ring.since(0), [&2, &3, &4]);
        assert_eq!(ring.since(2), [&2, &3, &4]);
        assert_eq!(ring.since(4), [&4]);
        assert_eq!(ring.len(), 3);
    }

    #[test]
    fn cursor_before_retained_start_is_clamped() {
        let mut ring = Ring::new(2);
        for value in 0..4 {
            ring.push(value);
        }
        // A consumer that fell behind by more than the capacity sees
        // only what is retained; `dropped()` tells it what it missed.
        assert_eq!(ring.since(1), [&2, &3]);
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
        assert_eq!(ring.since(0), Vec::<&&str>::new());
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
        assert_eq!(ring.since(0), Vec::<&&str>::new());
        ring.push("c");
        assert_eq!(ring.since(2), [&"c"]);
        assert_eq!(ring.push("d"), 3);
    }
}
