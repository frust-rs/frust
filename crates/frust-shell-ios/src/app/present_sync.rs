//! Present-sync: the iOS-only render→UI handoff that lets the UI thread issue
//! `[drawable present]` inside the same `CATransaction` that commits hosted
//! platform-view geometry. The slot ([`PresentHandoff`]) plus the UI-thread half
//! that drains it ([`IosAppHandle::present_pending_frame`]).
//!
//! Nothing here has an Android counterpart — that shell corrects the same
//! surface/view desync with the opposite sign (see [`PresentHandoff`]'s docs).

use std::sync::Mutex;

use frust_render::DeferredPresent;

use super::IosAppHandle;
use super::executor::FrameExecutor;

/// The render→UI handoff slot for the **present-sync** path:
/// a depth-1, latest-wins slot holding at most one submitted-but-unpresented
/// frame ([`DeferredPresent`]).
///
/// # Why it exists
///
/// A `CAMetalLayer` with `presentsWithTransaction = true` requires
/// `[drawable present]` to run on the thread committing the `CATransaction`
/// that also carries the hosted platform views' geometry — otherwise the
/// drawable never reaches the compositor at all (measured on device: the whole
/// screen stays the window background). Under
/// the render-thread split the present runs on the render thread, which commits
/// no transaction. So when the host arms present-sync
/// (`FrustViewController.synchronizesPresentWithPlatformViews` →
/// `frust_set_present_sync`), the render thread stops presenting: it submits as
/// usual and parks the acquired frame here, and the UI thread presents it from
/// `frust_present_frame`, inside the same display-link tick (and transaction)
/// that `FrustViewHost.poll` commits sibling geometry in. **The split stays
/// on** — this is the ladder rung 1 shipping form, not "turn the split off on
/// iOS".
///
/// iOS delays the *surface* to meet the view; Android delays the *view* to meet
/// the surface. The two corrections have opposite signs, so this mechanism is
/// deliberately iOS-local and shares nothing with the Android frame-id gate.
///
/// # Discipline
///
/// Latest-wins, exactly like the UI→render scene channel: storing over an
/// un-taken frame **drops** the older one (its drawable returns to the layer's
/// pool un-presented) rather than blocking the render thread, so a UI thread
/// that misses a tick costs one dropped frame and never a deadlock. A tick with
/// nothing parked presents nothing — the zero-frames-at-rest contract is
/// untouched.
///
/// # Pairing
///
/// The parked frame carries its own `frame_id`, because deferring the present
/// alone would only *move* the desync: the render thread is a tick behind the
/// UI thread (its `acquire` blocks on vsync), so the frame presented in tick N
/// was painted in tick N-1 — pairing it with tick N's geometry would leave the
/// surface lagging the view by a frame, the mirror of the defect. The id lets
/// the UI thread tell the shared release gate
/// ([`FramePairing`](frust_shell_common::platform_view::FramePairing))
/// exactly *which* frame it just presented, so the
/// geometry released in the same transaction is that frame's own. Deferred
/// present and paired release are two halves of one fix.
pub(crate) struct PresentHandoff {
    /// Whether the host armed present-sync. Read once from the process-global
    /// latch at handle construction and never mutated afterwards (the layer's
    /// `presentsWithTransaction` is likewise a fixed, pre-`frust_init` host
    /// choice — see `frust_set_present_sync`), so a plain `bool` shared behind
    /// the `Arc` suffices; no atomic, no interior mutability.
    armed: bool,
    /// The parked frame and the id of the frust frame that painted it, if any.
    /// `Mutex` rather than a channel: the slot is depth-1 and both sides only
    /// ever store/take one value, so a channel's queueing would be a liability
    /// (a backlog of stale drawables), not a feature.
    slot: Mutex<Option<(u64, DeferredPresent)>>,
}

impl PresentHandoff {
    pub(crate) fn new(armed: bool) -> Self {
        Self {
            armed,
            slot: Mutex::new(None),
        }
    }

