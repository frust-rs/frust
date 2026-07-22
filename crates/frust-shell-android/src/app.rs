//! The Android app runtime: [`AndroidAppHandle`], the state behind the opaque
//! JNI handle.
//!
//! This module is `#[cfg(target_os = "android")]`; it owns the same resources
//! the desktop shell's `ShellHandler` does — a [`RenderContext`],
//! [`SurfaceRenderer`], [`TextContext`], reusable [`Scene`], plus the app tree —
//! but is driven by Choreographer-posted JNI frames instead of a winit loop.
//! It contains no `unsafe`; the FFI boundary lives entirely in
//! [`crate::jni_glue`]. The `State`/`app_logic` erasure it drives
//! ([`AppTree`](frust_shell_common::AppTree)) is platform-agnostic and lives
//! in `frust-shell-common`.

use std::any::Any;
use std::collections::VecDeque;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use accesskit_android::InjectingAdapter;
use accesskit_android::jni as ak_jni;
use frust_core::FrameTime;
use frust_core::SemanticsUpdate;
use frust_core::accesskit::{
    Action, ActionHandler, ActionRequest, ActivationHandler, NodeId, Tree, TreeId, TreeUpdate,
};
use frust_core::event::{
    EditingState, ImeState, InputEvent, Key, KeyEvent, Modifiers, NamedKey, PointerButton,
    PointerEvent, PointerPhase,
};
use frust_core::insets::WindowInsets;
use frust_reactive::{ReactiveRuntime, TrackedScope, provide_context};
use frust_render::{
    AcquireOutcome, EncodeOutcome, FrameOutcome, RenderContext, SurfacePhase, SurfaceRenderer,
};
use frust_scene::{Scene, SceneBuilder};
use frust_shell_common::perf::{self, FramePasses, FrameStats, RenderSpans, StartupSpans, UiSpans};
use frust_shell_common::resample::{self, PointerResampler, RawPointerSample};
use frust_shell_common::{
    AppTree, FrameGate, FrameInputs, FrameMeta, RenderCommand, RenderSender, SceneFrame,
    SurfaceSize, ThemeOverrideWatcher, effective_brightness_for_platform_change, logical_insets,
    logical_size, sanitize_scale,
};
use frust_text::TextContext;
use frust_theme::{Brightness, Theme};
use kurbo::{Affine, Point, Size};
use ndk::native_window::NativeWindow;

use crate::ffi_support::TouchPhase;

/// Everything a running Android app needs across frames — the state behind the
/// opaque `jlong` handle the JVM passes back into every native call.
///
/// Field order is load-bearing for drop safety: `executor` (which — in the
/// render-thread split — owns the render thread whose `wgpu::Surface` was built
/// from `window`'s raw pointer, and — inline — owns the `SurfaceRenderer`
/// directly) is declared before `window`, so on drop the executor is torn down
/// (the split's `Drop` joins the render thread, dropping its surface) before the
/// [`NativeWindow`] it borrows is released (spec §8.1: no surface outlives its
/// window).
pub struct AndroidAppHandle {
    /// The render-path half of the frame loop (plan phase 11.B): either the
    /// render-thread split ([`FrameExecutor::Split`], the default) — where a
    /// dedicated thread owns the [`RenderContext`]/[`SurfaceRenderer`] + surface
    /// and the UI thread only hands it finished scenes — or the pre-split inline
    /// fallback ([`FrameExecutor::Inline`], `FRUST_NO_RENDER_THREAD`) where the
    /// renderer lives on this UI thread. Chosen once at construction.
    executor: FrameExecutor,
    text_ctx: TextContext,
    /// Reused across frames; `reset()` each frame rather than reallocated.
    scene: Scene,
    app: Box<dyn AppTree>,
    /// Records which signals the last per-frame rebuild read, so a later write
    /// to any of them trips the process-wide signals-dirty flag
    /// ([`TrackedScope::notify_dirty`] → `ReactiveRuntime::mark_signals_dirty`)
    /// that [`Self::frame`] drains via `take_signals_dirty` into
    /// [`FrameInputs::signals_dirty`]. Without this wrap a completed async load's
    /// signal write notifies no subscriber, so `signals_dirty` never trips and
    /// the frame gate skips the frame that would paint the loaded content until a
    /// touch forces a `Run` — the device-only "channel stuck on loading" stall
    /// (see device-parity task 09's root-cause). Mirrors the desktop shell's
    /// `ShellHandler::scope` (`frust-shell-desktop/src/app_handler.rs`):
    /// persistent across frames (not per-frame constructed) and re-tracked from
    /// scratch each `track`, so the sources frame N subscribes wake frame N+1.
    scope: TrackedScope,
    /// The current surface's physical (pixel) size, updated on create/resize and
    /// divided by `scale` to lay out in logical pixels.
    physical: (u32, u32),
    /// Display density (`resources.displayMetrics.density`), the device pixel
    /// ratio the whole scene is scaled by so glyphs rasterise sharp.
    scale: f32,
    /// The acquired window backing the current surface. Dropped after `renderer`
    /// (see the struct doc): its `Drop` calls `ANativeWindow_release`.
    window: Option<NativeWindow>,
    /// The app's active theme (M3 baseline). Mirrors the desktop shell's
    /// appearance ownership (task 05): starts [`Brightness::Light`] here and is
    /// flipped by [`Self::set_appearance`] once Kotlin reports the platform's
    /// real dark-mode preference (`nativeSetAppearance`, called right after
    /// `nativeInit` returns a handle — see `templates/app/android.tmpl`'s
    /// `FrustSurfaceView.surfaceCreated`).
    theme: Theme,
    /// The last window insets pushed to the render root (device-parity task 06),
    /// in logical px. Retained so [`Self::set_insets`] can skip a no-op push
    /// (`WindowInsets` is `PartialEq`) — both the `RenderRoot::set_insets` relayout
    /// and the app-side `provide_context` re-provide only fire on a real change.
    /// Starts zero (no occlusion) until Kotlin's first `nativeOnInsetsChanged`.
    insets: WindowInsets,
    /// Polls the process-wide app-facing theme override slot
    /// (`frust::set_app_theme`/`clear_app_theme`, task 6c-04) once per
    /// frame (see [`Self::frame`]) — see
    /// `frust_shell_common::theme_override`'s module docs.
    theme_override: ThemeOverrideWatcher,
    /// Whether an app-forced theme override is currently active. While `true`,
    /// [`Self::set_appearance`] must not flip `self.theme`'s brightness — the
    /// override wins entirely until `clear_app_theme` runs (see
    /// `effective_brightness_for_platform_change`).
    theme_override_active: bool,
    /// The platform's last-reported light/dark preference, tracked
    /// independently of `self.theme.brightness` so a `clear_app_theme` can
    /// restore exactly this value even if the platform reported a change
    /// *while* an override was active (during which `self.theme.brightness`
    /// itself does not move — see `effective_brightness_for_platform_change`).
    platform_brightness: Brightness,
    /// The accessibility adapter state (phase 6d D3), attached lazily by
    /// `nativeInitAccessibility` after `nativeInit` (see
    /// [`Self::attach_accessibility`]). `None` until then — a11y is best-effort
    /// and its wiring never gates the render path. This field is independent of
    /// the `renderer`/`window` drop-order contract above: [`InjectingAdapter`]'s
    /// `Drop` detaches the delegate through its own retained `JavaVM`, touching
    /// neither the surface nor the window.
    a11y: Option<AndroidA11y>,
    /// The skip-frame gate (task 17, spec §14 phase 7): consulted once per
    /// [`Self::frame`] after the per-tick inputs are gathered. When it returns
    /// [`FrameDecision::Skip`](frust_shell_common::FrameDecision::Skip) the
    /// frame's rebuild/layout/paint/encode/present passes are all skipped and
    /// only a `skipped` [`FramePasses`] is recorded — CPU/GPU stay near idle
    /// while nothing changes. The Choreographer keeps posting frames regardless
    /// (only frame *production* stops); the loop cadence is unchanged. Honors
    /// the [`FRUST_NO_FRAME_GATE`](frust_shell_common::frame_gate::NO_FRAME_GATE_VAR)
    /// kill switch (resolved once at construction) — a disabled gate always
    /// runs, matching pre-gate behavior verbatim.
    frame_gate: FrameGate,
    /// Latch: a pointer/IME event reached the tree since the last frame. Set by
    /// [`Self::dispatch_touch`]/[`Self::ime_apply`]/[`Self::ime_action`] (the
    /// JNI event entry points that run *between* frames), read-and-cleared each
    /// frame into [`FrameInputs::events_since_last_frame`]. This is what keeps a
    /// mid-drag gesture producing frames: Android delivers a continuous stream
    /// of `MotionEvent.ACTION_MOVE`s during a drag, each tripping this latch (so
    /// it also stands in for pointer-capture, which has no `AppTree` accessor —
    /// see [`Self::frame`]'s input-gathering).
    events_since_last_frame: bool,
    /// Latch: the GPU surface was (re)created or resized since the last frame.
    /// Set by [`Self::set_window`]/[`Self::resize`], read-and-cleared each frame
    /// into [`FrameInputs::surface_changed_or_resized`] (and, in the same frame,
    /// used to force the layout pass so the new dimensions take effect). Those
    /// same lifecycle transitions also open the gate's resume-warmup window (see
    /// [`FrameGate::note_resumed`]).
    surface_dirty: bool,
    /// Latch: the platform light/dark preference changed since the last frame
    /// (`nativeSetAppearance` → [`Self::set_appearance`]). Read-and-cleared each
    /// frame into [`FrameInputs::theme_or_appearance_changed`] (OR'd with the
    /// in-frame theme-override poll result). Belt-and-suspenders with the change
    /// flags [`Self::set_appearance`]'s `push_theme` already marks.
    appearance_dirty: bool,
    /// Latch: the previous paint pass asked for another frame
    /// ([`frust_core::PaintOutcome::needs_frame`] — a running animation/
    /// transition). Set from each run frame's paint return, read into
    /// [`FrameInputs::last_needs_frame`] so an in-flight animation keeps
    /// producing frames until it settles (whereupon paint returns `false` and
    /// the gate may skip again).
    last_needs_frame: bool,
    /// Whether the layout pass has run at least once. Until it has, the
    /// layout-skip seam in [`Self::frame`] force-runs layout (a paint before the
    /// first layout would have no valid geometry); after the first layout it is
    /// gated on the drained [`ChangeFlags`](frust_core::view::ChangeFlags)
    /// (or a surface resize).
    first_layout_done: bool,
    /// Pointer-event resampler (plan phase 10.C.1): buffers raw touch samples and
    /// emits frame-boundary-interpolated `Move`s (Down/Up/Cancel pass through
    /// losslessly). When disabled (the `FRUST_NO_RESAMPLE` kill switch, resolved
    /// once at construction) [`Self::dispatch_touch`] delivers touches directly,
    /// matching pre-resampling behavior verbatim. Its buffered-input signal ORs
    /// into the frame gate's `events_since_last_frame` so a too-new sample never
    /// starves the gate (see [`PointerResampler::has_pending`]).
    resampler: PointerResampler,
    /// The monotonic epoch every resampler timestamp is measured from — the raw
    /// samples ([`Self::dispatch_touch`]) and the per-frame sample query
    /// ([`Self::frame`]) are both stamped from this one `Instant`, so they share
    /// a clock domain (see `resample`'s *Clock domain* note). Deliberately
    /// decoupled from the Choreographer vsync clock threaded into [`FrameTime`]:
    /// resampling only differences timestamps, so a single consistent source is
    /// all it needs, and Choreographer's opaque `frameTimeNanos` is not exposed
    /// as an absolute `Instant`-comparable value.
    resample_clock: Instant,
    /// Scratch buffer the resampler drains into each frame, reused across frames
    /// (cleared, not reallocated) so a drag's per-frame resample allocates
    /// nothing on the hot path.
    pointer_scratch: Vec<PointerEvent>,
    /// The previous Choreographer tick's `frameTimeNanos`, for the deadline-
    /// aware pacing estimate (plan phase 10.C.2): the tick-to-tick delta is this
    /// frame's deadline budget (see [`resample::frame_interval_nanos`]). `None`
    /// before the first frame.
    last_frame_time_nanos: Option<u64>,
    /// Running count of frames whose measured work (rebuild+layout+paint+encode,
    /// excluding the vsync present wait) overran the frame-target deadline
    /// (plan phase 10.C.2). **Instrumentation only** — accumulated and logged
    /// (`frust-perf deadline`) behind [`perf::enabled`]; it never drops or
    /// reshapes work.
    deadline_overruns: u64,
}

