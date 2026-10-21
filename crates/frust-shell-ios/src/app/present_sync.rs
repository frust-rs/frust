//! Present-sync: the iOS-only render→UI handoff that lets the UI thread issue
//! `[drawable present]` inside the same `CATransaction` that commits hosted
//! platform-view geometry. The slot ([`PresentHandoff`]) plus the UI-thread half
//! that drains it ([`IosAppHandle::present_pending_frame`]).
//!
//! Nothing here has an Android counterpart — that shell corrects the same
//! surface/view desync with the opposite sign (see [`PresentHandoff`]'s docs).

use std::time::{Duration, Instant};

use frust_render::DeferredPresent;
use frust_render::overlay::SegmentedPresents;

use super::IosAppHandle;
use super::executor::FrameExecutor;
use crate::ffi_support::PresentSlot;

/// How long the render thread waits for the UI thread to present a parked
/// frame before it acquires or reconfigures a swapchain, past which the
/// leftover is dropped un-presented ([`PresentHandoff::wait_drained`]).
///
/// The UI thread presents in the same display-link tick that handed the next
/// scene over (`renderFrame`: paint, then present inside the transaction), so
/// the wait is normally microseconds; three 60 Hz refreshes bound the case of a
/// UI thread busy elsewhere, and keep a pause barrier racing a parked frame
/// from waiting on a present that will not come for long.
const PRESENT_DRAIN_WAIT: Duration = Duration::from_millis(50);

/// One submitted frame waiting for the UI thread: the id of the frust frame
/// that painted it, and every present it owes — the main surface's and each
/// overlay's, in cut order.
struct ParkedFrame {
    frame_id: u64,
    presents: SegmentedPresents,
}

/// The render→UI handoff for the **present-sync** path: at most one
/// submitted-but-unpresented frame — the main surface's present plus every
/// overlay's ([`SegmentedPresents`]) — parked for the UI thread.
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
/// usual and parks the acquired frames here, and the UI thread presents them
/// from `frust_present_frame` — main surface first, overlays after it, bottom to
/// top — inside the same display-link tick (and transaction) that
/// `FrustViewHost.poll` commits sibling geometry in. **The split stays on.**
///
/// iOS delays the *surface* to meet the view; Android delays the *view* to meet
/// the surface. The two corrections have opposite signs, so this mechanism is
/// deliberately iOS-local and shares nothing with the Android frame-id gate.
///
/// # Discipline: never a swapchain operation over a parked frame
///
/// wgpu allows one acquired frame per swapchain: acquiring again, or
/// reconfiguring, while one is still held is a validation error. So:
///
/// - the render thread **waits for the slot to drain** ([`Self::wait_drained`])
///   before it acquires any swapchain or resizes an overlay, dropping a
///   leftover un-presented only once [`PRESENT_DRAIN_WAIT`] expires;
/// - the UI thread **presents while holding the slot**
///   ([`IosAppHandle::present_pending_frame`]), so a frame it has taken is
///   never still in its hands when the render thread next touches a swapchain;
/// - every lifecycle door that reconfigures or tears down a swapchain
///   (backgrounding, a surface resize or (re)install, an overlay detach or
///   the land that drops a detached overlay's surface, teardown)
///   [`Self::clear`]s it (or drops the one overlay's present) first: a parked
///   `wgpu::SurfaceTexture` must not outlive its surface.
///
/// A tick with nothing parked presents nothing — the zero-frames-at-rest
/// contract is untouched.
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
/// geometry released in the same transaction is that frame's own — overlay
/// geometry included, which rides the same backlog.
pub(crate) struct PresentHandoff {
    /// Whether the host armed present-sync. Read once from the process-global
    /// latch at handle construction and never mutated afterwards (the layer's
    /// `presentsWithTransaction` is likewise a fixed, pre-`frust_init` host
    /// choice — see `frust_set_present_sync`).
    armed: bool,
    /// The parked frame, if any.
    slot: PresentSlot<ParkedFrame>,
}

impl PresentHandoff {
    pub(crate) fn new(armed: bool) -> Self {
        Self {
            armed,
            slot: PresentSlot::new(),
        }
    }

    /// Whether the render thread should defer its presents into this slot.
    pub(crate) fn is_armed(&self) -> bool {
        self.armed
    }

    /// Park a freshly submitted frame's presents (and the id of the frust
    /// frame that painted it) for the UI thread — render thread side. The slot
    /// was drained before this frame acquired ([`Self::wait_drained`]), so
    /// nothing is displaced in practice; anything that is gets dropped.
    pub(super) fn store(&self, frame_id: u64, presents: SegmentedPresents) {
        drop(self.slot.store(ParkedFrame { frame_id, presents }));
    }

