//! The OS-neutral half of the desktop platform-view host: this crate's owner of
//! `frust-shell-common`'s [`PlatformViewState`] differ, driven from the winit
//! frame loop and delivered to whatever per-OS shell is installed through
//! [`DesktopExtensions::on_platform_view_commands`].
//!
//! The mobile shells' equivalents (`frust-shell-android`'s
//! `platform_view_commands`, `frust-shell-ios`'s `platform_view_commands_json`)
//! serve their backlog to a *polling* host across an FFI boundary. Desktop has
//! no FFI boundary and no poller: the per-OS half is an in-process trait
//! implementation on the same thread, so the backlog is **pushed** to it once
//! per frame and acknowledged immediately. That collapses the mobile
//! poll/acknowledge round trip into one call and is the whole reason this module
//! is ~a screen of glue rather than a wire format.
//!
//! # What is deliberately absent
//!
//! - **Logical→physical conversion.** iOS's `to_pv_command` multiplies every
//!   rect by the scale factor because its Swift host places views in physical
//!   px. Desktop does not convert: AppKit (the first consumer) places `NSView`s
//!   in logical points, which is exactly the space
//!   [`ViewCommand`] already carries. The hook still receives the window's
//!   `scale_factor()` so a host that *does* need physical px can scale at its
//!   own boundary, keeping the conversion where the requirement is rather than
//!   in shared code that would have to undo it.
//! - **Input shields.** [`PlatformViewState::ingest`] takes the pass's
//!   auto-collected shield rects; desktop passes `&[]`. Shields only mean
//!   anything to a host forwarding pointer input into a hosted view, which no
//!   desktop host does — the frust surface consumes everything, as on the
//!   mobile v1 path. When a desktop host grows input forwarding, this is the one
//!   line that changes.
//! - **The scroll-sync tail.** Android's `sync_tail` derives a hold from
//!   Choreographer frame-timeline samples; desktop's winit loop has no
//!   equivalent timeline signal, so there is nothing to derive one from.
//! - **[`FramePairing`](frust_shell_common::platform_view::FramePairing), the
//!   present-sync release gate.** See the timing section below.
//!
//! # Timing: commands are applied one frame ahead of presentation
//!
//! A batch is handed to the host immediately after the finished scene is
//! submitted to the frame executor, on the UI thread, i.e. *before* that scene
//! is presented. A hosted view therefore reaches its new geometry up to one
//! frame ahead of the frust content it is pinned to — visible as a hosted view
//! leading its surroundings while a scroll animates, harmless while it is
//! static.
//!
//! Closing that gap is what [`FramePairing`](frust_shell_common::platform_view::FramePairing)
//! plus [`PlatformViewState::commands_up_to`] exist for, and both mobile shells
//! use them. Desktop cannot yet: the gate needs the id of the frame the render
//! side actually **presented**, and this crate's executor publishes only a
//! presented *count* (`FrameExecutor::presented_frames`) with no frame id
//! travelling with a submission — pairing would mean a new id channel through
//! the render split, beyond this seam. Deferred deliberately, with the lead
//! bounded at ≤1 frame by construction (the batch describes the very scene being
//! submitted, never an older one); it belongs in `docs/LIMITATIONS.md` as
//! `desktop-platform-view-frame-lead` once a per-OS host makes it observable.
//!
//! # Suspend / re-create pairing
//!
//! [`DesktopPlatformViews::suspend`] tells the host to drop every hosted view,
//! so every command still queued at that moment describes a view that no longer
//! exists; it is acknowledged away rather than delivered later. The differ's
//! own live-slot map survives, which is what makes the return trip cheap — but
//! it also means the host has nothing to apply an `Update` *to* until it has
//! been told to re-create those views. Hence the contract this module relies on,
//! and `app_handler` upholds: every path back from a suspend brings the surface
//! up again, and every surface bring-up calls
//! [`DesktopPlatformViews::on_surface_recreated`], whose replay re-creates each
//! live slot before the next ingest can emit a bare `Update` for it.