/// One finished frame's payload crossing the UI→render-thread handoff in the
/// split (plan phase 11.B): the painted [`Scene`] plus the clear color it was
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

/// The render-path half of the Android frame loop (plan phase 11.B): either the
/// render-thread split ([`Self::Split`], default) or the pre-split inline
/// fallback ([`Self::Inline`], `FRUST_NO_RENDER_THREAD`). Chosen once at
/// construction from
/// [`render_thread_enabled`](frust_shell_common::render_thread_enabled) and
/// owned by [`AndroidAppHandle`].
pub(crate) enum FrameExecutor {
    /// Pre-split fallback: the [`RenderContext`]/[`SurfaceRenderer`] and all perf
    /// recording live on the UI thread, and the encode→acquire→submit tail runs
    /// synchronously inside [`AndroidAppHandle::frame`]. Boxed — it owns the whole
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
    render_cx: RenderContext,
    renderer: SurfaceRenderer,
    frame_stats: FrameStats,
    /// The cold-start span recorder begun in [`crate::jni_glue::create_handle`];
    /// `Option::take`n on the first successful present (records
    /// [`perf::SPAN_FIRST_FRAME_PRESENTED`] + emits), `None` thereafter.
    startup_spans: Option<StartupSpans>,
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
        )
    }

    /// Record a gate-skipped frame (all-zero pass durations) so the skip counter
    /// accumulates in the perf line, mirroring the pre-split inline behavior.
    fn record_skip(&mut self) {
        self.frame_stats.record(FramePasses {
            skipped: true,
            ..FramePasses::default()
        });
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
    /// UI thread can't query it; this gates [`AndroidAppHandle::frame`]'s
    /// not-ready early return in place of `renderer.phase()`.
    surface_active: bool,
    /// Monotonically increasing per-frame id stamped into [`FrameMeta`].
    frame_id: u64,
    /// Fatal-signal flag (phase-11 fix F2): the render thread stores `true` here
    /// if its **first** surface install fails — unrecoverable (an incapable
    /// GPU/driver can't change mid-process). The UI thread reads it via
    /// [`AndroidAppHandle::render_fatal`] each `nativeOnFrame` and returns `false`
    /// to Kotlin so the Choreographer loop stops rather than driving doomed
    /// frames against a permanent black screen. One clone here, one in the render
    /// thread ([`crate::jni_glue::render_loop`]).
    fatal: Arc<AtomicBool>,
}

impl SplitExecutor {
    pub(crate) fn new(
        sender: RenderSender<PaintedScene, crate::jni_glue::SendableWindowPtr>,
        join: JoinHandle<()>,
        fatal: Arc<AtomicBool>,
    ) -> Self {
        Self {
            sender: Some(sender),
            join: Some(join),
            surface_active: true,
            frame_id: 0,
            fatal,
        }
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
            sender.send_scene(SceneFrame {
                scene: painted,
                meta: FrameMeta {
                    frame_time,
                    size,
                    frame_id: self.frame_id,
                },
                ui_spans: ui,
            });
        }
    }

    /// Send a [`RenderCommand::SurfaceChanged`] (in-place resize), fire-and-forget.
    fn resize(&mut self, size: SurfaceSize) {
        if let Some(sender) = self.sender.as_ref() {
            sender.send_command(RenderCommand::SurfaceChanged { size });
        }
    }

    /// Send a [`RenderCommand::SurfaceCreated`] carrying the raw window pointer
    /// (wrapped `Send`), for the render thread to install a surface from.
    fn send_surface_created(&mut self, ptr: *mut c_void, size: SurfaceSize) {
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
    /// release barrier: the caller (which owns the [`NativeWindow`]) must not
    /// release the window until this returns.
    fn destroy_surface_barrier(&mut self) {
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
    /// Whether a live surface exists — the [`AndroidAppHandle::frame`] not-ready
    /// gate. Inline reads the renderer's phase directly; the split tracks a
    /// UI-side `surface_active` flag (the render thread owns the real phase).
    fn has_surface(&self) -> bool {
        match self {
            FrameExecutor::Inline(inline) => inline.renderer.phase() == SurfacePhase::SurfaceReady,
            FrameExecutor::Split(split) => split.surface_active,
        }
    }

    /// Record the `first_rebuild_done` startup milestone after the initial
    /// rebuild. Inline records it here on the UI thread; the split records it
    /// render-side when the first scene arrives (see [`crate::jni_glue::render_loop`]),
    /// so this is a no-op there.
    fn record_first_rebuild(&mut self) {
        if let FrameExecutor::Inline(inline) = self {
            inline.record_first_rebuild();
        }
    }

    /// Record a gate-skipped frame. Inline accumulates it in its UI-side
    /// `FrameStats`; the split sends **nothing** on a skip (task 09 contract —
    /// the render thread is the single emitter and never sees skipped frames), so
    /// this is a no-op there.
    fn record_skip(&mut self) {
        if let FrameExecutor::Inline(inline) = self {
            inline.record_skip();
        }
    }

    /// Hand one finished frame to the executor. Inline runs the encode→present
    /// tail synchronously (borrowing `scene`, reused next frame) and returns its
    /// encode span; the split moves the scene out (replacing it with a fresh
    /// empty one) into a [`SceneFrame`] and sends it across the channel,
    /// returning `Duration::ZERO` (encode is off-thread, so it does not count
    /// against the UI thread's deadline).
    fn submit_frame(
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
                let painted = PaintedScene {
                    scene: std::mem::replace(scene, Scene::new()),
                    base_color,
                };
                split.submit_frame(painted, ui, frame_time, size);
                Duration::ZERO
            }
        }
    }
}

