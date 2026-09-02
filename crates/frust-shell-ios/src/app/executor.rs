//! The render-path half of the iOS frame loop: the two executors
//! ([`FrameExecutor::Split`], the default, and [`FrameExecutor::Inline`], the
//! `FRUST_NO_RENDER_THREAD` fallback), the scene payload they hand across the
//! split, and the shared encode→acquire→submit tail ([`render_scene`]) both run.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use frust_core::FrameTime;
use frust_render::{
    AcquireOutcome, EncodeOutcome, FrameOutcome, RenderContext, SurfacePhase, SurfaceRenderer,
};
use frust_scene::Scene;
use frust_shell_common::perf::{
    self, FramePasses, FrameStats, GpuPasses, RenderSpans, StartupSpans, UiSpans,
};
use frust_shell_common::{
    FrameMeta, RenderCommand, RenderSender, SceneFrame, SceneReturnReceiver, SurfaceSize,
};
use objc2::rc::autoreleasepool;

use super::present_sync::PresentHandoff;

/// One finished frame's payload crossing the UI→render-thread handoff in the
/// split: the painted [`Scene`] plus the clear color it was
/// painted for (the live theme's surface color — it must ride *with* the frame so
/// a mid-frame theme flip clears to the right color, mirroring the desktop and
/// Android shells' `PaintedScene`). This is the `S` type parameter of
/// [`SceneFrame`]/[`render_channel`](frust_shell_common::render_channel); both
/// `Scene` and `peniko::Color` are `Send`, keeping the handoff `Send`-clean with
/// no `unsafe`.
pub(crate) struct PaintedScene {
    pub(crate) scene: Scene,
    pub(crate) base_color: peniko::Color,
}

/// The render-path half of the iOS frame loop: either the
/// render-thread split ([`Self::Split`], default) or the pre-split inline fallback
/// ([`Self::Inline`], `FRUST_NO_RENDER_THREAD`). Chosen once at construction from
/// [`render_thread_enabled`](frust_shell_common::render_thread_enabled) and owned
/// by [`crate::app::IosAppHandle`].
pub(crate) enum FrameExecutor {
    /// Pre-split fallback: the [`RenderContext`]/[`SurfaceRenderer`] and all perf
    /// recording live on the UI thread, and the encode→acquire→submit tail runs
    /// synchronously inside [`crate::app::IosAppHandle::frame`]. Boxed — it owns
    /// the whole render stack and dwarfs the split's thread-handle variant.
    Inline(Box<InlineExecutor>),
    /// The split: the renderer + context moved to a dedicated render thread; the
    /// UI thread hands it finished frames over the channel.
    Split(SplitExecutor),
}

/// The single-thread fallback executor (kill switch engaged): the
/// [`RenderContext`]/[`SurfaceRenderer`] and all perf recording live on the UI
/// thread, exactly as the pre-split shell did.
pub(crate) struct InlineExecutor {
    /// The render context, borrowed out by
    /// [`crate::app::IosAppHandle::inline_renderer_mut`] for the FFI layer's
    /// `unsafe` surface recreation.
    pub(super) render_cx: RenderContext,
    /// The surface renderer, likewise borrowed out for the inline path's
    /// surface recreation and read for the live surface's phase/resolved alpha.
    pub(super) renderer: SurfaceRenderer,
    frame_stats: FrameStats,
    /// The cold-start span recorder begun in [`crate::ffi_glue::create_handle`];
    /// `Option::take`n on the first successful present (records
    /// [`perf::SPAN_FIRST_FRAME_PRESENTED`] + emits — inside [`render_scene`]),
    /// `None` thereafter.
    startup_spans: Option<StartupSpans>,
    /// Running count of presented frames. Inline renders on the UI
    /// thread, so this is bumped and read on the same thread — the `Arc<Atomic>`
    /// shape matches the split's cross-thread counter so `FrameExecutor` reads
    /// both variants uniformly.
    presented: Arc<AtomicU64>,
}

