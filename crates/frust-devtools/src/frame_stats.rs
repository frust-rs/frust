//! The frame-stats fan-out: one publisher (the shell's frame hook), any
//! number of subscribed clients.
//!
//! The whole point of this module is the **non-blocking publish**. It is
//! called from the frame thread, once per frame, and
//! `docs/REVIEW_FOCUS.md`'s severity calibration is explicit that a blocking
//! or parking call on the platform UI thread is a critical defect — so
//! [`FrameStatsBus::publish`] is a plain sync, bounded, never-waiting send: a
//! client too slow to keep up loses the **oldest** queued frames, and the
//! frame thread never notices.

use frust_devtools_protocol::FrameStats;
use tokio::sync::broadcast;

/// Per-subscriber queue depth. ~1 s of 60 Hz frames: enough that an ordinary
/// scheduling hiccup on the client side loses nothing, small enough that a
/// client that stops reading entirely cannot pin more than a few KiB.
pub(crate) const DEFAULT_CAPACITY: usize = 64;

/// The publish/subscribe seam for [`FrameStats`].
///
/// Backed by a `tokio::sync::broadcast` channel, whose semantics are exactly
/// the contract wanted here: a bounded ring per receiver, a sync non-blocking
/// send, and drop-**oldest** on overflow (the lagging receiver observes
/// `RecvError::Lagged(n)` and resumes at the newest retained frame). Frame
/// stats are a monitoring stream — the newest frames are the useful ones, so
/// dropping the oldest is the right loss, and it is the only loss mode that
/// keeps `publish` free of a wait.
pub(crate) struct FrameStatsBus {
    tx: broadcast::Sender<FrameStats>,
}

impl FrameStatsBus {
    pub(crate) fn new(capacity: usize) -> Self {
        // `broadcast::channel` panics on 0.
        let (tx, _) = broadcast::channel(capacity.max(1));
        Self { tx }
    }

    /// Publishes one frame's stats. **Never waits and never fails** — with no
    /// subscribers the value is simply dropped.
    ///
    /// "Never waits" is precise: the value goes into a preallocated ring, so
    /// there is no wait on a client, on the network, or on a full queue. The
    /// only synchronization is the channel's own short internal lock, held for
    /// a slot write — the same class of critical section any lock-based queue
    /// takes, and not a wait on another party's progress.
    pub(crate) fn publish(&self, stats: FrameStats) {
        let _ = self.tx.send(stats);
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<FrameStats> {
        self.tx.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(n: u64) -> FrameStats {
        FrameStats {
            n,
            total_us: 1_000,
            rebuild_us: 100,
            layout_us: 200,
            paint_us: 300,
            encode_us: 150,
            acquire_us: 50,
            submit_us: 200,
            skipped: false,
        }
    }

    #[test]
    fn publish_with_no_subscribers_is_a_no_op() {
        let bus = FrameStatsBus::new(4);
        for n in 0..100 {
            bus.publish(stats(n));
        }
    }

    #[test]
    fn a_full_queue_drops_the_oldest_frames_and_keeps_the_newest() {
        let bus = FrameStatsBus::new(4);
        let mut rx = bus.subscribe();

        // Ten frames published, four slots: the publisher must not block and
        // must not fail — it is the frame thread.
        for n in 0..10 {
            bus.publish(stats(n));
        }

        // The subscriber is told exactly how many it missed, then resumes at
        // the newest four (6, 7, 8, 9) — drop-OLDEST, not drop-newest.
        match rx.try_recv() {
            Err(broadcast::error::TryRecvError::Lagged(missed)) => assert_eq!(missed, 6),
            other => panic!("expected a Lagged report, got {other:?}"),
        }
        for n in 6..10 {
            assert_eq!(rx.try_recv().map(|s| s.n), Ok(n));
        }
        assert!(matches!(
            rx.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
    }

    #[test]
    fn every_subscriber_gets_every_frame_while_it_keeps_up() {
        let bus = FrameStatsBus::new(4);
        let mut a = bus.subscribe();
        let mut b = bus.subscribe();

        bus.publish(stats(1));
        bus.publish(stats(2));

        assert_eq!(a.try_recv().map(|s| s.n), Ok(1));
        assert_eq!(b.try_recv().map(|s| s.n), Ok(1));
        assert_eq!(a.try_recv().map(|s| s.n), Ok(2));
        assert_eq!(b.try_recv().map(|s| s.n), Ok(2));
    }

    #[test]
    fn a_late_subscriber_sees_only_frames_published_after_it_joined() {
        let bus = FrameStatsBus::new(4);
        bus.publish(stats(1));
        let mut rx = bus.subscribe();
        bus.publish(stats(2));

        assert_eq!(rx.try_recv().map(|s| s.n), Ok(2));
    }

    #[test]
    fn zero_capacity_is_clamped_rather_than_panicking() {
        let bus = FrameStatsBus::new(0);
        let mut rx = bus.subscribe();
        bus.publish(stats(1));
        assert_eq!(rx.try_recv().map(|s| s.n), Ok(1));
    }
}