/// Run the encode→acquire→submit tail for one painted `scene`, timing each span
/// behind `perf_on` (the FFI-path perf convention: zero clock reads when
/// disabled), recording the folded [`FramePasses`] through the single emitter
/// (`frame_stats`), and stamping the first-encode / first-frame startup
/// milestones on `startup_spans`. Returns the encode span. Shared by the inline
/// path (UI thread) and the split path's [`crate::jni_glue::render_loop`] (render
/// thread) so the per-frame render logic is not forked — the exact pre-split
/// tail, only relocated.
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
) -> Duration {
    // Encode span (GPU/CPU encode, no swapchain touch).
    let encode_start = perf_on.then(Instant::now);
    let encode_outcome = renderer.encode(render_cx, scene, base_color);
    let encode_time = encode_start.map_or(Duration::ZERO, |t| t.elapsed());

    // First-frame decomposition (task 10.A): stamp the first encode-complete
    // boundary once (only when something was actually encoded).
    if matches!(encode_outcome, Ok(EncodeOutcome::Encoded))
        && let Some(spans) = startup_spans.as_mut()
        && !spans
            .spans()
            .iter()
            .any(|(n, _)| *n == perf::SPAN_FIRST_ENCODE_DONE)
    {
        spans.record(perf::SPAN_FIRST_ENCODE_DONE);
    }

    // Acquire span (blocking vsync/present wait).
    let acquire_start = perf_on.then(Instant::now);
    let acquire_result = match encode_outcome {
        Ok(EncodeOutcome::Encoded) => renderer.acquire(render_cx),
        Ok(EncodeOutcome::Skipped) => Ok(AcquireOutcome::Skipped),
        Err(err) => Err(err),
    };
    let acquire_time = acquire_start.map_or(Duration::ZERO, |t| t.elapsed());

    // Submit span (blit + queue-submit + present).
    let submit_start = perf_on.then(Instant::now);
    let render_result = match acquire_result {
        Ok(AcquireOutcome::Acquired) => renderer.submit(render_cx),
        Ok(AcquireOutcome::Reconfigured) => Ok(FrameOutcome::Redraw),
        Ok(AcquireOutcome::Lost) => Ok(FrameOutcome::SurfaceLost),
        Ok(AcquireOutcome::Skipped) => Ok(FrameOutcome::Skipped),
        Err(err) => Err(err),
    };
    let submit_time = submit_start.map_or(Duration::ZERO, |t| t.elapsed());

    match render_result {
        // Stale swapchain (e.g. mid-rotation): reconfigured internally; the next
        // frame draws against the fresh configuration.
        Ok(FrameOutcome::Redraw) => {}
        // Surface lost: dropped by the machine; wait for surfaceChanged to
        // recreate it (Android pairs loss with a destroy/create cycle).
        Ok(FrameOutcome::SurfaceLost) => {
            log::warn!("frust-shell-android: surface lost; awaiting surfaceChanged");
        }
        Ok(FrameOutcome::Rendered) => {
            // First successful present: close out the cold-start recorder once.
            if let Some(mut spans) = startup_spans.take() {
                spans.record(perf::SPAN_FIRST_FRAME_PRESENTED);
                spans.emit_log();
            }
        }
        Ok(FrameOutcome::Skipped) => {}
        Err(err) => log::error!("frust-shell-android: render error: {err:#}"),
    }

    // One folded frame record through the single emitter (plan phase 11.B.3).
    frame_stats.record(FramePasses::from_split(
        ui,
        RenderSpans {
            encode: encode_time,
            acquire: acquire_time,
            submit: submit_time,
        },
    ));
    if frame_stats.should_emit() {
        frame_stats.emit_log();
    }

    encode_time
}

/// Per-handle accessibility state (phase 6d D3): the injecting accesskit Android
/// adapter plus the two UI-thread-shared channels its handlers use to talk to
/// the frame loop.
///
/// # Threading model (verified against `accesskit_android` 0.7.5)
///
/// Every accesskit callback into this shell — `request_initial_tree` (via
/// `AccessibilityNodeProvider.createAccessibilityNodeInfo`/`findFocus`) and
/// `do_action` (via `performAction`) — is dispatched by the Android
/// accessibility framework on the app's **main (UI) thread**, the very thread
/// the Choreographer frame loop and JNI touch/IME entry points already run on.
/// So there is no true concurrency: the two `Mutex`es below carry only the
/// `Send` bound the `ActionHandler`/`ActivationHandler` traits require (both are
/// `Send + 'static` and owned by the adapter), never real contention. Crucially,
/// neither handler ever touches the [`AndroidAppHandle`]: `do_action` only
/// enqueues, and the `&mut self` frame pass drains that queue and calls
/// [`AppTree::perform_accessibility_action`](frust_shell_common::AppTree::perform_accessibility_action)
/// itself — so an assistive-tech action can never alias the handle mid-frame.
struct AndroidA11y {
    /// The accesskit_android injecting adapter. On construction it attaches a
    /// `View.AccessibilityDelegate` + `OnHoverListener` to the host
    /// `FrustSurfaceView` (posted to the UI thread), and it pushes
    /// `TreeUpdate`s via [`InjectingAdapter::update_if_active`]. Every push is a
    /// cheap no-op until a screen reader activates the tree.
    adapter: InjectingAdapter,
    /// Actions requested by assistive tech: pushed by the adapter's
    /// `ActionHandler` ([`ForgeActionHandler::do_action`]) and drained by the
    /// frame loop ([`AndroidAppHandle::apply_pending_accessibility_actions`]).
    pending_actions: Arc<Mutex<VecDeque<(NodeId, Action)>>>,
    /// The latest full accessibility tree, published by the frame loop and read
    /// by the adapter's `ActivationHandler`
    /// ([`ForgeActivationHandler::request_initial_tree`]) the moment a screen
    /// reader activates, so it sees real content instead of the adapter's
    /// placeholder window. `None` until the first post-layout semantics pass.
    tree_snapshot: Arc<Mutex<Option<TreeUpdate>>>,
    /// The semantics generation last assembled and pushed, so the frame loop
    /// skips reassembling+re-pushing an unchanged tree (via
    /// [`AppTree::semantics_if_changed`](frust_shell_common::AppTree::semantics_if_changed)).
    last_pushed_gen: u64,
}

impl AndroidA11y {
    /// Build the adapter and its shared channels, attaching accessibility to
    /// `host` (the `FrustSurfaceView`). Contains no `unsafe`: the raw-pointer
    /// bridge from this shell's `jni` 0.22 to accesskit_android's `jni` 0.21
    /// lives at the FFI boundary in [`crate::jni_glue::native_init_accessibility`];
    /// this method receives already-bridged references.
    ///
    /// [`InjectingAdapter::new`] can panic on a JNI error (e.g. a missing
    /// `dev.accesskit.android.Delegate` class); the caller's `guard` wrapper
    /// catches it, leaving the handle with no a11y rather than failing.
    fn new(env: &mut ak_jni::JNIEnv, host: &ak_jni::objects::JObject) -> Self {
        let pending_actions: Arc<Mutex<VecDeque<(NodeId, Action)>>> =
            Arc::new(Mutex::new(VecDeque::new()));
        let tree_snapshot: Arc<Mutex<Option<TreeUpdate>>> = Arc::new(Mutex::new(None));
        let adapter = InjectingAdapter::new(
            env,
            host,
            ForgeActivationHandler {
                tree_snapshot: tree_snapshot.clone(),
            },
            ForgeActionHandler {
                pending_actions: pending_actions.clone(),
            },
        );
        Self {
            adapter,
            pending_actions,
            tree_snapshot,
            last_pushed_gen: 0,
        }
    }
}

/// accesskit `ActivationHandler`: hands a newly-activated screen reader the
/// latest full tree the frame loop published (or `None` before the first
/// semantics pass, in which case the adapter serves its own placeholder until
/// the next `update_if_active`). Reads the shared snapshot slot only — never the
/// [`AndroidAppHandle`]. Runs on the Android UI thread.
struct ForgeActivationHandler {
    tree_snapshot: Arc<Mutex<Option<TreeUpdate>>>,
}

