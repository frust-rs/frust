//! The render-path half of the frame loop: the two executor arms
//! ([`FrameExecutor::Inline`] and [`FrameExecutor::Split`]) and the payload one
//! finished frame crosses the UI→render-thread handoff as.
//!
//! Everything the UI thread does *with* a painted scene lives here — the
//! uniform accessors [`super::AndroidAppHandle`] reads each frame, the split's
//! channel/lifecycle commands, and the inline fallback's synchronous tail (which
//! defers to [`super::render::render_scene`], the shared render body).

use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::thread::JoinHandle;
use std::time::Duration;

use frust_core::FrameTime;
use frust_render::{RenderContext, SurfacePhase, SurfaceRenderer};
use frust_scene::Scene;
use frust_shell_common::perf::{self, FramePasses, FrameStats, StartupSpans, UiSpans};
use frust_shell_common::{
    FrameMeta, RenderCommand, RenderSender, SceneFrame, SceneReturnReceiver, SurfaceSize,
};

use super::render::{RenderSignals, gpu_passes, render_scene};

/// One finished frame's payload crossing the UI→render-thread handoff in the
/// split: the painted [`Scene`] plus the clear color it was
/// painted for (the live theme's surface color — it must ride *with* the frame
/// so a mid-frame theme flip clears to the right color, mirroring the desktop
/// shell's `PaintedScene`). This is the `S` type parameter of
/// [`SceneFrame`]/[`render_channel`](frust_shell_common::render_channel); both
/// `Scene` and `peniko::Color` are `Send`, keeping the handoff `Send`-clean with
/// no `unsafe`.
pub(crate) struct PaintedScene {
    pub(crate) scene: Scene,
    pub(crate) base_color: peniko::Color,
}

/// The render-path half of the Android frame loop: either the
/// render-thread split ([`Self::Split`], default) or the pre-split inline
/// fallback ([`Self::Inline`], `FRUST_NO_RENDER_THREAD`). Chosen once at
/// construction from
/// [`render_thread_enabled`](frust_shell_common::render_thread_enabled) and
/// owned by [`super::AndroidAppHandle`].
pub(crate) enum FrameExecutor {
    /// Pre-split fallback: the [`RenderContext`]/[`SurfaceRenderer`] and all perf
    /// recording live on the UI thread, and the encode→acquire→submit tail runs
    /// synchronously inside [`super::AndroidAppHandle::frame`]. Boxed — it owns the whole
    /// render stack and dwarfs the split's thread-handle variant.
    Inline(Box<InlineExecutor>),
    /// The split: the renderer + context moved to a dedicated render thread; the
    /// UI thread hands it finished frames over the channel.
    Split(SplitExecutor),
}

/// The single-thread fallback executor (kill switch engaged): the
/// [`RenderContext`]/[`SurfaceRenderer`] and all perf recording live on the UI
/// thread, exactly as the pre-split shell did.
pub(crate) struct InlineExecutor {
    /// Read by [`super::AndroidAppHandle`]'s surface-lifecycle arms, which drive
    /// this thread's renderer directly (`pub(super)` for that reach alone).
    pub(super) render_cx: RenderContext,
    /// See [`Self::render_cx`] for why this is `pub(super)`.
    pub(super) renderer: SurfaceRenderer,
    frame_stats: FrameStats,
    /// The cold-start span recorder begun in [`crate::jni_glue::create_handle`];
    /// `Option::take`n on the first successful present (records
    /// [`perf::SPAN_FIRST_FRAME_PRESENTED`] + emits), `None` thereafter.
    startup_spans: Option<StartupSpans>,
    /// Monotonically increasing per-frame id, the inline mirror of
    /// [`SplitExecutor::frame_id`]. Inline renders synchronously and drops no
    /// scene, so submitted and presented ids stay in lockstep — the release
    /// gate then reads one uniform signal across both arms with no special
    /// case.
    frame_id: u64,
    /// What this executor publishes about its rendered frames (the presented
    /// count + the release gate's frame id + the tail's acquire EWMA — see
    /// [`RenderSignals`]). Inline renders on the UI thread, so this is written
    /// and read on the same thread — the `Arc` shape matches the split's
    /// cross-thread slot so [`FrameExecutor`] reads both variants uniformly.
    signals: Arc<RenderSignals>,
}