use std::sync::Arc;

use frust_core::RenderRoot;
use frust_core::view::View;
use frust_core::widget::PlatformViewFrame;
use frust_shell_common::platform_view::{PlatformViewState, ViewCommand};
use winit::window::Window;

use crate::extensions::DesktopExtensions;

/// The desktop shell's platform-view host: the shared differ plus the frame-loop
/// sequencing around it (ingest after paint, push after submit, acknowledge, and
/// the two lifecycle resets).
///
/// Owned by the winit `ApplicationHandler`, one per app. Costs nothing until a
/// `platform_view` widget actually paints: an empty ingest produces no commands,
/// and a drain with an empty backlog never reaches the extension hook.
#[derive(Debug, Default)]
pub(crate) struct DesktopPlatformViews {
    state: PlatformViewState,
}

impl DesktopPlatformViews {
    /// A host with no live slots and an empty backlog.
    pub(crate) fn new() -> Self {
        Self {
            state: PlatformViewState::new(),
        }
    }

    /// Feed the frame that was just painted: retire the slots whose widgets were
    /// torn down, then ingest the pass's published frames. Returns whether this
    /// produced any commands.
    ///
    /// Must be called **immediately after** [`RenderRoot::paint`] and before the
    /// next rebuild: [`RenderRoot::platform_view_frames`] reflects only the most
    /// recent paint pass, and [`RenderRoot::take_retired_platform_views`] is a
    /// destructive drain, so an id missed here is only recovered by the differ's
    /// slower missing-streak backstop.
    ///
    /// Retire runs first so a slot that was torn down and a *new* slot that
    /// reused nothing of it cannot interleave: the `Dispose` is already in the
    /// batch before the ingest that might re-create the same id.
    pub(crate) fn after_paint<State, V>(&mut self, root: &mut RenderRoot<State, V>) -> bool
    where
        State: 'static,
        V: View<State>,
    {
        let retired = root.take_retired_platform_views();
        self.ingest(&retired, root.platform_view_frames())
    }

    /// The window-free core of [`Self::after_paint`], split out so the command
    /// stream can be driven from hand-built [`PlatformViewFrame`]s in tests
    /// rather than through a live `RenderRoot` and a real paint pass.
    fn ingest(&mut self, retired: &[u64], frames: &[PlatformViewFrame]) -> bool {
        let mut produced = false;
        for &slot_id in retired {
            produced |= self.state.retire(slot_id);
        }
        // Desktop has no shield channel — see the module docs.
        produced |= self.state.ingest(frames, &[]);
        produced
    }

    /// Hand the pending batch to the per-OS host and acknowledge it.
    ///
    /// Called once per frame, right after the scene is submitted (see the module
    /// docs' timing section) — unconditionally, not only when
    /// [`Self::after_paint`] reported a change: a backlog can also come from
    /// [`Self::on_surface_recreated`]'s replay, which no ingest produced.
    ///
    /// Acknowledging immediately after the hook returns is sound because the
    /// hook *is* the host: it has applied the batch by the time it returns,
    /// unlike a mobile poller whose acknowledgement arrives a round trip later.
    /// `scale` is the window's current `scale_factor()`, passed through for a
    /// host that needs physical px (AppKit does not).
    pub(crate) fn drain<E: DesktopExtensions>(
        &mut self,
        ext: &mut E,
        window: &Arc<Window>,
        scale: f64,
    ) {
        let Some((generation, commands)) = self.pending() else {
            return;
        };
        ext.on_platform_view_commands(window, scale, commands);
        self.applied(generation);
    }

    /// The batch [`Self::drain`] would deliver — the not-yet-acknowledged
    /// backlog plus the generation to acknowledge once it is applied — or `None`
    /// when there is nothing pending.
    ///
    /// The window-free half of the drain, so a test can assert the exact command
    /// stream: the hook call itself needs a live `winit::Window`, which cannot be
    /// constructed without an event loop (the same limit that keeps
    /// `DesktopExtensions::on_window_created` out of that trait's own spy tests).
    fn pending(&self) -> Option<(u64, &[ViewCommand])> {
        let (generation, commands) = self.state.commands();
        (!commands.is_empty()).then_some((generation, commands))
    }

