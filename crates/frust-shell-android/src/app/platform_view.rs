//! The platform-view seam Kotlin polls: the differ's releasable command backlog
//! behind the frame-pairing release gate and the shape-aware scroll-sync tail,
//! plus the lifecycle arms (suspend, acknowledge, frame-timeline sample) that
//! keep the two staged against live frames.

use frust_shell_common::ViewCommand;

use super::AndroidAppHandle;

impl AndroidAppHandle {
    /// Tear the surface down (`surfaceDestroyed`): drop the surface **first**,
    /// then release the `NativeWindow` it borrowed.
    ///
    /// The ordering is a hard correctness contract in the render-thread split:
    /// the render thread owns the surface built from `self.window`'s raw pointer,
    /// so `SplitExecutor::destroy_surface_barrier` sends a barriered
    /// `SurfaceDestroyed` and **blocks until the render thread has acknowledged**
    /// dropping that surface. Only after that ack returns do we set
    /// `self.window = None`, releasing the `ANativeWindow` — so the render thread
    /// can never touch the window after it is released (a known Android
    /// surface-lifecycle-race hazard). The inline path drops its surface
    /// synchronously above, so the same window-after-surface order holds there.
    /// Force every currently-visible platform-view slot to hide
    /// (`nativeOnPause`) — delegates to
    /// `PlatformViewState::suspend_all`.
    pub(crate) fn suspend_platform_views(&mut self) {
        self.platform_view_state.suspend_all();
        // While backgrounded no further frame is painted or presented, so a
        // pairing recorded before the pause would hold this hide behind a frame
        // that never lands. The gate is a smoothing device, not a correctness
        // barrier — drop it so the hide goes out on the next poll.
        self.platform_view_due.clear();
        // ...and with it the tail's hold: neither stage may outlive the
        // frames it refers to (mirroring the pairing above).
        self.sync_tail.clear();
    }

    /// Peek the differ's releasable command backlog. The
    /// `nativePlatformViewCommands` JNI export's read half; pair
    /// with [`Self::acknowledge_platform_view_commands`], called first per the
    /// differ's acknowledge-then-peek contract.
    ///
    /// Not the *whole* backlog: a batch is held until the frust frame that
    /// painted its geometry is actually on screen, so a hosted native view
    /// moves with the frust content it is pinned to instead of 3–5 frames ahead
    /// of it while scrolling (see [`Self::platform_view_due`]).
    /// Held commands are not lost — the poll is idempotent and re-serves them
    /// the moment their frame lands (or the moment the staleness escape hatch
    /// declares that frame gone, counted in submissions while frames flow and in
    /// skipped ticks once the loop idles — see
    /// [`Self::frame`]'s skip path). Kotlin needs no change: it applies exactly
    /// what it is handed and acks the generation it is told.
    pub(crate) fn platform_view_commands(&mut self) -> (u64, &[ViewCommand]) {
        let gated = self.platform_view_due.releasable_generation(
            self.executor.presented_frame_id(),
            self.executor.submitted_frame_id(),
        );
        // Resolve the gate's "nothing is held" answer (`u64::MAX`) against the
        // differ's live tip BEFORE the tail sees it: the tail ages a *real*
        // generation, and a saturated sentinel would be recorded once and then
        // never change again — a hold that silently expires for the rest of the
        // process (found on cupid: the tail read a steady depth 3 while the
        // measured band stayed at the gate-only residual).
        let tip = self.platform_view_state.commands().0;
        // Then the shape-aware tail, which can only ever delay
        // what the gate already released — and only while the surface is in the
        // acquire-bound regime where the leftover residual is constant-shaped.
        // Everywhere else (submit-bound device, below API 33, nothing hosted)
        // this returns the gate's own answer unchanged, i.e. bit-for-bit the
        // shipped gate.
        let releasable = self.sync_tail.releasable(gated.min(tip));
        self.platform_view_state.commands_up_to(releasable)
    }

    /// `nativeSetFrameTimeline`: record this tick's Choreographer frame-timeline
    /// delta (`expectedPresentationTimeNanos − frameTimeNanos`, API 33+) for the
    /// scroll-sync tail's depth derivation. Kotlin pushes it
    /// once per frame while a platform view is actually hosted, and pushes `0`
    /// otherwise — see [`crate::sync_tail`] for why every no-sample path is
    /// gate-only. Untrusted JNI input: a negative value clamps to `0` and an
    /// implausible one is rejected downstream by the tail itself.
    pub(crate) fn set_frame_timeline(&mut self, expected_present_delta_nanos: i64) {
        self.frame_timeline_delta_nanos = expected_present_delta_nanos.max(0) as u64;
    }

    /// Tell the differ the native side has finished applying everything
    /// through `generation` — delegates to
    /// `PlatformViewState::acknowledge`, and drops the matching release-gate
    /// bookkeeping so it tracks the live backlog rather than growing for the
    /// process lifetime.
    pub(crate) fn acknowledge_platform_view_commands(&mut self, generation: u64) {
        self.platform_view_state.acknowledge(generation);
        self.platform_view_due.acknowledge(generation);
    }
}