impl ActivationHandler for ForgeActivationHandler {
    fn request_initial_tree(&mut self) -> Option<TreeUpdate> {
        self.tree_snapshot.lock().ok().and_then(|slot| slot.clone())
    }
}

/// accesskit `ActionHandler`: records a requested action for the frame loop to
/// apply against the retained tree. It only enqueues — being `Send + 'static`
/// and owned by the adapter, it has no access to the [`AndroidAppHandle`], so it
/// cannot alias the `&mut self` frame pass that drains it. Runs on the Android
/// UI thread, the same thread as that frame pass.
struct ForgeActionHandler {
    pending_actions: Arc<Mutex<VecDeque<(NodeId, Action)>>>,
}

impl ActionHandler for ForgeActionHandler {
    fn do_action(&mut self, request: ActionRequest) {
        if let Ok(mut queue) = self.pending_actions.lock() {
            queue.push_back((request.target_node, request.action));
        }
    }
}

/// Assemble an accesskit [`TreeUpdate`] from a [`SemanticsUpdate`] (phase 6d D3).
///
/// v1 always publishes the whole tree (`RenderRoot::semantics` recomputes it in
/// full): every node is a "new or changed" entry, `tree` names the root, and
/// `focus` is the focused node or the root fallback
/// ([`SemanticsUpdate::focus_id`]). `tree_id` is always [`TreeId::ROOT`] — this
/// shell drives a single, non-subtree accessibility tree.
fn tree_update_from_semantics(update: &SemanticsUpdate) -> TreeUpdate {
    TreeUpdate {
        nodes: update.nodes.clone(),
        tree: Some(Tree::new(update.root)),
        tree_id: TreeId::ROOT,
        focus: update.focus_id(),
    }
}

impl AndroidAppHandle {
    /// Assemble a handle with an already-created, `SurfaceReady` renderer.
    ///
    /// Called from [`crate::jni_glue`] after the surface has been built from the
    /// `NativeWindow`; runs the first rebuild so the tree exists before the first
    /// frame. No `unsafe` here — the window acquisition and surface creation
    /// happen at the FFI boundary.
    ///
    /// Seeds the M3 baseline theme (Light, until Kotlin's follow-up
    /// `nativeSetAppearance` reports the real preference) into both delivery
    /// paths (`AppTree::set_theme` for widgets, `provide_context` for app code)
    /// before the first rebuild, mirroring the desktop shell's `apply_theme`.
    /// Must be called under the root reactive `Owner` (see
    /// `crate::jni_glue::create_handle`) so `provide_context` isn't a silent
    /// no-op.
    ///
    /// `executor` is the render-path half [`crate::jni_glue::create_handle`]
    /// already built (plan phase 11.B): the render-thread split
    /// ([`FrameExecutor::Split`], with the render thread already spawned and its
    /// initial `SurfaceCreated` sent) or the inline fallback
    /// ([`FrameExecutor::Inline`], with the surface + early startup spans already
    /// created on this UI thread). The initial `rebuild()` below records
    /// [`perf::SPAN_FIRST_REBUILD_DONE`] via [`FrameExecutor::record_first_rebuild`]
    /// — inline records it here, the split records it render-side on the first
    /// handed-off scene.
    ///
    /// `text_ctx` is the [`TextContext`] `create_handle` already resolved
    /// (phase 10.D) — the pre-built one from `JNI_OnLoad`'s background
    /// font-preload thread when it finished in time, or a synchronous
    /// fallback otherwise (see `jni_glue::take_preinit_text_context`) — so
    /// this method never itself pays the font-DB load cost.
    pub(crate) fn new(
        executor: FrameExecutor,
        text_ctx: TextContext,
        window: NativeWindow,
        physical: (u32, u32),
        scale: f32,
        mut app: Box<dyn AppTree>,
    ) -> Self {
        let theme = Theme::m3_baseline();
        app.set_theme(Box::new(theme.clone()));
        provide_context(theme.clone());
        app.rebuild();
        let mut executor = executor;
        executor.record_first_rebuild();
        // Seed the resume-warmup window so the first handful of frames run
        // unconditionally (surface just came up; the first tick's change
        // signals may not yet be observable — see `FrameGate::note_resumed`).
        // The initial `rebuild()` above also leaves change flags pending, which
        // independently forces the first frame to run and to lay out.
        let mut frame_gate = FrameGate::new();
        frame_gate.note_resumed();
        Self {
            executor,
            text_ctx,
            scene: Scene::new(),
            app,
            physical,
            scale,
            window: Some(window),
            scope: TrackedScope::new(),
            theme,
            insets: WindowInsets::default(),
            theme_override: ThemeOverrideWatcher::new(),
            theme_override_active: false,
            platform_brightness: Brightness::Light,
            a11y: None,
            frame_gate,
            events_since_last_frame: false,
            surface_dirty: false,
            appearance_dirty: false,
            last_needs_frame: false,
            first_layout_done: false,
            resampler: PointerResampler::new(),
            resample_clock: Instant::now(),
            pointer_scratch: Vec::new(),
            last_frame_time_nanos: None,
            deadline_overruns: 0,
        }
    }

    /// Attach the accesskit Android adapter to the host `FrustSurfaceView`
    /// (phase 6d D3, `nativeInitAccessibility`).
    ///
    /// `env`/`host` are the bridged (this shell's `jni` 0.22 → accesskit_android's
    /// `jni` 0.21) references built at the FFI boundary in
    /// [`crate::jni_glue::native_init_accessibility`]. Idempotent: a second call
    /// (e.g. a spurious re-init) is ignored, so the adapter — which panics if the
    /// host already has an accessibility delegate — is only ever created once per
    /// handle. Any panic constructing the adapter (a JNI hiccup, a missing
    /// `Delegate` class) unwinds into the caller's `guard`, leaving `self.a11y`
    /// as it was (`None`): accessibility is best-effort and never blocks the app.
    pub(crate) fn attach_accessibility(
        &mut self,
        env: &mut ak_jni::JNIEnv,
        host: &ak_jni::objects::JObject,
    ) {
        if self.a11y.is_some() {
            return;
        }
        self.a11y = Some(AndroidA11y::new(env, host));
    }

    /// Drain and apply any accessibility actions assistive tech queued since the
    /// last frame (phase 6d D3), routing each to
    /// [`AppTree::perform_accessibility_action`](frust_shell_common::AppTree::perform_accessibility_action).
    ///
    /// Drained into a local `Vec` first so the shared queue lock is released
    /// before `self.app` is touched — this both keeps the lock hold-time
    /// minimal and sidesteps borrowing `self.a11y` across the `&mut self.app`
    /// calls. The `needs_redraw` each action returns is implicit here: the
    /// continuous Choreographer loop already reconsiders the frame every tick.
    ///
    /// Returns whether any action was applied this call, which [`Self::frame`]
    /// feeds into [`FrameInputs::a11y_action_performed`] so an assistive-tech
    /// action forces the frame to run (the state it mutated must be reflected).
    fn apply_pending_accessibility_actions(&mut self) -> bool {
        let drained: Vec<(NodeId, Action)> = match self.a11y.as_ref() {
            Some(a11y) => match a11y.pending_actions.lock() {
                Ok(mut queue) => queue.drain(..).collect(),
                Err(_) => Vec::new(),
            },
            None => return false,
        };
        let performed = !drained.is_empty();
        for (node_id, action) in drained {
            let _ = self.app.perform_accessibility_action(node_id.0, action);
        }
        performed
    }

    /// Publish the current accessibility tree to the adapter (phase 6d D3), if it
    /// changed since the last push. Must run **after** [`Self::frame`]'s layout
    /// so node bounds are valid.
    ///
    /// Gated twice over: the [`AppTree::semantics_if_changed`](frust_shell_common::AppTree::semantics_if_changed)
    /// generation check skips reassembling an unchanged tree here, and the
    /// adapter's own `update_if_active` is a cheap no-op until a screen reader
    /// activates. The assembled tree is also snapshotted for a late-activating
    /// screen reader's `request_initial_tree`, so it sees real content rather
    /// than the adapter's placeholder window.
    fn publish_semantics(&mut self) {
        // Cheap generation gate first (immutable `a11y` borrow, released before
        // the `&mut self.app` call below).
        let last_gen = match self.a11y.as_ref() {
            Some(a11y) => a11y.last_pushed_gen,
            None => return,
        };
        let Some(update) = self.app.semantics_if_changed(last_gen) else {
            return; // tree unchanged since the last push
        };
        let current_gen = self.app.semantics_generation();
        let tree_update = tree_update_from_semantics(&update);
        if let Some(a11y) = self.a11y.as_mut() {
            if let Ok(mut slot) = a11y.tree_snapshot.lock() {
                *slot = Some(tree_update.clone());
            }
            a11y.adapter.update_if_active(move || tree_update);
            a11y.last_pushed_gen = current_gen;
        }
    }