    /// Acknowledge everything through `generation`, compacting it out of the
    /// backlog — the host has applied it.
    fn applied(&mut self, generation: u64) {
        self.state.acknowledge(generation);
    }

    /// The window's surface was (re)created: replay `Create` + `Update` for
    /// every live slot on the next drain, so a host that lost its view hierarchy
    /// rebuilds it from that batch alone.
    ///
    /// Called on every surface bring-up, not only a recovery: on the first one
    /// there are no live slots and this produces nothing, which is what lets the
    /// call site stay a single unconditional line covering both the render
    /// thread's `SurfaceLost` recovery and the resume after a suspend (see the
    /// module docs' suspend/re-create pairing).
    pub(crate) fn on_surface_recreated(&mut self) {
        self.state.reset_for_surface_recreate();
    }

    /// The window is going away (close, destroy, or a suspend that drops the
    /// surface): hide every live slot and tell the host to remove every hosted
    /// view.
    ///
    /// The queued backlog is acknowledged away rather than delivered: after the
    /// hook there is no hosted view left for any of it to apply to, and the
    /// dropped entries are superseded by
    /// [`Self::on_surface_recreated`]'s replay on the way back in. This mirrors
    /// the mobile shells dropping their frame pairing on the same signal —
    /// neither stage may outlive the views it refers to.
    ///
    /// Hiding (rather than disposing) keeps each slot tracked, so the return
    /// replay re-creates it with the geometry it last had; the ordinary paint
    /// that follows a resume then flips `visible` back on with a single
    /// `Update`.
    pub(crate) fn suspend<E: DesktopExtensions>(&mut self, ext: &mut E) {
        self.state.suspend_all();
        ext.on_platform_views_suspended();
        let (generation, _) = self.state.commands();
        self.applied(generation);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_shell_common::platform_view::DISPOSE_AFTER_MISSING_FRAMES;
    use kurbo::Rect;

    /// A spy over the window-free half of the platform-view seam. The command
    /// hook cannot be spied at all — it takes a `&Arc<Window>`, which needs a
    /// live event loop — so the batches it would receive are asserted through
    /// [`DesktopPlatformViews::pending`] instead, and this records only the
    /// suspend hook.
    #[derive(Debug, Default)]
    struct SpyExtensions {
        suspends: usize,
    }

    impl DesktopExtensions for SpyExtensions {
        fn on_platform_views_suspended(&mut self) {
            self.suspends += 1;
        }
    }

    /// One published frame for `slot_id` at `rect`, fully visible, with no
    /// params and no input forwarding — the shape a plain desktop slot paints.
    fn frame(slot_id: u64, rect: Rect) -> PlatformViewFrame {
        PlatformViewFrame {
            slot_id,
            view_type: "dev.frust.VideoPlayerFactory".to_string(),
            params_json: String::new(),
            params_generation: 0,
            rect,
            clip: None,
            visible: true,
            interactive: false,
            shields: Vec::new(),
        }
    }

    /// The commands a drain would hand the host right now, owned so the caller
    /// can acknowledge in the same expression the real drain does.
    fn pending(host: &DesktopPlatformViews) -> Vec<ViewCommand> {
        host.pending()
            .map(|(_, commands)| commands.to_vec())
            .unwrap_or_default()
    }

    /// Drain without a window: take the batch, then acknowledge it exactly as
    /// [`DesktopPlatformViews::drain`] does once the hook returns.
    fn drain(host: &mut DesktopPlatformViews) -> Vec<ViewCommand> {
        let Some((generation, commands)) = host.pending() else {
            return Vec::new();
        };
        let batch = commands.to_vec();
        host.applied(generation);
        batch
    }

    #[test]
    fn a_new_slot_arrives_as_create_then_update_in_one_batch() {
        let mut host = DesktopPlatformViews::new();
        let rect = Rect::new(10.0, 20.0, 110.0, 140.0);

        assert!(host.ingest(&[], &[frame(7, rect)]));

        let batch = pending(&host);
        assert_eq!(batch.len(), 2, "create and update ship together: {batch:?}");
        assert!(matches!(
            &batch[0],
            ViewCommand::Create { slot_id: 7, view_type, interactive: false, .. }
                if view_type == "dev.frust.VideoPlayerFactory"
        ));
        assert_eq!(
            batch[1],
            ViewCommand::Update {
                slot_id: 7,
                rect,
                clip: None,
                visible: true,
                // Desktop passes no auto-collected shields, and the frame
                // declares none of its own.
                shields: Vec::new(),
            }
        );
    }

    #[test]
    fn an_unchanged_repaint_produces_nothing() {
        let mut host = DesktopPlatformViews::new();
        let rect = Rect::new(0.0, 0.0, 100.0, 100.0);
        host.ingest(&[], &[frame(1, rect)]);
        drain(&mut host);

        assert!(
            !host.ingest(&[], &[frame(1, rect)]),
            "a still frame must not cost the host a call"
        );
        assert!(host.pending().is_none());
    }

    #[test]
    fn a_moved_slot_produces_exactly_one_update() {
        let mut host = DesktopPlatformViews::new();
        host.ingest(&[], &[frame(1, Rect::new(0.0, 0.0, 100.0, 100.0))]);
        drain(&mut host);

        let moved = Rect::new(0.0, 40.0, 100.0, 140.0);
        assert!(host.ingest(&[], &[frame(1, moved)]));

        assert_eq!(
            pending(&host),
            vec![ViewCommand::Update {
                slot_id: 1,
                rect: moved,
                clip: None,
                visible: true,
                shields: Vec::new(),
            }]
        );
    }

    #[test]
    fn a_slot_missing_from_two_passes_is_hidden_without_being_disposed() {
        let mut host = DesktopPlatformViews::new();
        let rect = Rect::new(0.0, 0.0, 100.0, 100.0);
        host.ingest(&[], &[frame(1, rect)]);
        drain(&mut host);

        // One missing pass is still inside the streak: a culled-but-alive slot
        // must not flicker.
        assert!(!host.ingest(&[], &[]));
        assert!(host.pending().is_none());

        assert!(host.ingest(&[], &[]));
        assert_eq!(
            drain(&mut host),
            vec![ViewCommand::Update {
                slot_id: 1,
                rect,
                clip: None,
                visible: false,
                shields: Vec::new(),
            }],
            "the hide reuses the slot's last known geometry"
        );

        // Still tracked: reappearing is an ordinary update, not a re-create.
        assert!(host.ingest(&[], &[frame(1, rect)]));
        assert_eq!(
            drain(&mut host),
            vec![ViewCommand::Update {
                slot_id: 1,
                rect,
                clip: None,
                visible: true,
                shields: Vec::new(),
            }]
        );
    }

    #[test]
    fn a_slot_missing_for_the_whole_dispose_streak_is_disposed() {
        let mut host = DesktopPlatformViews::new();
        host.ingest(&[], &[frame(1, Rect::new(0.0, 0.0, 100.0, 100.0))]);
        drain(&mut host);

        for _ in 0..DISPOSE_AFTER_MISSING_FRAMES {
            host.ingest(&[], &[]);
        }
        let batch = drain(&mut host);
        assert!(
            batch.contains(&ViewCommand::Dispose { slot_id: 1 }),
            "the streak backstop must tear the view down: {batch:?}"
        );

        // Forgotten, so the same id reappearing is indistinguishable from new.
        host.ingest(&[], &[frame(1, Rect::new(0.0, 0.0, 100.0, 100.0))]);
        let batch = drain(&mut host);
        assert!(matches!(batch.first(), Some(ViewCommand::Create { .. })));
    }

    #[test]
    fn a_retired_slot_is_disposed_immediately() {
        let mut host = DesktopPlatformViews::new();
        host.ingest(&[], &[frame(1, Rect::new(0.0, 0.0, 100.0, 100.0))]);
        drain(&mut host);

        // The torn-down widget's id arrives with the pass it stopped painting
        // in, which is how the prompt path beats the missing streak.
        assert!(host.ingest(&[1], &[]));
        assert_eq!(drain(&mut host), vec![ViewCommand::Dispose { slot_id: 1 }]);
    }

    #[test]
    fn a_surface_recreate_replays_every_live_slot() {
        let mut host = DesktopPlatformViews::new();
        let first = Rect::new(0.0, 0.0, 100.0, 100.0);
        let second = Rect::new(0.0, 200.0, 100.0, 300.0);
        host.ingest(&[], &[frame(1, first), frame(2, second)]);
        drain(&mut host);

        host.on_surface_recreated();

        let batch = drain(&mut host);
        assert_eq!(batch.len(), 4, "create + update per live slot: {batch:?}");
        assert!(matches!(&batch[0], ViewCommand::Create { slot_id: 1, .. }));
        assert_eq!(
            batch[1],
            ViewCommand::Update {
                slot_id: 1,
                rect: first,
                clip: None,
                visible: true,
                shields: Vec::new(),
            }
        );
        assert!(matches!(&batch[2], ViewCommand::Create { slot_id: 2, .. }));
        assert_eq!(
            batch[3],
            ViewCommand::Update {
                slot_id: 2,
                rect: second,
                clip: None,
                visible: true,
                shields: Vec::new(),
            }
        );
    }

    #[test]
    fn suspend_fires_the_hook_and_leaves_nothing_queued() {
        let mut host = DesktopPlatformViews::new();
        let mut ext = SpyExtensions::default();
        let rect = Rect::new(0.0, 0.0, 100.0, 100.0);
        host.ingest(&[], &[frame(1, rect)]);
        drain(&mut host);

        host.suspend(&mut ext);

        assert_eq!(ext.suspends, 1, "the host must be told to drop its views");
        assert!(
            host.pending().is_none(),
            "queued commands describe views the host has just removed"
        );

        // The slots stay tracked, so the way back in is the replay — a bare
        // `Update` for a view the host no longer has would be dropped on the
        // floor.
        host.on_surface_recreated();
        let batch = drain(&mut host);
        assert!(matches!(&batch[0], ViewCommand::Create { slot_id: 1, .. }));
        assert_eq!(
            batch[1],
            ViewCommand::Update {
                slot_id: 1,
                rect,
                clip: None,
                // Hidden by the suspend; the first paint after the resume flips
                // it back.
                visible: false,
                shields: Vec::new(),
            }
        );
    }

    #[test]
    fn suspending_with_nothing_hosted_still_notifies_the_host() {
        let mut host = DesktopPlatformViews::new();
        let mut ext = SpyExtensions::default();

        host.suspend(&mut ext);

        assert_eq!(ext.suspends, 1);
        assert!(host.pending().is_none());
    }

    #[test]
    fn acknowledging_a_batch_stops_it_being_delivered_again() {
        let mut host = DesktopPlatformViews::new();
        let rect = Rect::new(0.0, 0.0, 100.0, 100.0);
        host.ingest(&[], &[frame(1, rect)]);

        let first = drain(&mut host);
        assert_eq!(first.len(), 2);
        assert!(
            host.pending().is_none(),
            "an acknowledged batch is compacted out of the backlog"
        );

        // An un-acknowledged batch, by contrast, is re-served verbatim — which
        // is what makes a missed drain harmless.
        host.ingest(&[], &[frame(1, Rect::new(0.0, 10.0, 100.0, 110.0))]);
        assert_eq!(pending(&host), pending(&host));
        assert_eq!(pending(&host).len(), 1);
    }
}