impl InlineExecutor {
    /// Build the fallback executor around the already-created, `SurfaceReady`
    /// renderer + context (surface creation and the early startup spans happened
    /// in [`crate::jni_glue::create_handle`], which hands `startup_spans` over
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
            frame_id: 0,
            signals: Arc::new(RenderSignals::default()),
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
        self.frame_id += 1;
        render_scene(
            &mut self.renderer,
            &self.render_cx,
            scene,
            base_color,
            ui,
            &mut self.frame_stats,
            &mut self.startup_spans,
            perf_on,
            &self.signals,
            self.frame_id,
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

/// Upper bound on how long `surfaceDestroyed` blocks the UI (JVM main) thread on
/// the [`RenderCommand::SurfaceDestroyed`] barrier before proceeding degraded
/// rather than releasing the `ANativeWindow` behind a wedged render thread.
///
/// **Community-approximate**: Android's exact per-callback watchdog for a
/// `SurfaceHolder.Callback` is not a published constant, but a blocked main
/// thread trips the ANR ("Application Not Responding") watchdog — 5s for input
/// dispatch (the published, longest of the ANR budgets;
/// developer.android.com/topic/performance/vitals/anr, retrieved 2026-07-22).
/// 2s stays safely under that so the barrier degrades before the system flags an
/// ANR; the barrier normally returns in microseconds (drop the surface's `wgpu`
/// resources), so a multi-second wait means the render thread is stuck.
const DESTROY_SURFACE_BARRIER_DEADLINE: Duration = Duration::from_secs(2);

/// The render-thread-split executor (default): the UI-thread [`RenderSender`]
/// half of the scene-handoff channel plus the render thread's [`JoinHandle`].
/// The render thread owns the [`RenderContext`]/[`SurfaceRenderer`], the surface,
/// the [`FrameStats`] recorder (the single perf emitter), and the startup line —
/// see [`crate::jni_glue::render_loop`].
pub(crate) struct SplitExecutor {
    /// `Option` so [`Drop`] can drop it *before* joining: dropping the sender is
    /// what signals the render loop to exit.
    sender: Option<RenderSender<PaintedScene, crate::jni_glue::SendableWindowPtr>>,
    join: Option<JoinHandle<()>>,
    /// The UI-side mirror of "a surface exists" (a `SurfaceCreated` was sent and
    /// not yet destroyed) — the render thread owns the real `SurfacePhase`, so the
    /// UI thread can't query it; this gates [`super::AndroidAppHandle::frame`]'s
    /// not-ready early return in place of `renderer.phase()`.
    surface_active: bool,
    /// Monotonically increasing per-frame id stamped into [`FrameMeta`].
    frame_id: u64,
    /// Fatal-signal flag: the render thread stores `true` here
    /// if its **first** surface install fails — unrecoverable (an incapable
    /// GPU/driver can't change mid-process). The UI thread reads it via
    /// [`super::AndroidAppHandle::render_fatal`] each `nativeOnFrame` and returns `false`
    /// to Kotlin so the Choreographer loop stops rather than driving doomed
    /// frames against a permanent black screen. One clone here, one in the render
    /// thread ([`crate::jni_glue::render_loop`]).
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
    /// What the render side publishes about its rendered frames (the presented
    /// count, the release gate's frame id, the scroll-sync tail's acquire
    /// EWMA): one clone here (read by the UI thread before paint via
    /// `FrameExecutor::presented_frames`/`presented_frame_id`/
    /// `acquire_wait_us`), one on the render thread
    /// ([`crate::jni_glue::render_loop`], which records into it from the render
    /// tail). A plain `Arc` — no channel/protocol, mirroring the `fatal`-flag
    /// pattern. See [`RenderSignals`] for why the count alone is not enough.
    signals: Arc<RenderSignals>,
}

impl SplitExecutor {
    pub(crate) fn new(
        sender: RenderSender<PaintedScene, crate::jni_glue::SendableWindowPtr>,
        join: JoinHandle<()>,
        fatal: Arc<AtomicBool>,
        scene_return: SceneReturnReceiver<Scene>,
        signals: Arc<RenderSignals>,
    ) -> Self {
        Self {
            sender: Some(sender),
            join: Some(join),
            surface_active: true,
            frame_id: 0,
            fatal,
            scene_return,
            spare_scene: None,
            signals,
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

    /// Send a [`RenderCommand::SurfaceChanged`] (in-place resize), fire-and-forget.
    pub(super) fn resize(&mut self, size: SurfaceSize) {
        if let Some(sender) = self.sender.as_ref() {
            sender.send_command(RenderCommand::SurfaceChanged { size });
        }
    }

    /// Send a [`RenderCommand::SurfaceCreated`] carrying the raw window pointer
    /// (wrapped `Send`), for the render thread to install a surface from.
    pub(super) fn send_surface_created(&mut self, ptr: *mut c_void, size: SurfaceSize) {
        if let Some(sender) = self.sender.as_ref() {
            sender.send_command(RenderCommand::SurfaceCreated {
                window: crate::jni_glue::SendableWindowPtr::new(ptr),
                size,
            });
        }
        self.surface_active = true;
    }

    /// Send a barriered [`RenderCommand::SurfaceDestroyed`] and **block** until the
    /// render thread has dropped its surface resources — the `ANativeWindow`
    /// release barrier: the caller (which owns the `NativeWindow`) must not
    /// release the window until this returns.
    pub(super) fn destroy_surface_barrier(&mut self) {
        if let Some(sender) = self.sender.as_ref() {
            // Bounded so a wedged render thread degrades instead of blocking the
            // JVM main thread into an ANR (see `DESTROY_SURFACE_BARRIER_DEADLINE`).
            if !sender
                .destroy_surface()
                .wait_timeout(DESTROY_SURFACE_BARRIER_DEADLINE)
            {
                log::error!(
                    "frust-shell-android: surface-destroy barrier timed out after \
                     {DESTROY_SURFACE_BARRIER_DEADLINE:?}; releasing the window (degraded)"
                );
            }
        }
        self.surface_active = false;
    }
}

impl Drop for SplitExecutor {
    fn drop(&mut self) {
        // Drop the sender first: that signals the render loop's `wait_next` to
        // wake with a disconnection and exit. Then join so the render thread's
        // final surface teardown completes before the UI thread drops the
        // `NativeWindow` the surface borrowed (the handle's `executor`-before-
        // `window` field order keeps that drop ordering).
        self.sender.take();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl FrameExecutor {
    /// Whether a live surface exists — the [`super::AndroidAppHandle::frame`] not-ready
    /// gate. Inline reads the renderer's phase directly; the split tracks a
    /// UI-side `surface_active` flag (the render thread owns the real phase).
    pub(super) fn has_surface(&self) -> bool {
        match self {
            FrameExecutor::Inline(inline) => inline.renderer.phase() == SurfacePhase::SurfaceReady,
            FrameExecutor::Split(split) => split.surface_active,
        }
    }

    /// The running count of frames the render side has actually presented
    /// (`FrameOutcome::Rendered`). The UI thread loads this once per frame and
    /// pushes it into `AppTree::set_presented_frames` before paint, so a widget
    /// measuring FPS reports the presented rate — under the split, that is far
    /// below the Choreographer's paint cadence. Both variants share
    /// the counter with their render side via an `Arc<AtomicU64>`.
    pub(super) fn presented_frames(&self) -> u64 {
        match self {
            FrameExecutor::Inline(inline) => inline.signals.count(),
            FrameExecutor::Split(split) => split.signals.count(),
        }
    }

    /// The id of the last frame the render side actually PRESENTED — the
    /// platform-view release gate's "is the frame that produced this geometry
    /// on screen yet?" input (see [`RenderSignals`]).
    pub(super) fn presented_frame_id(&self) -> u64 {
        match self {
            FrameExecutor::Inline(inline) => inline.signals.frame_id(),
            FrameExecutor::Split(split) => split.signals.frame_id(),
        }
    }

    /// The render tail's swapchain-acquire wait EWMA in µs — the scroll-sync
    /// tail's regime discriminator (see [`RenderSignals`] and
    /// [`crate::sync_tail`]). Read once per frame on the UI thread; both
    /// executor arms publish it the same way, so the tail sees one uniform
    /// signal whether or not the render split is engaged.
    pub(super) fn acquire_wait_us(&self) -> u64 {
        match self {
            FrameExecutor::Inline(inline) => inline.signals.acquire_ewma_us(),
            FrameExecutor::Split(split) => split.signals.acquire_ewma_us(),
        }
    }

    /// The id of the last frame **handed to** the render side — the gate's
    /// submission cursor, against which a batch whose own frame was dropped by
    /// the latest-wins channel is declared stale (`MAX_FRAMES_IN_FLIGHT`). The
    /// next frame to be submitted is therefore this + 1, which is the id a
    /// batch ingested during that frame's paint is paired with.
    pub(super) fn submitted_frame_id(&self) -> u64 {
        match self {
            FrameExecutor::Inline(inline) => inline.frame_id,
            FrameExecutor::Split(split) => split.frame_id,
        }
    }

    /// Record the `first_rebuild_done` startup milestone after the initial
    /// rebuild. Inline records it here on the UI thread; the split records it
    /// render-side when the first scene arrives (see [`crate::jni_glue::render_loop`]),
    /// so this is a no-op there.
    pub(super) fn record_first_rebuild(&mut self) {
        if let FrameExecutor::Inline(inline) = self {
            inline.record_first_rebuild();
        }
    }

    /// Record a gate-skipped frame. Inline accumulates it in its UI-side
    /// `FrameStats`; the split sends **nothing** on a skip (the render thread is
    /// the single emitter and never sees skipped frames), so
    /// this is a no-op there.
    pub(super) fn record_skip(&mut self) {
        if let FrameExecutor::Inline(inline) = self {
            inline.record_skip();
        }
    }

    /// Hand one finished frame to the executor. Inline runs the encode→present
    /// tail synchronously (borrowing `scene`, reused next frame) and returns its
    /// encode span; the split moves the scene out into a [`SceneFrame`] and
    /// sends it across the channel, replacing it with a scene reclaimed off the
    /// render thread's give-back channel — `reset()`, so its
    /// buffer is reused rather than reallocated — falling back to `Scene::new()`
    /// only when none is available yet, and returning `Duration::ZERO` (encode
    /// is off-thread, so it does not count against the UI thread's deadline).
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