    /// `nativeSetAppearance`: flip the theme's brightness and re-push it to both
    /// delivery paths (mirrors the desktop shell's `apply_theme`, task 05).
    ///
    /// Runs the `provide_context` re-provide under the process-wide root
    /// [`ReactiveRuntime`]'s owner (fetched fresh here, since — unlike
    /// [`Self::new`] — this call arrives on its own JNI entry, not nested inside
    /// `create_handle`'s `with_owner` wrap). No explicit redraw is scheduled —
    /// the continuous Choreographer loop already repaints every tick.
    pub(crate) fn set_appearance(&mut self, dark: bool) {
        let platform = match crate::ffi_support::appearance_from_dark(dark) {
            crate::ffi_support::Appearance::Dark => Brightness::Dark,
            crate::ffi_support::Appearance::Light => Brightness::Light,
        };
        self.platform_brightness = platform;
        // Override-wins rule (task 6c-04): while an app-forced theme override
        // is active, this platform-appearance report must not flip brightness.
        self.theme.brightness = effective_brightness_for_platform_change(
            self.theme_override_active,
            self.theme.brightness,
            platform,
        );
        // Frame-gate input (task 17): an appearance change must force the next
        // frame to run so the re-themed tree repaints. `push_theme` below also
        // marks LAYOUT|PAINT change flags, so this is belt-and-suspenders with
        // `change_flags_pending` — but it maps the appearance edit onto its own
        // `theme_or_appearance_changed` input directly.
        self.appearance_dirty = true;
        self.push_theme();
    }

    /// Push the current [`Self::theme`] to both delivery paths — boxed
    /// type-erased into the render root ([`AppTree::set_theme`]) and
    /// re-`provide_context`ed under the process-wide root
    /// [`ReactiveRuntime`]'s owner for app-side `use_context` reads.
    ///
    /// This IS the shared theme-delivery body: both [`Self::set_appearance`]
    /// (after it resolves the new brightness) and [`Self::frame`]'s
    /// theme-override poll call it, so a forced override and a live appearance
    /// change go through one code path.
    fn push_theme(&mut self) {
        self.app.set_theme(Box::new(self.theme.clone()));
        let theme = self.theme.clone();
        match ReactiveRuntime::get() {
            Some(rt) => rt.with_owner(|| provide_context(theme)),
            None => provide_context(theme),
        }
    }

    /// The current surface's window, if any (used by [`crate::jni_glue`] to
    /// compare against a newly delivered `Surface` before deciding whether a
    /// `surfaceChanged` is a resize or a recreate).
    pub(crate) fn window(&self) -> Option<&NativeWindow> {
        self.window.as_ref()
    }

    /// Whether the render-thread split is engaged (as opposed to the inline
    /// fallback) — the FFI layer branches surface (re)creation on this: the split
    /// routes it through owned channel commands (safe), the inline path drives the
    /// `unsafe` surface creation directly with [`Self::inline_renderer_mut`].
    pub(crate) fn executor_is_split(&self) -> bool {
        matches!(self.executor, FrameExecutor::Split(_))
    }

    /// Whether the render thread signalled a fatal, unrecoverable first-surface
    /// install failure (phase-11 fix F2), read by
    /// [`crate::jni_glue::native_on_frame`] to tell Kotlin to stop the
    /// Choreographer loop. The split reads its shared `fatal` flag; the inline
    /// fallback never faults here — a failed first install returns `Err` from
    /// `create_handle`, so `nativeInit` yields the `0` handle and no frame is ever
    /// driven — so it is always `false`.
    pub(crate) fn render_fatal(&self) -> bool {
        match &self.executor {
            FrameExecutor::Split(split) => split.fatal.load(Ordering::Acquire),
            FrameExecutor::Inline(_) => false,
        }
    }

    /// Access to the inline render context + renderer for the FFI layer to drive
    /// an `unsafe` surface *creation* (the one lifecycle transition that crosses
    /// the raw-pointer boundary). `None` in the render-thread split — there the
    /// render thread owns the renderer and surface (re)creation is a channel
    /// command (see [`Self::split_recreate_surface`] / the render loop).
    pub(crate) fn inline_renderer_mut(
        &mut self,
    ) -> Option<(&mut RenderContext, &mut SurfaceRenderer)> {
        match &mut self.executor {
            FrameExecutor::Inline(inline) => Some((&mut inline.render_cx, &mut inline.renderer)),
            FrameExecutor::Split(_) => None,
        }
    }

    /// Recreate the surface against `new_window` in the render-thread split
    /// (the inline path drives its own `unsafe` recreation in the FFI layer).
    ///
    /// The window-release ordering is the split's real hazard: the render thread
    /// holds the OLD surface built from the OLD window's raw pointer, so the UI
    /// thread must not release the old [`NativeWindow`] until the render thread has
    /// dropped that surface. So this **first** blocks on a barriered
    /// `SurfaceDestroyed` (the render thread drops the old surface and acks),
    /// **then** releases the old window, **then** hands the new window's pointer
    /// across for the render thread to build a fresh surface — the new window is
    /// retained here to keep its `ANativeWindow` alive for that new surface.
    pub(crate) fn split_recreate_surface(
        &mut self,
        new_window: NativeWindow,
        physical: (u32, u32),
        density: f32,
    ) {
        // Read the new window's raw pointer before it is moved into `self.window`
        // below (moving the wrapper leaves the underlying `ANativeWindow` — and
        // thus this pointer — unchanged).
        let ptr = new_window.ptr().as_ptr().cast::<c_void>();
        if let FrameExecutor::Split(split) = &mut self.executor {
            // Barrier: the render thread drops the old surface and acks before we
            // release the old window.
            split.destroy_surface_barrier();
        }
        // Old surface gone → releasing the old `NativeWindow`
        // (`ANativeWindow_release` via its `Drop`) is now safe.
        self.window = None;
        self.physical = physical;
        self.scale = density;
        self.window = Some(new_window);
        if let FrameExecutor::Split(split) = &mut self.executor {
            split.send_surface_created(
                ptr,
                SurfaceSize {
                    width: physical.0,
                    height: physical.1,
                    scale: density as f64,
                },
            );
        }
        // Surface (re)creation: force the next frame to run + lay out at the new
        // dimensions and open the resume-warmup window (task 17).
        self.surface_dirty = true;
        self.frame_gate.note_resumed();
    }

    /// Record the window + physical size backing a freshly (re)created surface.
    ///
    /// Assigning `window` last drops the previous [`NativeWindow`] (releasing it)
    /// — safe here because the previous surface was already torn down inside the
    /// preceding `on_surface_created_from_android_window` (spec §8.1).
    ///
    /// `density` is this configuration's `displayMetrics.density` (device-parity
    /// task 06): stored raw and re-sanitized at every use (layout/paint/insets),
    /// so a config change that alters the device pixel ratio takes effect on the
    /// next frame. Stored raw for the same reason `nativeInit`'s `scale` is —
    /// `sanitize_scale` runs once per frame at the point of use.
    pub(crate) fn set_window(&mut self, window: NativeWindow, physical: (u32, u32), density: f32) {
        self.physical = physical;
        self.scale = density;
        self.window = Some(window);
        // Surface (re)creation: force the next frame to run (and lay out at the
        // new dimensions) and open the gate's resume-warmup window — the first
        // ticks after a surface swap must not be gated away (task 17).
        self.surface_dirty = true;
        self.frame_gate.note_resumed();
    }

    /// Resize the live surface in place (same window, new dimensions). Safe: no
    /// raw pointers — inline reconfigures the renderer's swapchain directly; the
    /// split sends a [`RenderCommand::SurfaceChanged`] command.
    ///
    /// `density` re-sanitizes and stores the display's device pixel ratio for
    /// this configuration (device-parity task 06 — see [`Self::set_window`]).
    pub(crate) fn resize_surface(&mut self, physical: (u32, u32), density: f32) {
        match &mut self.executor {
            FrameExecutor::Inline(inline) => {
                inline
                    .renderer
                    .on_surface_changed(&inline.render_cx, physical.0, physical.1)
            }
            FrameExecutor::Split(split) => split.resize(SurfaceSize {
                width: physical.0,
                height: physical.1,
                scale: density as f64,
            }),
        }
        self.physical = physical;
        self.scale = density;
        // In-place resize: force the next frame to run and relayout at the new
        // size, and open the warmup window (task 17) — same rationale as
        // `set_window`.
        self.surface_dirty = true;
        self.frame_gate.note_resumed();
    }