impl InlineExecutor {
    /// Build the fallback executor around the already-created, `SurfaceReady`
    /// renderer + context (surface creation and the early startup spans happened
    /// in [`crate::ffi_glue::create_handle`], which hands `startup_spans` over
    /// here to finish).
    pub(crate) fn new(
        render_cx: RenderContext,
        renderer: SurfaceRenderer,
        startup_spans: StartupSpans,
    ) -> Self {
        Self {
            render_cx,
            renderer,
            frame_stats: FrameStats::new(),
            startup_spans: Some(startup_spans),
            presented: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Run the encode→acquire→submit tail synchronously for `scene`, recording
    /// the folded frame and returning the encode span for the UI-side deadline
    /// estimate.
    fn submit_frame(
        &mut self,
        scene: &Scene,
        base_color: peniko::Color,
        ui: UiSpans,
        perf_on: bool,
    ) -> Duration {
        render_scene(
            &mut self.renderer,
            &self.render_cx,
            scene,
            base_color,
            ui,
            &mut self.frame_stats,
            &mut self.startup_spans,
            perf_on,
            &self.presented,
            // Present-sync never applies inline: this tail already runs on the
            // UI thread inside the `CADisplayLink` tick, so wgpu's own present
            // is *already* issued on the transaction-committing thread — fully
            // in sync with no deferral needed. Deferring it a tick here would
            // only add latency.
            None,
        )
    }

    /// Record a gate-skipped frame (all-zero pass durations) so the skip counter
    /// accumulates in the perf line, mirroring the pre-split inline behavior.
    ///
    /// Still asks for this surface's real GPU pass timing: the reading lags
    /// the calling frame by design (see [`SurfaceRenderer::gpu_pass_timings`]),
    /// so a run of gate-skipped ticks would otherwise show `gpu_q=0` gaps in
    /// the log even while the ring keeps a perfectly good recent reading.
    fn record_skip(&mut self) {
        let mut passes = FramePasses {
            skipped: true,
            ..FramePasses::default()
        };
        if let Some(gpu) = gpu_passes(&self.renderer) {
            passes = passes.with_gpu(gpu);
        }
        self.frame_stats.record(passes);
        if self.frame_stats.should_emit() {
            self.frame_stats.emit_log();
        }
    }

    /// Record [`perf::SPAN_FIRST_REBUILD_DONE`] right after the initial rebuild
    /// (the UI thread owns the startup line in the inline path).
    fn record_first_rebuild(&mut self) {
        if let Some(spans) = self.startup_spans.as_mut() {
            spans.record(perf::SPAN_FIRST_REBUILD_DONE);
        }
    }
}

/// Upper bound on how long `frust_pause` blocks the UI (main) thread on the
/// [`RenderCommand::Pause`] barrier before letting the app background degraded
/// rather than hanging behind a wedged render thread.
///
/// **Community-approximate**: UIKit's app-suspension watchdog (the deadline for
/// returning from `applicationDidEnterBackground` before the system kills the
/// process, `0x8badf00d`) is not a published constant; the community-converged
/// estimate is ~5s of background-transition grace. 2s stays safely under that so
/// the barrier degrades before the watchdog fires; the barrier normally returns
/// in microseconds (the render thread quiesces to `Paused`), so a multi-second
/// wait means the render thread is stuck and hanging risks the process kill this
/// barrier exists to prevent.
const PAUSE_BARRIER_DEADLINE: Duration = Duration::from_secs(2);

/// The render-thread-split executor (default): the UI-thread [`RenderSender`] half
/// of the scene-handoff channel plus the render thread's [`JoinHandle`]. The
/// render thread owns the [`RenderContext`]/[`SurfaceRenderer`], the surface, the
/// [`FrameStats`] recorder (the single perf emitter), and the startup line — see
/// [`crate::ffi_glue::render_loop`].
pub(crate) struct SplitExecutor {
    /// `Option` so [`Drop`] can drop it *before* joining: dropping the sender is
    /// what signals the render loop to exit.
    sender: Option<RenderSender<PaintedScene, crate::ffi_glue::SendableMetalLayer>>,
    join: Option<JoinHandle<()>>,
    /// The UI-side mirror of "a surface exists" — the render thread owns the real
    /// `SurfacePhase`, so the UI thread can't query it; this gates
    /// [`crate::app::IosAppHandle::frame`]'s not-ready early return in place of
    /// `renderer.phase()`.
    /// Always `true` after construction on iOS (the `CAMetalLayer` is permanent
    /// and the render thread self-heals a lost surface — no UI-side destroy path).
    surface_active: bool,
    /// Monotonically increasing per-frame id stamped into [`FrameMeta`].
    frame_id: u64,
    /// Fatal-signal flag: the render thread stores `true` here
    /// if its **first** surface install fails — unrecoverable (an incapable
    /// GPU/driver can't change mid-process). The UI thread reads it via
    /// [`crate::app::IosAppHandle::render_fatal`] each `frust_render_frame` and
    /// returns [`FRAME_FATAL`](crate::ffi_support::FRAME_FATAL) so Swift latches
    /// `initFailed` + invalidates its `CADisplayLink`. One clone here, one in the
    /// render thread ([`crate::ffi_glue::render_loop`]).
    pub(super) fatal: Arc<AtomicBool>,
    /// The UI-side half of the render thread's scene give-back channel:
    /// polled once per [`Self::submit_frame`] for a scene the
    /// render thread has finished with, so its buffer is reused instead of
    /// reallocating a fresh `Scene` every frame.
    scene_return: SceneReturnReceiver<Scene>,
    /// A scene reclaimed from [`RenderSender::send_scene`]'s returned stale
    /// frame (the UI thread outran the render thread, overwriting an
    /// un-taken scene in the latest-wins slot) — checked before
    /// [`Self::scene_return`] on the next [`Self::take_reusable_scene`] call
    /// so that buffer is reused too, rather than dropped.
    spare_scene: Option<Scene>,
    /// Running count of presented frames: one clone here (read by the
    /// UI thread before paint via `FrameExecutor::presented_frames`), one on the
    /// render thread ([`crate::ffi_glue::render_loop`], which bumps it on each
    /// `FrameOutcome::Rendered`). A plain `Arc<AtomicU64>` — no channel/protocol,
    /// mirroring the `fatal`-flag pattern.
    presented: Arc<AtomicU64>,
    /// The present-sync handoff slot: one clone here (the UI
    /// thread takes and presents from it in
    /// [`crate::app::IosAppHandle::present_pending_frame`]), one on the render
    /// thread ([`crate::ffi_glue::render_loop`], which parks each submitted frame
    /// in it instead of presenting when armed). Inert — never written, always
    /// empty — when the host did not arm present-sync, which is the default.
    /// See [`PresentHandoff`].
    pub(super) present: Arc<PresentHandoff>,
    /// The render thread's "I just (re)installed the surface myself" signal:
    /// set by [`crate::ffi_glue::render_loop`]'s
    /// `SurfaceLost` self-heal arm, taken once per frame by
    /// [`FrameExecutor::take_surface_reinstalled`] and fed into
    /// [`FrameInputs::surface_changed_or_resized`].
    ///
    /// # Why the flag exists
    ///
    /// iOS is the one shell whose surface loss has **no platform entry point**:
    /// the `CAMetalLayer` is permanent, so nothing external re-drives creation
    /// and the render thread heals itself (see [`crate::ffi_glue::render_loop`]'s
    /// iOS surface recovery). Android's equivalent arrives as a Kotlin
    /// `surfaceChanged`, which latches its own gate input. Before the frame gate
    /// actually idled on iOS that gap was invisible — the next of an unbroken
    /// stream of frames repainted the healed surface within a tick. Now that a
    /// resting screen produces no frames at all, a self-heal with no UI-side
    /// signal would leave the fresh swapchain un-painted (and, under
    /// present-sync, the platform-view batch paired with the lost frame held)
    /// until some unrelated dirtiness happened along. So the render thread says
    /// so, and the next tick runs a real frame.
    ///
    /// A plain `Arc<AtomicBool>` — one clone here, one on the render thread —
    /// the same shape as [`Self::fatal`]/[`Self::presented`], no channel or
    /// protocol. Inert on the inline path, which recovers UI-side in
    /// `frust_render_frame`/`frust_resize` (and re-opens the gate's warmup
    /// through `IosAppHandle::set_surface`) instead.
    surface_reinstalled: Arc<AtomicBool>,
}

impl SplitExecutor {
    pub(crate) fn new(
        sender: RenderSender<PaintedScene, crate::ffi_glue::SendableMetalLayer>,
        join: JoinHandle<()>,
        fatal: Arc<AtomicBool>,
        scene_return: SceneReturnReceiver<Scene>,
        presented: Arc<AtomicU64>,
        present: Arc<PresentHandoff>,
        surface_reinstalled: Arc<AtomicBool>,
    ) -> Self {
        Self {
            sender: Some(sender),
            join: Some(join),
            surface_active: true,
            frame_id: 0,
            fatal,
            scene_return,
            spare_scene: None,
            presented,
            present,
            surface_reinstalled,
        }
    }

    /// Reclaim a reusable, empty `Scene` for the next frame: prefer a scene
    /// already reclaimed from a stale [`Self::submit_frame`]
    /// give-back ([`Self::spare_scene`]), else poll the render thread's
    /// give-back channel ([`Self::scene_return`]), else allocate a fresh one.
    /// Either reclaimed scene is [`Scene::reset`] before being handed out —
    /// clearing its commands while keeping the backing `Vec` capacity, which
    /// is the whole point of reusing it over `Scene::new()`.
    fn take_reusable_scene(&mut self) -> Scene {
        if let Some(spare) = self.spare_scene.take() {
            return spare;
        }
        if let Some(mut returned) = self.scene_return.try_recv() {
            returned.reset();
            return returned;
        }
        Scene::new()
    }

    /// Hand one finished frame to the render thread (latest-wins).
    fn submit_frame(
        &mut self,
        painted: PaintedScene,
        ui: UiSpans,
        frame_time: FrameTime,
        size: SurfaceSize,
    ) {
        self.frame_id += 1;
        if let Some(sender) = self.sender.as_ref() {
            let stale = sender.send_scene(SceneFrame {
                scene: painted,
                meta: FrameMeta {
                    frame_time,
                    size,
                    frame_id: self.frame_id,
                },
                ui_spans: ui,
            });
            // The UI thread outran the render thread: the just-overwritten,
            // never-rendered stale frame's scene is still perfectly reusable —
            // reclaim its buffer instead of letting it drop.
            if let Some(stale_frame) = stale {
                let mut reclaimed = stale_frame.scene.scene;
                reclaimed.reset();
                self.spare_scene = Some(reclaimed);
            }
        }
    }

    /// Send a [`RenderCommand::SurfaceChanged`] (the layer-owned in-place resize),
    /// fire-and-forget.
    pub(super) fn resize(&mut self, size: SurfaceSize) {
        if let Some(sender) = self.sender.as_ref() {
            sender.send_command(RenderCommand::SurfaceChanged { size });
        }
    }

    /// The **iOS backgrounding barrier**: send a
    /// barriered [`RenderCommand::Pause`] and **block** until the render thread
    /// has acknowledged it. The render loop processes the `Pause` only after any
    /// in-flight frame's submit completes, moves its [`RenderPhase`] to `Paused`
    /// (so any scene still in the latest-wins slot is dropped, not submitted), and
    /// only *then* acks — so when this returns, the render thread is guaranteed to
    /// be parked, submitting no more Metal work. The caller (`frust_pause`) must
    /// not let the app background until this returns: Metal submission from a
    /// suspended app can get the process killed.
    pub(super) fn pause_barrier(&mut self) {
        if let Some(sender) = self.sender.as_ref() {
            // Bounded so a wedged render thread degrades instead of blocking the
            // main thread past the backgrounding watchdog (see
            // `PAUSE_BARRIER_DEADLINE`).
            if !sender.pause().wait_timeout(PAUSE_BARRIER_DEADLINE) {
                log::error!(
                    "frust-shell-ios: pause barrier timed out after \
                     {PAUSE_BARRIER_DEADLINE:?}; backgrounding (degraded)"
                );
            }
        }
    }

    /// Send a fire-and-forget [`RenderCommand::Resume`]: the render thread returns
    /// to `Active` and resumes submitting handed-off scenes.
    pub(super) fn resume(&mut self) {
        if let Some(sender) = self.sender.as_ref() {
            sender.send_command(RenderCommand::Resume);
        }
    }
}

impl Drop for SplitExecutor {
    fn drop(&mut self) {
        // Present-sync teardown: release any parked, unpresented frame
        // FIRST, so the drawable it holds is gone before the render thread (and
        // with it the surface it came from) is torn down below. Presenting it
        // here would be wrong — `frust_destroy` runs as the view goes away.
        self.present.clear();
        // Destroy-join ordering: drop the sender first
        // — that signals the render loop's `wait_next` to wake with a
        // disconnection and exit, dropping its `SurfaceRenderer` (and the
        // `wgpu::Surface` built from the retained `CAMetalLayer` pointer). Then
        // join, so the surface is fully torn down BEFORE this returns — and, since
        // this runs inside `frust_destroy` (which drops the handle), before
        // `frust_destroy` returns and Swift releases the layer. So the render
        // thread can never touch the layer after Swift frees it.
        self.sender.take();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl FrameExecutor {
    /// Whether a live surface exists — the [`crate::app::IosAppHandle::frame`]
    /// not-ready gate.
    /// Inline reads the renderer's phase directly; the split tracks a UI-side
    /// `surface_active` flag (the render thread owns the real phase).
    pub(super) fn has_surface(&self) -> bool {
        match self {
            FrameExecutor::Inline(inline) => inline.renderer.phase() == SurfacePhase::SurfaceReady,
            FrameExecutor::Split(split) => split.surface_active,
        }
    }

    /// The running count of frames the render side has actually presented
    /// (`FrameOutcome::Rendered`). The UI thread loads this once per frame and
    /// pushes it into `AppTree::set_presented_frames` before paint, so a widget
    /// measuring FPS reports the presented rate — under the split, below the
    /// `CADisplayLink` paint cadence. Both variants share the counter
    /// with their render side via an `Arc<AtomicU64>`.
    pub(super) fn presented_frames(&self) -> u64 {
        match self {
            FrameExecutor::Inline(inline) => inline.presented.load(Ordering::Relaxed),
            FrameExecutor::Split(split) => split.presented.load(Ordering::Relaxed),
        }
    }

    /// The id of the last frame **handed to** the render side — the release
    /// gate's submission cursor, against which a batch whose own frame was
    /// dropped by the depth-1 latest-wins channel is declared stale
    /// (`MAX_FRAMES_IN_FLIGHT`). The next frame to be submitted is therefore
    /// this + 1, which is the id a batch ingested during that frame's paint is
    /// paired with.
    ///
    /// `0` on the inline path, which stamps no frame ids — harmless, because
    /// the gate that reads this is only ever armed in the split (see
    /// `IosAppHandle::present_sync`).
    pub(super) fn submitted_frame_id(&self) -> u64 {
        match self {
            FrameExecutor::Inline(_) => 0,
            FrameExecutor::Split(split) => split.frame_id,
        }
    }

    /// Take (and clear) the render thread's surface-self-heal signal — `true`
    /// exactly once per render-side surface (re)install attempt (see
    /// [`SplitExecutor::surface_reinstalled`] for why iOS needs it and
    /// Android does not). Always `false` on the inline path, which recovers
    /// UI-side instead.
    ///
    /// `swap` rather than a load: the signal must be consumed by the one frame
    /// it forces to run, or a single self-heal would keep forcing `Run` forever
    /// — the very defect this fix exists to close.
    pub(super) fn take_surface_reinstalled(&self) -> bool {
        match self {
            FrameExecutor::Inline(_) => false,
            FrameExecutor::Split(split) => split.surface_reinstalled.swap(false, Ordering::AcqRel),
        }
    }

    /// Record the `first_rebuild_done` startup milestone after the initial
    /// rebuild. Inline records it here on the UI thread; the split records it
    /// render-side when the first scene arrives (see [`crate::ffi_glue::render_loop`]),
    /// so this is a no-op there.
    pub(super) fn record_first_rebuild(&mut self) {
        if let FrameExecutor::Inline(inline) = self {
            inline.record_first_rebuild();
        }
    }

    /// Record a gate-skipped frame. Inline accumulates it in its UI-side
    /// `FrameStats`; the split sends **nothing** on a skip (the render thread is
    /// the single emitter and never sees skipped frames — split mode records no
    /// skip frames; known, logged for 11.E), so this is a no-op there.
    pub(super) fn record_skip(&mut self) {
        if let FrameExecutor::Inline(inline) = self {
            inline.record_skip();
        }
    }

    /// Hand one finished frame to the executor. Inline runs the encode→present
    /// tail synchronously (borrowing `scene`, reused next frame) and returns its
    /// encode span; the split moves the scene out into a [`SceneFrame`] and sends
    /// it across the channel, replacing it with a scene reclaimed off the render
    /// thread's give-back channel — `reset()`, so its buffer
    /// is reused rather than reallocated — falling back to `Scene::new()` only
    /// when none is available yet, and returning `Duration::ZERO` (encode is
    /// off-thread, so it does not count against the UI thread's deadline).
    pub(super) fn submit_frame(
        &mut self,
        scene: &mut Scene,
        base_color: peniko::Color,
        ui: UiSpans,
        frame_time: FrameTime,
        size: SurfaceSize,
        perf_on: bool,
    ) -> Duration {
        match self {
            FrameExecutor::Inline(inline) => inline.submit_frame(scene, base_color, ui, perf_on),
            FrameExecutor::Split(split) => {
                let replacement = split.take_reusable_scene();
                let painted = PaintedScene {
                    scene: std::mem::replace(scene, replacement),
                    base_color,
                };
                split.submit_frame(painted, ui, frame_time, size);
                Duration::ZERO
            }
        }
    }
}

/// This surface's most recent real GPU pass timing, folded into the
/// [`GpuPasses`] shape [`FramePasses::with_gpu`] takes, or `None` when the
/// surface produces no such measurement (a device that never offered
/// `TIMESTAMP_QUERY`, or simply no reading landed yet — see [`SurfaceRenderer::gpu_pass_timings`]).
///
/// Shared by [`render_scene`] and [`InlineExecutor::record_skip`] below, the
/// two frame-record sites in this module.
fn gpu_passes(renderer: &SurfaceRenderer) -> Option<GpuPasses> {
    renderer.gpu_pass_timings().map(|timings| GpuPasses {
        prepass: timings.prepass,
        main: timings.main,
        composite: timings.composite,
        blit: timings.blit,
    })
}

/// Run the encode→acquire→submit tail for one painted `scene`, timing each span
/// behind `perf_on` (the FFI-path perf convention: zero clock reads when
/// disabled), recording the folded [`FramePasses`] through the single emitter
/// (`frame_stats`), and stamping the first-encode / first-frame startup milestones
/// on `startup_spans`. Returns the encode span. Shared by the inline path (UI
/// thread) and the split path's [`crate::ffi_glue::render_loop`] (render thread)
/// so the per-frame render logic is not forked — the exact pre-split tail, only
/// relocated.
///
/// # Autorelease pool
///
/// The swapchain **acquire** (`nextDrawable`) and Metal command submission happen
/// on *this* thread — in the split that is the dedicated render thread, which has
/// no UIKit runloop draining an autorelease pool each iteration. So the whole GPU
/// tail is wrapped in an explicit [`autoreleasepool`] drain: without it every
/// frame's autoreleased `CAMetalDrawable` (and other Metal temporaries) would
/// accumulate and exhaust the layer's small drawable pool — a hard stall. On the
/// inline path (the UIKit main thread) this nests inside the runloop's own pool,
/// which is harmless — the iOS-specific hazard the split adds.
///
/// # Present-sync
///
/// `present` is the render side of the [`PresentHandoff`] slot, paired with the
/// id of the frame being rendered (`None` on the inline path, which needs no
/// deferral). When it is `Some` **and armed**, the submit step becomes
/// `submit_deferred`: the frame's GPU work is submitted here as usual, but its
/// `[drawable present]` is parked — under its own frame id — for the UI thread
/// to issue inside the platform-view transaction. The
/// `presented` counter and the `first_frame_presented` startup span are still
/// stamped here, so on that path both run up to one display-link tick ahead of
/// the actual present — a constant offset that leaves the *rate* those values
/// report unchanged (iOS uses the counter for FPS reporting only; the frame-id
/// gate that needs a true presented count is Android-only).
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_scene(
    renderer: &mut SurfaceRenderer,
    render_cx: &RenderContext,
    scene: &Scene,
    base_color: peniko::Color,
    ui: UiSpans,
    frame_stats: &mut FrameStats,
    startup_spans: &mut Option<StartupSpans>,
    perf_on: bool,
    presented: &AtomicU64,
    present: Option<(&PresentHandoff, u64)>,
) -> Duration {
    autoreleasepool(|_pool| {
        // Encode span (GPU/CPU encode, no swapchain touch).
        let encode_start = perf_on.then(Instant::now);
        let encode_outcome = renderer.encode(render_cx, scene, base_color);
        let encode_time = encode_start.map_or(Duration::ZERO, |t| t.elapsed());

        // First-frame decomposition: stamp the first encode-complete boundary once
        // (only when something was actually encoded).
        if matches!(encode_outcome, Ok(EncodeOutcome::Encoded))
            && let Some(spans) = startup_spans.as_mut()
            && !spans
                .spans()
                .iter()
                .any(|(n, _)| *n == perf::SPAN_FIRST_ENCODE_DONE)
        {
            spans.record(perf::SPAN_FIRST_ENCODE_DONE);
        }

        // Acquire span (blocking vsync/present wait — `nextDrawable`).
        let acquire_start = perf_on.then(Instant::now);
        let acquire_result = match encode_outcome {
            Ok(EncodeOutcome::Encoded) => renderer.acquire(render_cx),
            Ok(EncodeOutcome::Skipped) => Ok(AcquireOutcome::Skipped),
            Err(err) => Err(err),
        };
        let acquire_time = acquire_start.map_or(Duration::ZERO, |t| t.elapsed());

        // Submit span (blit + queue-submit + present — the present itself is
        // deferred to the UI thread under present-sync, see this fn's docs).
        let submit_start = perf_on.then(Instant::now);
        let render_result = match acquire_result {
            Ok(AcquireOutcome::Acquired) => {
                match present.filter(|(handoff, _)| handoff.is_armed()) {
                    Some((handoff, frame_id)) => {
                        renderer.submit_deferred(render_cx).map(|(outcome, frame)| {
                            if let Some(frame) = frame {
                                handoff.store(frame_id, frame);
                            }
                            outcome
                        })
                    }
                    None => renderer.submit(render_cx),
                }
            }
            Ok(AcquireOutcome::Reconfigured) => Ok(FrameOutcome::Redraw),
            Ok(AcquireOutcome::Lost) => Ok(FrameOutcome::SurfaceLost),
            Ok(AcquireOutcome::Skipped) => Ok(FrameOutcome::Skipped),
            Err(err) => Err(err),
        };
        let submit_time = submit_start.map_or(Duration::ZERO, |t| t.elapsed());

        match render_result {
            // Stale swapchain (e.g. mid-rotation): reconfigured internally; the
            // next tick draws against the fresh configuration.
            Ok(FrameOutcome::Redraw) => {}
            // Surface lost: dropped by the machine. The render loop self-heals from
            // the retained `CAMetalLayer` pointer on this same wakeup (split); the
            // inline path recovers on the next FFI entry (see `ffi_glue`).
            Ok(FrameOutcome::SurfaceLost) => {
                log::warn!("frust-shell-ios: surface lost; recreating");
            }
            Ok(FrameOutcome::Rendered) => {
                // A presented frame: bump the shared counter the UI thread reads
                // before paint. `Skipped` presents nothing, so it doesn't.
                presented.fetch_add(1, Ordering::Relaxed);
                // First successful present: close out the cold-start recorder once.
                if let Some(mut spans) = startup_spans.take() {
                    spans.record(perf::SPAN_FIRST_FRAME_PRESENTED);
                    spans.emit_log();
                }
            }
            Ok(FrameOutcome::Skipped) => {}
            Err(err) => log::error!("frust-shell-ios: render error: {err:#}"),
        }

        // One folded frame record through the single emitter, with this
        // surface's real GPU pass timing attached when it produces one
        // (engine tier, `perf-trace`, a device that offered `TIMESTAMP_QUERY`).
        let mut passes = FramePasses::from_split(
            ui,
            RenderSpans {
                encode: encode_time,
                acquire: acquire_time,
                submit: submit_time,
            },
        );
        if let Some(gpu) = gpu_passes(renderer) {
            passes = passes.with_gpu(gpu);
        }
        frame_stats.record(passes);
        if frame_stats.should_emit() {
            frame_stats.emit_log();
        }

        encode_time
    })
}