    /// Park a single-surface frame (no overlay drew this frame).
    pub(super) fn store_main(&self, frame_id: u64, frame: DeferredPresent) {
        self.store(
            frame_id,
            SegmentedPresents {
                main: Some(frame),
                overlays: Vec::new(),
            },
        );
    }

    /// Render thread side, before this frame acquires or resizes anything:
    /// wait (bounded) for the UI thread to present what is parked, then drop
    /// whatever it did not. A no-op when unarmed.
    pub(crate) fn wait_drained(&self) {
        if !self.armed {
            return;
        }
        if let Some(stale) = self.slot.wait_drained(PRESENT_DRAIN_WAIT) {
            log::debug!(
                "frust-shell-ios: present-sync frame {} not presented within \
                 {PRESENT_DRAIN_WAIT:?}; dropped ({} present(s))",
                stale.frame_id,
                stale.presents.len()
            );
        }
    }

    /// Drop any parked frame without presenting it — used where presenting
    /// would be wrong or impossible: backgrounding (Metal work from a suspended
    /// app can get the process killed), a surface resize or (re)install (the
    /// parked frame belongs to a swapchain about to be reconfigured or gone),
    /// and teardown.
    pub(crate) fn clear(&self) {
        drop(self.slot.clear());
    }

    /// Drop overlay `overlay_id`'s parked present, if any — its surface is
    /// about to go with its layer. The main surface's and every other
    /// overlay's stay parked.
    pub(crate) fn drop_overlay(&self, overlay_id: u64) {
        self.slot.with_parked(|parked| {
            parked.presents.overlays.retain(|(id, _)| *id != overlay_id);
        });
    }
}

impl IosAppHandle {
    /// Drop overlay `overlay_id`'s parked present, if any (see
    /// [`PresentHandoff::drop_overlay`]) — `frust_overlay_attach`/`_detach`,
    /// before the overlay's surface goes (the render thread's land does the
    /// same through its own handle). A no-op on the inline path.
    pub(crate) fn drop_parked_overlay(&self, overlay_id: u64) {
        if let FrameExecutor::Split(split) = &self.executor {
            split.present.drop_overlay(overlay_id);
        }
    }

    /// Present the frame the render thread parked for this thread, if any — the
    /// UI-thread half of the present-sync path, called from
    /// `frust_present_frame` inside the display-link tick, before
    /// `FrustViewHost.poll` applies this frame's sibling geometry in the same
    /// `CATransaction`.
    ///
    /// The main surface presents first, then each overlay in cut order (bottom
    /// to top), all while the handoff slot is held (see [`PresentHandoff`]'s
    /// discipline). A no-op when present-sync is not armed (nothing is ever
    /// parked), when the frame gate skipped this tick, or on the inline path
    /// (which presents in-line already). Never waits on the render thread
    /// beyond the slot's own short critical sections.
    pub(crate) fn present_pending_frame(&mut self) {
        let FrameExecutor::Split(split) = &self.executor else {
            return;
        };
        let presented = split.present.slot.take_with(|parked| {
            let ParkedFrame { frame_id, presents } = parked;
            let SegmentedPresents { main, overlays } = presents;
            // The actual `[drawable present]`s: with each layer's
            // `presentsWithTransaction` set, wgpu-hal commits the present
            // command buffer, waits until it is scheduled, and hands the
            // drawable over — all on THIS (transaction-committing) thread,
            // which is the whole contract.
            let main_at = main.map(|frame| {
                frame.present();
                Instant::now()
            });
            let count = overlays.len();
            for (_, frame) in overlays {
                frame.present();
            }
            if let Some(main_at) = main_at
                && count > 0
            {
                crate::ffi_glue::record_overlay_presents(main_at.elapsed(), count);
            }
            frame_id
        });
        if let Some(frame_id) = presented {
            // Which frame is now on screen, so the `FrustViewHost.poll` that
            // follows in this same transaction releases exactly this frame's
            // geometry (the Swift ordering contract: present, then poll, both
            // inside one `CATransaction`). `max` because ids only move forward.
            self.presented_frame_id = self.presented_frame_id.max(frame_id);
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
        assert!(!handoff.slot.is_parked());
        // Unarmed, the drain is a no-op that never waits.
        handoff.wait_drained();
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
        handoff.drop_overlay(1);
        assert!(!handoff.slot.is_parked());
    }
}