    /// `nativeOnInsetsChanged`: convert the platform's physical-px per-edge insets
    /// to logical px with the stored scale and push them onto the render root
    /// (device-parity task 06). Mirrors [`Self::set_appearance`]'s two-path
    /// delivery shape but for insets: [`AppTree::set_insets`] threads them into
    /// layout/paint (widget path — a `SafeArea`'s `LayoutCtx::window_insets`), and
    /// [`Self::push_insets`] re-`provide_context`s them for app code
    /// (`use_context::<WindowInsets>()` in `Component::build`).
    ///
    /// `physical` is the eight-value pack `logical_insets` expects (`view_padding`
    /// then `view_insets`, each l/t/r/b — see [`logical_insets`]). No-op-guarded on
    /// `PartialEq`: a shell that re-reports unchanged insets neither relayouts nor
    /// re-provides. On a real change, `RenderRoot::set_insets` marks `LAYOUT |
    /// PAINT` pending (task 01), which the frame gate already treats as
    /// dirty (`change_flags_pending`) — no new gate input needed. The continuous
    /// Choreographer loop repaints the next tick with no extra wake.
    pub(crate) fn set_insets(&mut self, physical: [f64; 8]) {
        let scale = sanitize_scale(self.scale);
        let insets = logical_insets(physical, scale);
        if insets == self.insets {
            return; // no-op push — skip both the relayout and the re-provide
        }
        self.insets = insets;
        self.push_insets(insets);
    }

    /// Push the current [`WindowInsets`] to both delivery paths — into the render
    /// root ([`AppTree::set_insets`], the widget/layout path) and
    /// re-`provide_context`ed under the process-wide root [`ReactiveRuntime`]'s
    /// owner for app-side `use_context::<WindowInsets>()` reads. Mirrors
    /// [`Self::push_theme`]'s shape exactly (the theme re-provide precedent).
    fn push_insets(&mut self, insets: WindowInsets) {
        self.app.set_insets(insets);
        match ReactiveRuntime::get() {
            Some(rt) => rt.with_owner(|| provide_context(insets)),
            None => provide_context(insets),
        }
    }

    /// Tear the surface down (`surfaceDestroyed`): drop the surface **first**,
    /// then release the [`NativeWindow`] it borrowed (spec §8.1).
    ///
    /// The ordering is a hard correctness contract in the render-thread split:
    /// the render thread owns the surface built from `self.window`'s raw pointer,
    /// so [`SplitExecutor::destroy_surface_barrier`] sends a barriered
    /// `SurfaceDestroyed` and **blocks until the render thread has acknowledged**
    /// dropping that surface. Only after that ack returns do we set
    /// `self.window = None`, releasing the `ANativeWindow` — so the render thread
    /// can never touch the window after it is released (the plan's Android
    /// surface-lifecycle-race hazard). The inline path drops its surface
    /// synchronously above, so the same window-after-surface order holds there.
    pub(crate) fn destroy_surface(&mut self) {
        match &mut self.executor {
            FrameExecutor::Inline(inline) => inline.renderer.on_surface_destroyed(),
            FrameExecutor::Split(split) => split.destroy_surface_barrier(),
        }
        // Barrier complete (split) / surface dropped (inline): releasing the
        // window is now safe.
        self.window = None;
    }

    /// Deliver one touch contact to the tree (spec §9), converting the incoming
    /// physical view-local coordinates into the logical space the tree lays out
    /// in — the same `sanitize_scale` value `frame()` uses, so hit-testing and
    /// layout never disagree.
    ///
    /// Single-pointer in v1: the Kotlin side forwards only the primary pointer,
    /// so every contact is a [`PointerButton::Primary`] event. The redraw the
    /// tree requests is implicit here — the Choreographer loop already posts a
    /// frame every vsync, so the mutated state is picked up on the next
    /// `frame()` without an explicit schedule (contrast the desktop shell's
    /// `request_redraw`).
    pub(crate) fn dispatch_touch(&mut self, phase: TouchPhase, x: f32, y: f32) {
        let scale = sanitize_scale(self.scale);
        let position = Point::new(x as f64 / scale, y as f64 / scale);
        let core_phase = match phase {
            TouchPhase::Down => PointerPhase::Down,
            TouchPhase::Move => PointerPhase::Move,
            TouchPhase::Up => PointerPhase::Up,
            TouchPhase::Cancel => PointerPhase::Cancel,
        };
        // Frame-gate latch (task 17): an event between frames must force the
        // next frame to run so the tree reflects the dispatch. Set even on a
        // no-op dispatch — correctness beats savings, and the gate defaults to
        // "must run" when in doubt.
        self.events_since_last_frame = true;

        // Pointer resampling (plan phase 10.C.1): buffer the raw sample (stamped
        // on the shared resample clock) so [`Self::frame`] can emit a
        // frame-boundary-interpolated position; Down/Up/Cancel still pass through
        // losslessly. When the kill switch disabled the resampler, deliver
        // directly instead — pre-resampling behavior verbatim.
        if self.resampler.is_enabled() {
            let time_nanos = self.resample_clock.elapsed().as_nanos() as u64;
            self.resampler.push(RawPointerSample {
                phase: core_phase,
                position,
                button: PointerButton::Primary,
                time_nanos,
            });
        } else {
            let event = InputEvent::Pointer(PointerEvent {
                phase: core_phase,
                position,
                button: PointerButton::Primary,
            });
            let _ = self.app.event(&event);
        }
    }

    /// Apply a whole editing state pushed by the platform IME (`nativeImeApply`),
    /// routing it to the focused widget as an
    /// [`InputEvent::Ime`]`(`[`ImeEvent::ApplyEditingState`]`)` via
    /// [`AppTree::ime_apply`]. `state`'s indices are UTF-16 code units (the seam
    /// unit); the focused widget converts them. No explicit redraw is scheduled —
    /// the Choreographer loop already posts the next frame (see [`Self::dispatch_touch`]).
    ///
    /// [`ImeEvent::ApplyEditingState`]: frust_core::event::ImeEvent::ApplyEditingState
    pub(crate) fn ime_apply(&mut self, state: EditingState) {
        // Frame-gate latch (task 17): an IME edit between frames forces the next
        // frame to run (see `dispatch_touch`).
        self.events_since_last_frame = true;
        let _ = self.app.ime_apply(state);
    }

    /// The IME surface the focused widget published, for the FFI layer to
    /// serialise into the `nativeImeState` JSON. Delegates to
    /// [`AppTree::ime_state`]; `None` when nothing is focused.
    pub(crate) fn ime_state(&self) -> Option<ImeState> {
        self.app.ime_state()
    }

    /// Forward a soft-keyboard editor action (`nativeImeAction`, e.g.
    /// `IME_ACTION_DONE`) as an [`NamedKey::Enter`] key press down the focus path.
    ///
    /// `action` is retained for future differentiation; v1 configures only
    /// `IME_ACTION_DONE`, so every action maps to `Enter`. Reuses the same
    /// focus-routed key path a hardware Enter would (spec §9), so a widget's
    /// submit/newline handling stays in one place.
    pub(crate) fn ime_action(&mut self, _action: i32) {
        let event = InputEvent::Key(KeyEvent {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers::default(),
            repeat: false,
        });
        // Frame-gate latch (task 17): a soft-keyboard action forces the next
        // frame to run (see `dispatch_touch`).
        self.events_since_last_frame = true;
        let _ = self.app.event(&event);
    }