    /// Whether the render thread should defer its present into this slot.
    pub(crate) fn is_armed(&self) -> bool {
        self.armed
    }

    /// Park a freshly submitted frame (and the id of the frust frame that
    /// painted it) for the UI thread — render thread side. Any frame still
    /// parked is dropped; see the latest-wins discipline above.
    pub(super) fn store(&self, frame_id: u64, frame: DeferredPresent) {
        // Poisoning can only come from a panic while the slot is held, which is
        // a `take`/`store` of an `Option` — no invariant to corrupt — so recover
        // the guard rather than panicking across an FFI-adjacent path.
        let mut slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        *slot = Some((frame_id, frame));
    }

    /// Take the parked frame to present it (UI thread side); `None` when the
    /// render thread produced nothing since the last tick (a gate-skipped or
    /// still-in-flight frame).
    fn take(&self) -> Option<(u64, DeferredPresent)> {
        self.slot.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    /// Drop any parked frame without presenting it — used where presenting
    /// would be wrong or impossible: backgrounding (Metal work from a suspended
    /// app can get the process killed), a surface (re)install (the parked frame
    /// belongs to a swapchain that no longer exists), and teardown.
    pub(crate) fn clear(&self) {
        drop(self.take());
    }
}

impl IosAppHandle {
    /// Present the frame the render thread parked for this thread, if any — the
    /// UI-thread half of the present-sync path, called from
    /// `frust_present_frame` inside the display-link tick, right after
    /// `FrustViewHost.poll` has committed this frame's sibling geometry.
    ///
    /// A no-op when present-sync is not armed (nothing is ever parked), when
    /// the frame gate skipped this tick, or on the inline path (which presents
    /// in-line already). Never blocks on the render thread: an empty slot is
    /// simply nothing to present this tick.
    pub(crate) fn present_pending_frame(&mut self) {
        if let FrameExecutor::Split(split) = &self.executor
            && let Some((frame_id, frame)) = split.present.take()
        {
            // Tell the release gate which frame is now on screen BEFORE
            // presenting, so the `FrustViewHost.poll` that follows in this same
            // transaction releases exactly this frame's geometry (the Swift
            // ordering contract: present, then poll, both inside one
            // `CATransaction`). `max` because ids only move forward.
            self.presented_frame_id = self.presented_frame_id.max(frame_id);
            // The actual `[drawable present]`: with the layer's
            // `presentsWithTransaction` set, wgpu-hal commits the present
            // command buffer, waits until it is scheduled, and hands the
            // drawable over — all on THIS (transaction-committing) thread,
            // which is the whole contract.
            frame.present();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PresentHandoff;

    /// The default (host did not arm present-sync): the render thread presents
    /// inline, nothing is ever parked, and the slot stays inert — the
    /// zero-cost-when-unused half of the mechanism.
    #[test]
    fn unarmed_handoff_is_inert() {
        let handoff = PresentHandoff::new(false);
        assert!(!handoff.is_armed());
        assert!(handoff.take().is_none());
    }

    /// Armed: the render thread must defer its present into the slot instead
    /// of issuing it off the transaction-committing thread. `armed` is fixed
    /// at construction (a pre-`frust_init` host choice), so this is the whole
    /// of the render side's decision.
    #[test]
    fn armed_handoff_reports_deferred_present() {
        let handoff = PresentHandoff::new(true);
        assert!(handoff.is_armed());
    }

    /// `clear` is the backgrounding/(re)install/teardown door and must be a
    /// no-op on an empty slot — it runs on paths that cannot know whether the
    /// render thread parked anything (a gate-skipped tick parks nothing).
    #[test]
    fn clear_on_an_empty_slot_is_a_no_op() {
        let handoff = PresentHandoff::new(true);
        handoff.clear();
        handoff.clear();
        assert!(handoff.take().is_none());
    }
}
