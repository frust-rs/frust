//! The one bounded, drop-oldest retention buffer every per-session stream in
//! [`crate::engine`] keeps: log lines, frame-stats samples.
//!
//! Same contract as `frust_drive::process`'s `LINE_BUFFER_CAP` ring — a push
//! at capacity discards the *oldest* entry and bumps a monotonic
//! [`Ring::dropped`] counter rather than blocking the producer or growing
//! without bound. Capacity is a constructor argument (not one workspace-wide
//! constant) because the two streams retain for different reasons: a log tail
//! is what an agent reads back, a frame ring is what the performance tool
//! aggregates over.

use std::collections::VecDeque;

/// A bounded FIFO that drops its oldest entry when full.
pub(crate) struct Ring<T> {
    items: VecDeque<T>,
    cap: usize,
    dropped: u64,
}

impl<T> Ring<T> {
    /// A ring retaining the most recent `cap` entries. A `cap` of `0` is
    /// clamped to `1` — a zero-capacity ring would silently discard
    /// everything, which is never what a caller means.
    pub(crate) fn new(cap: usize) -> Self {
        Self {
            items: VecDeque::new(),
            cap: cap.max(1),
            dropped: 0,
        }
    }

    /// Appends `item`, evicting the oldest entry (and counting it in
    /// [`dropped`](Self::dropped)) if the ring is already at capacity.
    pub(crate) fn push(&mut self, item: T) {
        if self.items.len() >= self.cap {
            self.items.pop_front();
            self.dropped += 1;
        }
        self.items.push_back(item);
    }

    /// How many entries are retained right now (never more than `cap`).
    pub(crate) fn len(&self) -> usize {
        self.items.len()
    }

    /// How many entries have been evicted for capacity since this ring was
    /// created. Additive — never resets — so a caller can tell a complete
    /// history from a truncated one.
    pub(crate) fn dropped(&self) -> u64 {
        self.dropped
    }

    /// The most recent `n` entries in arrival order, or everything retained
    /// when `n` is `None` or exceeds what is held.
    pub(crate) fn tail(&self, n: Option<usize>) -> Vec<T>
    where
        T: Clone,
    {
        let start = match n {
            Some(n) => self.items.len().saturating_sub(n),
            None => 0,
        };
        self.items.iter().skip(start).cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{FRAME_RING_CAP, LOG_RING_CAP};

    #[test]
    fn under_capacity_retains_everything_in_order() {
        let mut ring = Ring::new(4);
        for i in 0..3 {
            ring.push(i);
        }
        assert_eq!(ring.tail(None), vec![0, 1, 2]);
        assert_eq!(ring.len(), 3);
        assert_eq!(ring.dropped(), 0);
    }

    #[test]
    fn tail_returns_the_most_recent_n_in_arrival_order() {
        let mut ring = Ring::new(8);
        for i in 0..6 {
            ring.push(i);
        }
        assert_eq!(ring.tail(Some(2)), vec![4, 5]);
        // A tail longer than what is held is not an error — it is everything.
        assert_eq!(ring.tail(Some(99)), vec![0, 1, 2, 3, 4, 5]);
    }

    /// The log ring's real capacity, exercised at its real constant: pushing
    /// past [`LOG_RING_CAP`] must evict the OLDEST lines and leave the tail
    /// intact.
    #[test]
    fn log_ring_past_capacity_drops_oldest_and_keeps_the_tail() {
        let overflow = 10;
        let total = LOG_RING_CAP + overflow;
        let mut ring: Ring<String> = Ring::new(LOG_RING_CAP);
        for i in 0..total {
            ring.push(format!("line {i}"));
        }

        assert_eq!(ring.len(), LOG_RING_CAP, "the ring must stay capped");
        assert_eq!(ring.dropped(), overflow as u64);
        let held = ring.tail(None);
        // The oldest `overflow` lines are gone; the tail is exact.
        assert_eq!(held.first().unwrap(), &format!("line {overflow}"));
        assert_eq!(held.last().unwrap(), &format!("line {}", total - 1));
        assert_eq!(
            ring.tail(Some(2)),
            vec![format!("line {}", total - 2), format!("line {}", total - 1),]
        );
    }

    /// Same contract at the frame ring's own capacity — the performance
    /// tool aggregates over at most [`FRAME_RING_CAP`] samples, always the
    /// most recent ones.
    #[test]
    fn frame_ring_past_capacity_drops_oldest() {
        let overflow = 25;
        let mut ring: Ring<u64> = Ring::new(FRAME_RING_CAP);
        for i in 0..(FRAME_RING_CAP + overflow) as u64 {
            ring.push(i);
        }
        assert_eq!(ring.len(), FRAME_RING_CAP);
        assert_eq!(ring.dropped(), overflow as u64);
        assert_eq!(ring.tail(None).first(), Some(&(overflow as u64)));
    }

    #[test]
    fn zero_capacity_is_clamped_so_nothing_is_silently_lost() {
        let mut ring = Ring::new(0);
        ring.push("only");
        assert_eq!(ring.tail(None), vec!["only"]);
    }
}