    /// Run one frame: rebuild → layout → paint → render, mirroring the desktop
    /// shell's `RedrawRequested` path (spec §8) but driven by Choreographer.
    ///
    /// `frame_time_nanos` is Kotlin's Choreographer `frameTimeNanos` for this
    /// tick (already clamped non-negative at the JNI boundary — see
    /// [`crate::jni_glue::native_on_frame`]), the shell-owned monotonic clock
    /// threaded into [`FrameTime`] (spec §8: `frust-core` never reads a clock
    /// itself).
    ///
    /// A no-op when the surface isn't `SurfaceReady` (Kotlin keeps posting frames
    /// across surface loss; this makes those cheap). On `FrameOutcome::SurfaceLost`
    /// the machine has already dropped the surface; recovery waits for the next
    /// `surfaceChanged`/`surfaceCreated` rather than recreating mid-frame.
    ///
    /// # Frame gate (task 17, spec §14 phase 7)
    ///
    /// The pass order is contract-critical: **pump first**, then gather every
    /// [`FrameInputs`] signal, then [`FrameGate::decide`]. On a [`Skip`] the
    /// rebuild/layout/paint/encode/present passes are all bypassed (only a
    /// `skipped` [`FramePasses`] is recorded); on a [`Run`] the passes proceed
    /// as before, with the layout pass itself finer-gated on the drained
    /// [`ChangeFlags`](frust_core::view::ChangeFlags). Every input either
    /// reads a tree accessor or a handle-side latch cleared here — see the
    /// inline comments at each gather site for the RESEARCH §C mapping.
    ///
    /// The pump → gather → decide ordering and the latch lifecycle are asserted
    /// only by inspection, not a unit test: this whole module is
    /// `#[cfg(target_os = "android")]` (it needs a live GPU surface + JNI handle
    /// to construct an [`AndroidAppHandle`]), so it never runs under the host
    /// `cargo test --workspace`. The decision logic it drives *is* exhaustively
    /// host-tested where it lives — `frust_shell_common::frame_gate`'s
    /// `decide`/warmup/kill-switch tests — so what stays unverified here is only
    /// the wiring, which the `cargo check --target aarch64-linux-android` gate
    /// compile-checks and the manual 7.E device checklist exercises.
    ///
    /// [`Skip`]: frust_shell_common::FrameDecision::Skip
    /// [`Run`]: frust_shell_common::FrameDecision::Run
    pub(crate) fn frame(&mut self, frame_time_nanos: u64) {
        // ---------------------------------------------------------------
        // Per-tick input gathering (contract order, task 17 / task 07):
        // pump FIRST, then gather every FrameInputs signal, THEN decide.
        // ---------------------------------------------------------------

        // Pump the reactive runtime's local task queue BEFORE anything else: a
        // controller-driven `spawn_local` task must keep draining every
        // Choreographer tick even while the surface is torn down (e.g.
        // mid-rotation) or not yet created, not just once it's ready — otherwise
        // local tasks stall through surface churn. Pumping first is also task
        // 07's documented ordering contract: a signal a just-drained local task
        // writes must be observed by *this* frame's dirty check below.
        crate::jni_glue::pump_reactive_runtime();

        // Poll the app-facing theme override slot (task 6c-04) once per
        // frame, before the surface-ready gate — theme delivery needs no
        // renderer, so this stays in sync even while the surface is torn down
        // (mirroring the reactive-runtime pump just above). A poll that changes
        // the theme is a frame-gate input (`theme_or_appearance_changed`).
        let mut theme_or_appearance_changed = false;
        match self.theme_override.poll() {
            Some(Some(theme)) => {
                self.theme = theme;
                self.theme_override_active = true;
                self.push_theme();
                theme_or_appearance_changed = true;
            }
            Some(None) => {
                self.theme = Theme::m3_baseline();
                self.theme.brightness = self.platform_brightness;
                self.theme_override_active = false;
                self.push_theme();
                theme_or_appearance_changed = true;
            }
            None => {}
        }

        // Apply accessibility actions queued by assistive tech since the last
        // frame (phase 6d D3), before the surface-ready gate and before the
        // rebuild below so an action's state change is reflected this frame.
        // Cheap (a no-op) whenever nothing is queued, which is the common case.
        // Whether anything was applied is a frame-gate input.
        let a11y_action_performed = self.apply_pending_accessibility_actions();

        if !self.executor.has_surface() {
            // Surface not ready (Kotlin keeps posting frames across surface
            // loss): nothing to render or gate. Inline reads the renderer's phase;
            // the split reads its UI-side `surface_active` mirror (the render
            // thread owns the real phase — a scene handed off while the render
            // thread is still creating the surface is dropped render-side). The
            // reactive pump + theme poll above already ran so state stays live
            // through surface churn; the event/surface latches are intentionally
            // *not* cleared here so the first ready frame still sees them. No
            // FrameStats row is recorded for a not-ready tick (it never was
            // pre-gate either).
            return;
        }

        // Reactive signals-dirty (task 07), drained only past the surface-ready
        // gate — mirroring the iOS shell — so a signal written during a
        // not-ready window is never consumed by a tick that can't render; it is
        // observed by the first ready frame instead. The pump-first ordering
        // contract still holds (the pump above runs before this drain). On a
        // frame the gate goes on to skip the drain is still correct: a skip
        // means "nothing changed", so there is no dirty edge to preserve.
        let signals_dirty = ReactiveRuntime::get()
            .map(|rt| rt.take_signals_dirty())
            .unwrap_or(false);

        // Gather the remaining inputs from the tree's existing accessors and the
        // handle-side latches, then let the gate decide. `mem::take` clears each
        // latch as it is read, so a skipped frame does not leave a stale signal
        // for the next tick.
        //
        // `pointer_capture_active`/`focus_or_ime_active` read the dedicated
        // `AppTree` accessors (`is_pointer_captured`/`is_focus_active`), the
        // same sources the iOS shell's gate uses — the two frame() bodies must
        // stay input-for-input comparable. `ime_state().is_some()` is OR'd in
        // as belt-and-braces: a published IME surface must keep frames running
        // even if the focus path and the published surface ever disagree for a
        // frame (they converge one event pass later by contract).
        let inputs = FrameInputs {
            signals_dirty,
            // Pending buffered pointer samples (a sample too new for this tick's
            // instant) must keep frames running until drained — the resampler's
            // pending signal ORs into the events input (plan phase 10.C.1's
            // "never starves the gate" contract; default-to-run rule).
            events_since_last_frame: std::mem::take(&mut self.events_since_last_frame)
                || self.resampler.has_pending(),
            pointer_capture_active: self.app.is_pointer_captured(),
            focus_or_ime_active: self.app.is_focus_active() || self.app.ime_state().is_some(),
            last_needs_frame: self.last_needs_frame,
            change_flags_pending: self.app.has_pending_change_flags(),
            // The `appearance_dirty` latch (set by `set_appearance`) is taken
            // only past the surface-ready gate — like `signals_dirty` above —
            // so an appearance flip during a not-ready window is observed by
            // the first ready frame instead of being discarded. (`push_theme`'s
            // LAYOUT|PAINT change flags carry correctness either way; the
            // explicit latch is belt-and-suspenders, mirrored on iOS.)
            theme_or_appearance_changed: theme_or_appearance_changed
                || std::mem::take(&mut self.appearance_dirty),
            surface_changed_or_resized: std::mem::take(&mut self.surface_dirty),
            a11y_action_performed,
            // The gate's own warmup counter (seeded by `note_resumed`) drives
            // the resume-warmup Run; leaving this `false` and relying on the
            // counter avoids double-counting (both force a Run identically).
            resumed_recently: false,
        };

        // The surface (re)creation / resize that set `surface_dirty` also forces
        // the layout pass this frame (new dimensions must take effect); ditto the
        // very first frame, before any layout has established geometry.
        let force_layout = inputs.surface_changed_or_resized || !self.first_layout_done;

        // Perf instrumentation (task 08, spec §14 phase 7.A): the process-wide
        // switch is one cached bool read (`perf::enabled`'s `OnceLock`), not a
        // clock read — every `Instant::now()` below is gated behind it via
        // `bool::then`, so a disabled build/run never reads a timer on this
        // hot path (guard first, per this module's perf convention).
        let perf_on = perf::enabled();

        // Deadline-aware pacing (plan phase 10.C.2): estimate this frame's
        // target budget from the tick-to-tick delta, updating the stored tick
        // every frame (skip or run) so the estimate always reflects one refresh
        // interval rather than a gap across skipped ticks.
        let frame_interval = resample::frame_interval_nanos(
            self.last_frame_time_nanos.replace(frame_time_nanos),
            frame_time_nanos,
        );

        if self.frame_gate.decide(inputs).is_skip() {
            // Skip path (task 17): nothing changed — return before rebuild, so
            // CPU/GPU stay near idle. Inline records a `skipped` FramePasses
            // (all-zero pass durations) so the skip counter accumulates in the
            // perf log line. In the render-thread split a Skip sends **nothing**
            // across the channel (task 09 contract — the render thread is the
            // single emitter and never sees skipped frames), so `record_skip` is a
            // no-op there. Either way only frame *production* stops; the
            // Choreographer keeps re-posting callbacks, so the loop cadence is
            // unchanged.
            self.executor.record_skip();
            return;
        }

        // ---------------------------------------------------------------
        // Run path: rebuild -> (layout iff needed) -> paint -> encode/present,
        // timed as before (task 08's instrumentation preserved).
        // ---------------------------------------------------------------

        // Pointer resampling (plan phase 10.C.1): drain buffered samples up to
        // this frame's sample instant and feed the interpolated events into the
        // tree BEFORE the rebuild, so the rebuild reflects this frame's
        // resampled input. Uses the same `resample_clock` domain the raw samples
        // were stamped in. A no-op when the resampler is disabled (touches were
        // delivered directly in `dispatch_touch`). `PointerEvent` is `Copy`, so
        // indexing the scratch buffer avoids holding its borrow across the
        // `self.app.event` call.
        if self.resampler.is_enabled() {
            let now_nanos = self.resample_clock.elapsed().as_nanos() as u64;
            self.pointer_scratch.clear();
            self.resampler
                .resample(now_nanos, &mut self.pointer_scratch);
            let n = self.pointer_scratch.len();
            for i in 0..n {
                let event = InputEvent::Pointer(self.pointer_scratch[i]);
                let _ = self.app.event(&event);
            }
        }

        // Rebuild under the root `Owner` AND inside the persistent
        // [`TrackedScope`] so every signal read this frame subscribes the scope:
        // a later write to any of them trips `signals_dirty` (drained above into
        // `FrameInputs::signals_dirty`), so the frame gate runs the frame that
        // paints the change. Without the `scope.track` wrap a completed async
        // load's write would notify no subscriber and the gate would skip until a
        // touch forced a `Run` (device-parity task 09's device-only "stuck on
        // loading" stall). Mirrors the desktop shell
        // (`app_handler.rs` `scope.track` site) and `create_handle`'s initial
        // construction; fields are borrowed disjointly so the tracking closure
        // captures only what the rebuild needs, not all of `self`. Degrade
        // gracefully to an unwrapped rebuild if the runtime is somehow absent —
        // the frame path must never panic across the JNI boundary.
        let rebuild_start = perf_on.then(Instant::now);
        match ReactiveRuntime::get() {
            Some(rt) => {
                let scope = &self.scope;
                let app = &mut self.app;
                rt.with_owner(|| scope.track(|| app.rebuild()));
            }
            None => self.app.rebuild(),
        }
        let rebuild_time = rebuild_start.map_or(Duration::ZERO, |t| t.elapsed());

        // Layout-skip seam (task 16 / frame_gate module docs): drain the change
        // flags the rebuild (or a prior `set_theme`) accumulated, and run layout
        // only if they need it — or the first frame / a surface resize forces it.
        // The `set_theme => LAYOUT|PAINT` contract keeps `Text`'s layout-baked
        // glyph color correct across a bare theme swap (it marks LAYOUT pending,
        // so a theme change always relayouts even with no view change). Paint
        // still always runs below, replaying the last-baked geometry on a
        // layout-skipped frame.
        let needs_layout = self.app.take_change_flags().needs_layout();
        // Sanitize once per frame; layout and the paint transform below MUST
        // consume this identical value (an untrusted JNI `jfloat` density
        // must never let the two passes disagree — see `sanitize_scale`).
        let scale = sanitize_scale(self.scale);
        let layout_start = perf_on.then(Instant::now);
        if needs_layout || force_layout {
            let (lw, lh) = logical_size(self.physical.0, self.physical.1, scale);
            let logical = Size::new(lw, lh);
            let text_ctx: &mut dyn Any = &mut self.text_ctx;
            self.app.layout(logical, text_ctx);
            self.first_layout_done = true;
        }
        let layout_time = layout_start.map_or(Duration::ZERO, |t| t.elapsed());

        // Publish the accessibility tree post-layout (phase 6d D3), so node
        // bounds are valid. A cheap no-op unless the tree changed AND a screen
        // reader is active (double-gated inside).
        self.publish_semantics();

        self.scene.reset();
        let paint_start = perf_on.then(Instant::now);
        {
            let mut builder = SceneBuilder::new(&mut self.scene);
            // HiDPI (spec task 08): lay out in logical pixels, then scale the
            // whole scene by the device pixel ratio for sharp glyphs.
            builder.push_transform(Affine::scale(scale));
            // Shell-owned frame clock (spec §8: time enters from the shell, never
            // `Instant::now()` inside `frust-core`) — Choreographer's
            // `frameTimeNanos`, forwarded from Kotlin via `nativeOnFrame`.
            let frame_time = FrameTime::from_nanos(frame_time_nanos);
            // The paint pass returns a `needs_frame` continuation signal (spec's
            // v1 animation seam). The Choreographer keeps posting frames, but
            // the frame gate (task 17) now decides whether each is *produced* —
            // so this flag is no longer irrelevant: latch it into
            // `last_needs_frame` so an in-flight animation/transition forces the
            // next frame to run (and stops forcing once it settles).
            let outcome = self.app.paint(&mut builder, frame_time);
            self.last_needs_frame = outcome.needs_frame;
            builder.pop_transform();
        }
        let paint_time = paint_start.map_or(Duration::ZERO, |t| t.elapsed());

        // Hand the finished frame to the render-path executor (plan phase 11.B).
        // The inline fallback runs the encode→acquire→submit tail synchronously
        // here (via the shared [`render_scene`]) and returns its encode span; the
        // split moves the painted scene out (replacing `self.scene` with a fresh
        // empty one) into a [`SceneFrame`] and hands it across the channel for the
        // render thread to encode/present, returning `Duration::ZERO` (encode is
        // off-thread). Either way [`render_scene`] is the single place the folded
        // [`FramePasses`] is recorded, the first-encode/first-frame startup
        // milestones are stamped, and SurfaceLost/Redraw are handled — the exact
        // pre-split tail, only relocated. The clear color (the live theme's
        // surface color, not white) rides *with* the scene so a mid-frame theme
        // flip clears correctly (6e Finding 6).
        let ui = UiSpans {
            rebuild: rebuild_time,
            layout: layout_time,
            paint: paint_time,
            skipped: false,
        };
        let base_color = self.theme.scheme().surface;
        let size = SurfaceSize {
            width: self.physical.0,
            height: self.physical.1,
            scale: self.scale as f64,
        };
        let frame_time = FrameTime::from_nanos(frame_time_nanos);
        let encode_time =
            self.executor
                .submit_frame(&mut self.scene, base_color, ui, frame_time, size, perf_on);

        // Deadline-aware pacing overrun (plan phase 10.C.2): this frame's *work*
        // (everything but the vsync `present` wait, which is expected to block)
        // overrunning the tick-to-tick budget is counted and logged. Gated
        // behind `perf_on` so a non-perf build reads no clocks and logs nothing;
        // instrumentation only — no work is dropped on the strength of this. In
        // the render-thread split `encode_time` is zero (encode is off-thread), so
        // `work` reduces to the UI thread's real budget — rebuild+layout+paint —
        // which is exactly what the UI thread is now responsible for hitting.
        if perf_on {
            let work = rebuild_time + layout_time + paint_time + encode_time;
            if resample::deadline_overrun(work, frame_interval) {
                self.deadline_overruns += 1;
                log::info!(
                    "frust-perf deadline overrun_work_us={} budget_us={} total_overruns={}",
                    work.as_micros(),
                    frame_interval / 1_000,
                    self.deadline_overruns,
                );
            }
        }
    }
}

/// Unit tests for the pure accessibility-tree assembly (phase 6d D3).
///
/// This module is inside the `#[cfg(target_os = "android")]` `app` module, so it
/// only compiles/runs for the Android target — the assembly references
/// `frust_core`/`accesskit` types, both of which are Android-gated
/// dependencies of this crate by deliberate design (see `Cargo.toml`), so it
/// cannot be a host test the way [`crate::ffi_support`]'s pure helpers are. The
/// Android compile gate (`cargo check --target aarch64-linux-android`) is the
/// primary check that this path stays correct.
#[cfg(test)]
mod tests {
    use super::tree_update_from_semantics;
    use frust_core::SemanticsUpdate;
    use frust_core::accesskit::{Node, NodeId, Role, Tree, TreeId};

    #[test]
    fn tree_update_carries_nodes_root_and_focused_node() {
        let nodes = vec![
            (NodeId(1), Node::new(Role::Window)),
            (NodeId(5), Node::new(Role::Button)),
        ];
        let update = SemanticsUpdate {
            nodes: nodes.clone(),
            root: NodeId(1),
            focus: Some(NodeId(5)),
        };
        let tree_update = tree_update_from_semantics(&update);
        assert_eq!(tree_update.nodes, nodes);
        assert_eq!(tree_update.tree, Some(Tree::new(NodeId(1))));
        assert_eq!(tree_update.tree_id, TreeId::ROOT);
        // A focused node is reported directly.
        assert_eq!(tree_update.focus, NodeId(5));
    }

    #[test]
    fn tree_update_focus_falls_back_to_root_when_unfocused() {
        let update = SemanticsUpdate {
            nodes: vec![(NodeId(1), Node::new(Role::Window))],
            root: NodeId(1),
            focus: None,
        };
        let tree_update = tree_update_from_semantics(&update);
        // accesskit requires a non-optional focus target; the root is the
        // conventional fallback (`SemanticsUpdate::focus_id`).
        assert_eq!(tree_update.focus, NodeId(1));
    }
}
