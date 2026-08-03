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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
use frust_shell_common::font_registry::FontRegistryWatcher;
use frust_shell_common::perf::{self, FramePasses, FrameStats, RenderSpans, StartupSpans, UiSpans};
use frust_shell_common::platform_view::FramePairing;
use frust_shell_common::resample::{self, PointerResampler, RawPointerSample};
use frust_shell_common::{
    AppTree, FrameGate, FrameInputs, FrameMeta, FramePacing, PlatformViewState, RenderCommand,
    RenderSender, SceneFrame, SceneReturnReceiver, SurfaceSize, ThemeOverrideWatcher, ViewCommand,
    WindowMetricsPublisher, default_theme, effective_brightness_for_platform_change,
    logical_insets, logical_size, publish_resolved_surface_mode, sanitize_scale,
};
use frust_text::TextContext;
use frust_theme::{Brightness, Theme};
use kurbo::{Affine, Point, Size};
use ndk::native_window::NativeWindow;

use crate::ffi_support::TouchPhase;
use crate::sync_tail::{ScrollSyncTail, TailSignals};

/// Everything a running Android app needs across frames — the state behind the
/// opaque `jlong` handle the JVM passes back into every native call.
///
/// Field order is load-bearing for drop safety: `executor` (which — in the
/// render-thread split — owns the render thread whose `wgpu::Surface` was built
/// from `window`'s raw pointer, and — inline — owns the `SurfaceRenderer`
/// directly) is declared before `window`, so on drop the executor is torn down
/// (the split's `Drop` joins the render thread, dropping its surface) before the
/// [`NativeWindow`] it borrows is released: no surface outlives its
/// window.
pub struct AndroidAppHandle {
    /// The render-path half of the frame loop: either the
    /// render-thread split ([`FrameExecutor::Split`], the default) — where a
    /// dedicated thread owns the [`RenderContext`]/[`SurfaceRenderer`] + surface
    /// and the UI thread only hands it finished scenes — or the pre-split inline
    /// fallback ([`FrameExecutor::Inline`], `FRUST_NO_RENDER_THREAD`) where the
    /// renderer lives on this UI thread. Chosen once at construction.
    executor: FrameExecutor,
    text_ctx: TextContext,
    /// Polls the process-wide app-facing pending-font registry
    /// (`frust::register_app_fonts`) once per frame (see [`Self::frame`],
    /// beside the theme-override poll) — draining any late registration into
    /// `text_ctx`. Also drained once at construction ([`Self::new`], after the
    /// background prewarm join, on this UI thread) before the first rebuild. See
    /// `frust_shell_common::font_registry`'s module docs.
    font_registry: FontRegistryWatcher,
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
    /// touch forces a `Run` — the device-only "channel stuck on loading" stall.
    /// Mirrors the desktop shell's
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
    /// The app's active theme — the seeded base ([`base_theme`]: a design
    /// system's `set_default_theme`, else the built-in fallback) until an
    /// app-forced override replaces it. Mirrors the desktop shell's
    /// appearance ownership: starts [`Brightness::Light`] here and is
    /// flipped by [`Self::set_appearance`] once Kotlin reports the platform's
    /// real dark-mode preference (`nativeSetAppearance`, called right after
    /// `nativeInit` returns a handle — see `platform/android/frust-embedding/src/main/kotlin/dev/frust/
    /// FrustSurfaceView.kt`'s `surfaceCreated`).
    theme: Theme,
    /// The last window insets pushed to the render root, in logical px.
    /// Retained so [`Self::set_insets`] can skip a no-op push
    /// (`WindowInsets` is `PartialEq`) — both the `RenderRoot::set_insets` relayout
    /// and the app-side `provide_context` re-provide only fire on a real change.
    /// Starts zero (no occlusion) until Kotlin's first `nativeOnInsetsChanged`.
    insets: WindowInsets,
    /// Change detector for the app-facing [`WindowMetrics`](frust_core::WindowMetrics)
    /// context: seeded before the first rebuild in [`Self::new`] and re-polled
    /// from every entry point where one of its inputs actually moves
    /// ([`Self::set_window`]/[`Self::split_recreate_surface`]/
    /// [`Self::resize_surface`], and [`Self::set_insets`]) — never from
    /// [`Self::frame`], which only reads values those entry points stored.
    /// See [`Self::push_window_metrics`] for why the guard is load-bearing.
    window_metrics: WindowMetricsPublisher,
    /// Polls the process-wide app-facing theme override slot
    /// (`frust::set_app_theme`/`clear_app_theme`) once per
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
    /// The platform's last-reported reduced-motion preference
    /// (`Settings.Global.ANIMATOR_DURATION_SCALE == 0`, delivered by
    /// `nativeSetReduceMotion` → [`Self::set_reduce_motion`]), tracked
    /// independently of `self.theme.motion.reduce_motion` for the same reason
    /// [`Self::platform_brightness`] is: a theme swap replaces the token and
    /// this latch is what re-raises the floor over the new theme. Starts
    /// `false` (no OS report yet — Kotlin pushes the real value right after
    /// `nativeInit`, alongside `nativeSetAppearance`).
    os_reduce_motion: bool,
    /// The **active theme's own** `motion.reduce_motion` token, captured every
    /// time a whole `Theme` is installed ([`Self::new`]'s seed and the
    /// override-poll arm in [`Self::frame`]). Paired with
    /// [`Self::os_reduce_motion`] through [`effective_reduce_motion`] so the
    /// OS report is a floor over the authored value rather than a replacement
    /// of it — without this, turning the OS setting back off would have to
    /// guess what the theme originally asked for.
    authored_reduce_motion: bool,
    /// The accessibility adapter state, attached lazily by
    /// `nativeInitAccessibility` after `nativeInit` (see
    /// [`Self::attach_accessibility`]). `None` until then — a11y is best-effort
    /// and its wiring never gates the render path. This field is independent of
    /// the `renderer`/`window` drop-order contract above: [`InjectingAdapter`]'s
    /// `Drop` detaches the delegate through its own retained `JavaVM`, touching
    /// neither the surface nor the window.
    a11y: Option<AndroidA11y>,
    /// The skip-frame gate: consulted once per
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
    /// Latch: a platform appearance-ish preference changed since the last
    /// frame — the light/dark mode (`nativeSetAppearance` →
    /// [`Self::set_appearance`]) or the reduced-motion setting
    /// (`nativeSetReduceMotion` → [`Self::set_reduce_motion`]).
    /// Read-and-cleared each
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
    /// Latch: the previous paint's frame request aggregated to
    /// [`frust_core::TickClass::CosmeticLoop`] alone (a pacable decorative loop
    /// with no concurrent transition —
    /// [`frust_core::PaintOutcome::needs_frame_paced_only`]). Read into
    /// [`FrameInputs::last_needs_frame_paced_only`] so
    /// [`FrameGate::decide_paced`] throttles a paced-only frame to the theme's
    /// `cosmetic_loop_rate` instead of running it every Choreographer tick.
    last_needs_frame_paced_only: bool,
    /// Whether the layout pass has run at least once. Until it has, the
    /// layout-skip seam in [`Self::frame`] force-runs layout (a paint before the
    /// first layout would have no valid geometry); after the first layout it is
    /// gated on the drained [`ChangeFlags`](frust_core::view::ChangeFlags)
    /// (or a surface resize).
    first_layout_done: bool,
    /// Pointer-event resampler: buffers raw touch samples and
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
    /// aware pacing estimate: the tick-to-tick delta is this
    /// frame's deadline budget (see [`resample::frame_interval_nanos`]). `None`
    /// before the first frame.
    last_frame_time_nanos: Option<u64>,
    /// Running count of frames whose measured work (rebuild+layout+paint+encode,
    /// excluding the vsync present wait) overran the frame-target deadline.
    /// **Instrumentation only** — accumulated and logged
    /// (`frust-perf deadline`) behind [`perf::enabled`]; it never drops or
    /// reshapes work.
    deadline_overruns: u64,
    /// The differ turning this handle's
    /// published `PlatformViewFrame`s into the idempotent [`ViewCommand`]
    /// backlog `nativePlatformViewCommands` serves to Kotlin's per-frame
    /// poll. Ingested once per RUN frame, right after paint (see
    /// [`Self::frame`]); never touched on a gate `Skip` (a rect can't move
    /// during a skip — the differ's own skip-safety contract). Reset for a
    /// surface recreate via [`Self::set_window`]/[`Self::split_recreate_surface`]
    /// (`reset_for_surface_recreate`); every currently-visible slot is
    /// force-hidden on backgrounding via [`Self::suspend_platform_views`]
    /// (`nativeOnPause`).
    platform_view_state: PlatformViewState,
    /// The release gate's frame pairing: which frust frame
    /// produced each of the differ's command batches, so
    /// [`Self::platform_view_commands`] hands Kotlin only the batches whose
    /// geometry is already **on screen**. Recorded right after each RUN frame's
    /// ingest (the frame about to be submitted is the one that painted it),
    /// drained on acknowledgement, and cleared wholesale when the frames it
    /// refers to stop being meaningful (backgrounding, surface recreation) —
    /// see [`FramePairing`]'s Lifecycle note.
    ///
    /// Always on: a hosted view otherwise runs 3–5 frames ahead of the frust
    /// content it is pinned to while scrolling, and the gate measured strictly
    /// better than the ungated path on both test devices with zero
    /// overshoot on either — so there is no dial.
    platform_view_due: FramePairing,
    /// The shape-aware tail stacked on top of that gate: a
    /// regime-gated, frame-timeline-derived hold that closes the compositor-queue
    /// residual the gate cannot see, and applies **only** while the surface is
    /// acquire-bound (where that residual is constant-shaped). Ticked once per
    /// Choreographer frame at the top of [`Self::frame`] — before every early
    /// return, since the hold ages in display frames — consulted in
    /// [`Self::platform_view_commands`], and cleared beside `platform_view_due`
    /// on the same lifecycle edges. Off (depth 0, bit-for-bit gate-only) below
    /// API 33, with no platform view hosted, or on a submit-bound device: see
    /// [`crate::sync_tail`].
    sync_tail: ScrollSyncTail,
    /// The latest Choreographer frame-timeline delta Kotlin pushed in
    /// (`nativeSetFrameTimeline`, API 33+): `expectedPresentationTimeNanos −
    /// frameTimeNanos`, the platform's own answer to when a window frame
    /// committed now reaches the screen, and the tail's depth input. `0` means
    /// "no sample" — the only signal in this shell the JVM side must read for
    /// us, because `Choreographer.postVsyncCallback` has no NDK equivalent in
    /// this crate's dependency set.
    frame_timeline_delta_nanos: u64,
    /// Whether this handle's GPU surface **actually came up** translucent
    /// (alpha-channel, Mode B) — the RESOLVED capability, not the
    /// [`SurfaceModeWatcher`] request latch (review finding M1).
    ///
    /// The latch says what the app *asked* for; `frust-render` resolves that
    /// against the platform's advertised alpha modes and can silently fall
    /// back to an opaque swapchain
    /// ([`SurfaceRenderer::surface_resolved_translucent`] is the truth). Keying
    /// the base-color swap and `RenderRoot::set_surface_translucent` off the
    /// request instead would clear to `TRANSPARENT` and let every
    /// `platform_view` slot `DestOut`-punch its rect on an opaque surface —
    /// black rectangles. So the invariant "fixed before the surface exists,
    /// never changes" is **false**: a (re)install can downgrade this.
    ///
    /// An `Arc<AtomicBool>` (the `fatal`/`presented` cross-thread pattern)
    /// because in the default render-thread split the surface is created on
    /// the RENDER thread while this (UI) thread owns the `RenderRoot` and the
    /// per-frame base color. **Seeded from the request at construction** so the
    /// overwhelmingly common (capable) case renders Mode B correctly from frame
    /// 1 with no flicker; only a real render-thread resolution may downgrade
    /// it, within one frame of the install (see [`Self::sync_translucent_resolved`]).
    ///
    /// [`SurfaceRenderer::surface_resolved_translucent`]: frust_render::SurfaceRenderer::surface_resolved_translucent
    translucent_resolved: Arc<AtomicBool>,
}

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

/// What the render side publishes about the frames it renders, read by the UI
/// thread — one instance shared by both sides through an `Arc`, the same
/// no-channel/no-protocol pattern as the `fatal` flag.
///
/// Three values, because they answer three different questions and none can
/// substitute for another:
///
/// - `count` — **how many** frames reached the screen, pushed into
///   `AppTree::set_presented_frames` so an FPS-measuring widget reports the
///   presented rate rather than the Choreographer's paint cadence.
/// - `frame_id` — **which** frame is on screen, the platform-view release
///   gate's input ([`AndroidAppHandle::platform_view_commands`]). The count
///   cannot answer this: the UI→render channel is depth-1 latest-wins, so
///   scenes the UI thread submitted are routinely overtaken and never rendered,
///   and the two clocks drift apart by exactly the dropped frames.
/// - `acquire_ewma_us` — **how deep the swapchain queue is**, as the wait the
///   render tail spends inside `acquire` (a quarter-weight EWMA in µs). This is
///   the regime discriminator the shape-aware scroll-sync tail switches on
///   ([`crate::sync_tail`]): acquire-bound means every frame is
///   equally late behind the same queue — a constant-shaped residual a hold can
///   close — while submit-bound means there is no queue to correct for. An
///   `Arc` field beside the two above, deliberately scoped to this instance
///   rather than a process-global.
#[derive(Debug, Default)]
pub(crate) struct RenderSignals {
    count: AtomicU64,
    frame_id: AtomicU64,
    acquire_ewma_us: AtomicU64,
}

impl RenderSignals {
    /// Record one presented frame. `fetch_max` on the id rather than `store`:
    /// ids only ever move forward, and a late write must never walk the release
    /// gate backwards.
    fn record_present(&self, frame_id: u64) {
        self.count.fetch_add(1, Ordering::Relaxed);
        self.frame_id.fetch_max(frame_id, Ordering::Relaxed);
    }

    /// Fold one frame's acquire wait into the published EWMA. Called only for
    /// frames that actually acquired — a skipped frame's ~0 would drag the
    /// average toward zero exactly when the queue is idle-but-deep, which is the
    /// regime the tail exists to detect.
    fn record_acquire_wait(&self, micros: u64) {
        let previous = self.acquire_ewma_us.load(Ordering::Relaxed);
        // Quarter-weight, seeded by the first sample rather than ramping up
        // from zero (the regime must be readable within a few frames of a
        // scroll starting, not a few dozen).
        let next = if previous == 0 {
            micros
        } else {
            (3 * previous + micros) / 4
        };
        self.acquire_ewma_us.store(next, Ordering::Relaxed);
    }

    fn count(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }

    fn frame_id(&self) -> u64 {
        self.frame_id.load(Ordering::Relaxed)
    }

    fn acquire_ewma_us(&self) -> u64 {
        self.acquire_ewma_us.load(Ordering::Relaxed)
    }
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
    /// Fatal-signal flag: the render thread stores `true` here
    /// if its **first** surface install fails — unrecoverable (an incapable
    /// GPU/driver can't change mid-process). The UI thread reads it via
    /// [`AndroidAppHandle::render_fatal`] each `nativeOnFrame` and returns `false`
    /// to Kotlin so the Choreographer loop stops rather than driving doomed
    /// frames against a permanent black screen. One clone here, one in the render
    /// thread ([`crate::jni_glue::render_loop`]).
    fatal: Arc<AtomicBool>,
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

    /// Reclaim a reusable, empty `Scene` for the next frame (review finding
    /// F5): prefer a scene already reclaimed from a stale [`Self::submit_frame`]
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
            // reclaim its buffer instead of letting it drop (review finding F5).
            if let Some(stale_frame) = stale {
                let mut reclaimed = stale_frame.scene.scene;
                reclaimed.reset();
                self.spare_scene = Some(reclaimed);
            }
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

    /// The running count of frames the render side has actually presented
    /// (`FrameOutcome::Rendered`). The UI thread loads this once per frame and
    /// pushes it into `AppTree::set_presented_frames` before paint, so a widget
    /// measuring FPS reports the presented rate — under the split, that is far
    /// below the Choreographer's paint cadence. Both variants share
    /// the counter with their render side via an `Arc<AtomicU64>`.
    fn presented_frames(&self) -> u64 {
        match self {
            FrameExecutor::Inline(inline) => inline.signals.count(),
            FrameExecutor::Split(split) => split.signals.count(),
        }
    }

    /// The id of the last frame the render side actually PRESENTED — the
    /// platform-view release gate's "is the frame that produced this geometry
    /// on screen yet?" input (see [`RenderSignals`]).
    fn presented_frame_id(&self) -> u64 {
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
    fn acquire_wait_us(&self) -> u64 {
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
    fn submitted_frame_id(&self) -> u64 {
        match self {
            FrameExecutor::Inline(inline) => inline.frame_id,
            FrameExecutor::Split(split) => split.frame_id,
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
    /// `FrameStats`; the split sends **nothing** on a skip (the render thread is
    /// the single emitter and never sees skipped frames), so
    /// this is a no-op there.
    fn record_skip(&mut self) {
        if let FrameExecutor::Inline(inline) = self {
            inline.record_skip();
        }
    }

    /// Hand one finished frame to the executor. Inline runs the encode→present
    /// tail synchronously (borrowing `scene`, reused next frame) and returns its
    /// encode span; the split moves the scene out into a [`SceneFrame`] and
    /// sends it across the channel, replacing it with a scene reclaimed off the
    /// render thread's give-back channel (review finding F5) — `reset()`, so its
    /// buffer is reused rather than reallocated — falling back to `Scene::new()`
    /// only when none is available yet, and returning `Duration::ZERO` (encode
    /// is off-thread, so it does not count against the UI thread's deadline).
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
    signals: &RenderSignals,
    // The id of the frame this scene came from (`FrameMeta::frame_id` in the
    // split, the inline executor's own counter otherwise) — published on an
    // actual present so a platform-view geometry batch can be paired back to
    // the frame that produced it (the release gate; see `RenderSignals`).
    frame_id: u64,
) -> Duration {
    // Encode span (GPU/CPU encode, no swapchain touch).
    let encode_start = perf_on.then(Instant::now);
    let encode_outcome = renderer.encode(render_cx, scene, base_color);
    let encode_time = encode_start.map_or(Duration::ZERO, |t| t.elapsed());

    // First-frame decomposition: stamp the first encode-complete
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
    //
    // The one clock read on this path that is NOT gated behind `perf_on` (the
    // module's perf convention — see `docs/CODE_STANDARDS.md`'s Instrumentation
    // conventions): the acquire wait is not instrumentation here, it is the
    // shape-aware scroll-sync tail's live regime input (`RenderSignals::
    // acquire_ewma_us`), which must be readable in a plain
    // profile/release build with tracing off. One `Instant` pair per rendered
    // frame on the render thread; the perf span itself still resolves to
    // `Duration::ZERO` when tracing is off, so nothing else changes.
    let encoded = matches!(encode_outcome, Ok(EncodeOutcome::Encoded));
    let acquire_start = Instant::now();
    let acquire_result = match encode_outcome {
        Ok(EncodeOutcome::Encoded) => renderer.acquire(render_cx),
        Ok(EncodeOutcome::Skipped) => Ok(AcquireOutcome::Skipped),
        Err(err) => Err(err),
    };
    let acquire_elapsed = acquire_start.elapsed();
    if encoded {
        signals.record_acquire_wait(acquire_elapsed.as_micros() as u64);
    }
    let acquire_time = if perf_on {
        acquire_elapsed
    } else {
        Duration::ZERO
    };

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
            // A presented frame: bump the shared counter the UI thread reads
            // before paint and publish WHICH frame is now on screen
            // for the platform-view release gate. `Skipped` presents nothing,
            // so it does neither.
            signals.record_present(frame_id);
            // First successful present: close out the cold-start recorder once.
            if let Some(mut spans) = startup_spans.take() {
                spans.record(perf::SPAN_FIRST_FRAME_PRESENTED);
                spans.emit_log();
            }
        }
        Ok(FrameOutcome::Skipped) => {}
        Err(err) => log::error!("frust-shell-android: render error: {err:#}"),
    }

    // One folded frame record through the single emitter.
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

/// Per-handle accessibility state: the injecting accesskit Android
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

/// The base [`Theme`] this shell seeds itself from: the design-system-supplied
/// default (`frust_shell_common::set_default_theme`, read back through
/// [`default_theme`]) when a plugin seeded one, else the shell's own built-in
/// fallback.
///
/// A seeded default supplies only the *starting point*: unlike an app-forced
/// override (`set_app_theme`) it does not pin brightness — every call site
/// still derives `brightness` from the platform's own preference
/// (`nativeSetAppearance` → [`AndroidAppHandle::platform_brightness`]) against
/// this base, so a design-system default keeps following system dark mode.
///
/// The fallback is deliberately the design-language-free
/// [`Theme::neutral`] — system fonts, no bundled font bytes: a shell names no
/// design system of its own, so an app that installs none gets the neutral
/// floor rather than someone's brand. A design system supplies both halves
/// itself (its base theme through `set_default_theme`, its font bytes through
/// `frust_shell_common::font_registry::register_app_fonts`).
///
/// Takes the slot's value as an argument rather than reading the process-global
/// itself, so the fallback ladder is unit-testable without touching a
/// process-wide slot that has no reset; every call site passes
/// [`default_theme()`](default_theme).
fn base_theme(seeded: Option<Theme>) -> Theme {
    seeded.unwrap_or_else(Theme::neutral)
}

/// The theme a cleared app-theme override reverts to: the seeded base
/// ([`base_theme`] — a design system's `set_default_theme`, else the built-in
/// fallback) at the platform's *current* brightness
/// ([`AndroidAppHandle::platform_brightness`]), never the cleared override's own
/// pinned one.
///
/// Extracted so the `clear_app_theme` arm in [`AndroidAppHandle::frame`] and its
/// unit tests run one implementation: a test that recomputed this in its own
/// body would stay green if the arm regressed to an unconditional
/// `Theme::neutral()` — the exact regression this ladder exists to
/// prevent. `crates/frust/tests/theme_ladder_conformance.rs` pins this body
/// identical to the desktop shell's twin, whose unit tests DO run in a host
/// `cargo test --workspace` (this module's cannot — see its own docs).
fn reverted_theme(seeded: Option<Theme>, platform: Brightness) -> Theme {
    let mut theme = base_theme(seeded);
    theme.brightness = platform;
    theme
}

/// The active-theme decision for one [`ThemeOverrideWatcher::poll`] result —
/// the precedence ladder's top two rungs as one pure function, shared by the
/// per-frame poll arm in [`AndroidAppHandle::frame`] and its unit tests.
///
/// Returns `None` when the poll reported no change (the shell leaves its theme
/// alone), else the new active theme paired with whether an app-forced override
/// is now pinning it ([`AndroidAppHandle::theme_override_active`]).
///
/// `seeded`/`platform` are suppliers rather than values because only the
/// cleared-override arm needs them: reading the process-global default slot
/// ([`default_theme`] — a `Mutex` lock plus a whole-`Theme` clone) would
/// otherwise become per-frame cost on every Choreographer tick, for a poll that
/// reports "nothing changed" on all but a handful of frames.
fn theme_after_override_poll(
    polled: Option<Option<Theme>>,
    seeded: impl FnOnce() -> Option<Theme>,
    platform: impl FnOnce() -> Brightness,
) -> Option<(Theme, bool)> {
    match polled {
        // `set_app_theme`: the forced theme wins wholesale — neither the seeded
        // default nor the platform's brightness is even consulted (the
        // override-wins rule).
        Some(Some(theme)) => Some((theme, true)),
        // `clear_app_theme`: back to the seeded base at the platform's own
        // current brightness.
        Some(None) => Some((reverted_theme(seeded(), platform()), false)),
        None => None,
    }
}

/// Re-derive `theme`'s brightness from a platform appearance report, honouring
/// the override-wins rule ([`effective_brightness_for_platform_change`]): an
/// app-forced override pins brightness, a design-system-seeded default does not
/// — `theme` still IS that base, so flipping it in place re-derives light/dark
/// against the design system's own tokens.
///
/// Extracted for the same reason as [`reverted_theme`]: the
/// `nativeSetAppearance` arm ([`AndroidAppHandle::set_appearance`]) and the
/// seed-ladder tests share one implementation.
fn follow_platform_brightness(theme: &mut Theme, override_active: bool, platform: Brightness) {
    theme.brightness =
        effective_brightness_for_platform_change(override_active, theme.brightness, platform);
}

/// Whether `theme` — the APP's currently active theme, already resolved
/// through the override-wins ladder above — is dark, the pure decision
/// [`AndroidAppHandle::is_dark_theme`] (`nativeAppIsDark`) exposes to Kotlin.
///
/// Extracted as a free function for the same reason as
/// [`follow_platform_brightness`]/[`theme_after_override_poll`]: constructing
/// a real [`AndroidAppHandle`] in a unit test needs a live renderer/surface,
/// which this crate's `#[cfg(target_os = "android")]` gate keeps off the host
/// entirely — a plain `&Theme -> bool` needs neither and is where this
/// module's other ladder tests below assert the actual bug this seam fixes:
/// the result tracks `theme.brightness` (what [`follow_platform_brightness`]/
/// [`theme_after_override_poll`] resolved the APP to), never a raw device
/// `Configuration.uiMode` read.
fn app_is_dark(theme: &Theme) -> bool {
    theme.brightness == Brightness::Dark
}

/// The reduced-motion **floor** rule: the OS's accessibility preference
/// (`Settings.Global.ANIMATOR_DURATION_SCALE == 0`, reported through
/// `nativeSetReduceMotion`) is OR'd over the active theme's own authored
/// `motion.reduce_motion` token — never assigned over it.
///
/// Why a floor rather than the brightness rule's override-wins ladder
/// ([`follow_platform_brightness`]): reduced motion is an accessibility
/// *guarantee*, not a style preference, so a live OS toggle must reach a theme
/// an app forced with `set_app_theme` (a catalog's design-language switcher,
/// say) instead of being pinned out of it. Symmetrically, an app that
/// deliberately authored `reduce_motion: true` keeps it while the OS setting is
/// off — neither side can un-reduce what the other asked for, which is the one
/// direction it is never safe to get wrong.
///
/// Deliberately NOT one of the ladder helpers pinned byte-identical across all
/// three shells by `crates/frust/tests/theme_ladder_conformance.rs`: desktop
/// has no reduced-motion source at all (see `MotionScheme::reduce_motion`), so
/// only the two mobile shells carry this.
fn effective_reduce_motion(authored: bool, os: bool) -> bool {
    authored || os
}

/// Run one **event pass** under the reactive runtime's root
/// [`Owner`](frust_reactive::Owner), so `use_context` resolves from inside a
/// press/key/IME handler exactly as it does from `Component::build`.
///
/// Every path that reaches [`AppTree::event`](frust_shell_common::AppTree::event)
/// — touch dispatch (raw and frame-resampled), `ime_apply`, `ime_action`, and a
/// queued accessibility action — routes through this. Without it
/// `Owner::current()` is `None` for the whole pass (`Owner::with` restores the
/// previous owner when the rebuild wrap returns), so a handler's
/// `use_context::<Theme>()` silently resolves to `None`.
///
/// Three deliberate properties, mirrored in the iOS shell and pinned by the
/// desktop shell's `event_pass_*` tests (the mobile `app` modules are
/// target-gated and never host-compiled, so that is where this shape is
/// testable):
///
/// * **The root owner, not a fresh child scope.** A child owner would have to be
///   created and disposed per input event — including per resampled `Move` —
///   and a handler's `provide_context` would evaporate on dispose. Sharing costs
///   one thread-local swap per pass and no allocation.
/// * **No [`TrackedScope`].** `TrackedScope::track` clears the scope's recorded
///   sources and dirty flag on entry, so tracking an event pass would unsubscribe
///   the frame loop from every signal the last rebuild read *and* swallow a
///   pending wake. A handler that writes a signal still wakes the shell through
///   the rebuild scope's own subscription, unchanged.
/// * **Never panics.** Like the rebuild wrap, it degrades to running the pass
///   unwrapped if the runtime is somehow absent — this path is reached from JNI,
///   where an unwind is undefined behavior.
fn under_root_owner<R>(pass: impl FnOnce() -> R) -> R {
    match ReactiveRuntime::get() {
        Some(rt) => rt.with_owner(pass),
        None => pass(),
    }
}

/// Assemble an accesskit [`TreeUpdate`] from a [`SemanticsUpdate`].
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
    /// Seeds the base theme ([`base_theme`]: a design system's
    /// `set_default_theme`, else the built-in [`Theme::neutral`] fallback,
    /// carrying that base's own brightness until Kotlin's follow-up
    /// `nativeSetAppearance`
    /// reports the real preference; `platform_brightness` defaults `Light` and
    /// unconditionally overwrites `theme.brightness` on that call, so a
    /// light-preference device still ends up light) into both delivery
    /// paths (`AppTree::set_theme` for widgets, `provide_context` for app code)
    /// before the first rebuild, mirroring the desktop shell's `apply_theme`.
    /// Must be called under the root reactive `Owner` (see
    /// `crate::jni_glue::create_handle`) so `provide_context` isn't a silent
    /// no-op.
    ///
    /// `executor` is the render-path half [`crate::jni_glue::create_handle`]
    /// already built: the render-thread split
    /// ([`FrameExecutor::Split`], with the render thread already spawned and its
    /// initial `SurfaceCreated` sent) or the inline fallback
    /// ([`FrameExecutor::Inline`], with the surface + early startup spans already
    /// created on this UI thread). The initial `rebuild()` below records
    /// [`perf::SPAN_FIRST_REBUILD_DONE`] via [`FrameExecutor::record_first_rebuild`]
    /// — inline records it here, the split records it render-side on the first
    /// handed-off scene.
    ///
    /// `text_ctx` is the [`TextContext`] `create_handle` already resolved
    /// — the pre-built one from `JNI_OnLoad`'s background
    /// font-preload thread when it finished in time, or a synchronous
    /// fallback otherwise (see `jni_glue::take_preinit_text_context`) — so
    /// this method never itself pays the font-DB load cost.
    pub(crate) fn new(
        executor: FrameExecutor,
        mut text_ctx: TextContext,
        window: NativeWindow,
        physical: (u32, u32),
        scale: f32,
        translucent_resolved: Arc<AtomicBool>,
        mut app: Box<dyn AppTree>,
    ) -> Self {
        // Construction-time font drain: apply any fonts registered via
        // `frust::register_app_fonts` before this handle existed into the joined
        // `TextContext`, on this (the UI) thread after the background prewarm
        // join — NOT inside the spawned prewarm closure. `create_handle` funnels
        // both executor paths (inline + split) through here with the already-
        // joined `text_ctx`, so this one drain covers both. Pre-first-rebuild, so
        // no invalidation is needed; the per-frame poll in `frame` picks up any
        // later registration.
        let mut font_registry = FontRegistryWatcher::new();
        font_registry.drain_into(&mut text_ctx);

        // The seeded base theme (`base_theme`: a design system's
        // `set_default_theme`, else the built-in `Theme::neutral()` fallback),
        // carrying that base's own brightness until Kotlin's follow-up
        // `nativeSetAppearance` overwrites it with the device's real
        // preference. A design system that needs its own font bytes shaped from
        // the first frame registers them through
        // `frust_shell_common::font_registry::register_app_fonts`, which the
        // construction-time drain above applies — the shell itself bundles no
        // fonts.
        let theme = base_theme(default_theme());
        // The base's own reduced-motion token, kept so a later OS report (or a
        // later theme swap) can re-derive the effective value instead of
        // guessing what the theme asked for — see `authored_reduce_motion`.
        let authored_reduce_motion = theme.motion.reduce_motion;

        app.set_theme(Box::new(theme.clone()));
        // Thread the surface's RESOLVED translucency into the render root so
        // the platform-view hole-punch clears each Mode B slot's rect — and
        // only when the surface really came up translucent, not merely
        // requested. At construction the split's surface may still be
        // installing render-side, so this reads the request-seeded flag; the
        // per-frame `sync_translucent_resolved` below re-reads it every frame
        // and downgrades within one frame of a fallback.
        let resolved_translucent =
            crate::ffi_support::read_resolved_translucency(&translucent_resolved);
        app.set_surface_translucent(resolved_translucent);
        // Seed the app-facing RESOLVED slot from that same value,
        // so app/plugin code reading `frust::resolved_surface_mode()` during
        // the very first rebuild below sees a real verdict rather than
        // `Unknown`. Re-published every frame by `sync_translucent_resolved`.
        publish_resolved_surface_mode(resolved_translucent);
        provide_context(theme.clone());
        // Seed the app-facing window-shape context beside the theme, so an
        // `app_logic`/`Component::build` calling `use_context::<WindowMetrics>()`
        // during the very first rebuild below resolves a real value rather than
        // `None`. Insets start at zero here — Kotlin's first
        // `nativeOnInsetsChanged` arrives after `nativeInit` and re-polls this
        // publisher (see `set_insets`). No `ReactiveRuntime::get()` wrap: like
        // the theme seed above, this runs nested inside `create_handle`'s
        // `with_owner`.
        let mut window_metrics = WindowMetricsPublisher::new();
        if let Some(metrics) =
            window_metrics.poll(physical, sanitize_scale(scale), WindowInsets::default())
        {
            provide_context(metrics);
        }
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
            font_registry,
            scene: Scene::new(),
            app,
            physical,
            scale,
            window: Some(window),
            scope: TrackedScope::new(),
            theme,
            insets: WindowInsets::default(),
            window_metrics,
            theme_override: ThemeOverrideWatcher::new(),
            theme_override_active: false,
            platform_brightness: Brightness::Light,
            // No OS reduced-motion report yet (Kotlin pushes one right after
            // `nativeInit`, beside `nativeSetAppearance`), so the seeded base's
            // own token IS the effective value — `effective_reduce_motion`
            // against a `false` OS latch is the identity, which is why the
            // seed above needs no fix-up.
            os_reduce_motion: false,
            authored_reduce_motion,
            a11y: None,
            frame_gate,
            events_since_last_frame: false,
            surface_dirty: false,
            appearance_dirty: false,
            last_needs_frame: false,
            last_needs_frame_paced_only: false,
            first_layout_done: false,
            resampler: PointerResampler::new(),
            resample_clock: Instant::now(),
            pointer_scratch: Vec::new(),
            last_frame_time_nanos: None,
            deadline_overruns: 0,
            platform_view_state: PlatformViewState::new(),
            platform_view_due: FramePairing::new(),
            sync_tail: ScrollSyncTail::new(),
            frame_timeline_delta_nanos: 0,
            translucent_resolved,
        }
    }

    /// Re-read the live surface's RESOLVED translucency and push it into the
    /// render root, returning it for this frame's base-color choice (review
    /// finding M1).
    ///
    /// Two sources, one flag:
    /// - **Inline** (`FRUST_NO_RENDER_THREAD`): the renderer lives on this
    ///   thread, so its `surface_resolved_translucent()` is authoritative and
    ///   is copied into the shared flag here — never stale, even after a failed
    ///   reinstall (the renderer still describes whatever surface is live).
    /// - **Split** (default): the render thread stored the resolution when it
    ///   installed the surface; this is a plain atomic load.
    ///
    /// Cheap enough to call every frame: one enum match plus one atomic load,
    /// and [`AppTree::set_surface_translucent`] is no-op-if-unchanged (it marks
    /// `ChangeFlags::PAINT` only on an actual flip, which is exactly what makes
    /// a downgrade repaint without the punch).
    ///
    /// Also the shell's single publish point for the app-facing RESOLVED slot
    /// (`frust::resolved_surface_mode()`): app code polls that slot
    /// during rebuild, so it has to be current *before* the rebuild this frame
    /// leads into — publishing here rather than at the install sites keeps one
    /// UI-thread beat as the source for both consumers (the render root and the
    /// app), split and inline alike. One uncontended `Mutex` store per frame.
    fn sync_translucent_resolved(&mut self) -> bool {
        if let FrameExecutor::Inline(inline) = &self.executor {
            crate::ffi_support::publish_resolved_translucency(
                &self.translucent_resolved,
                Some(inline.renderer.surface_resolved_translucent()),
            );
        }
        let resolved = crate::ffi_support::read_resolved_translucency(&self.translucent_resolved);
        publish_resolved_surface_mode(resolved);
        self.app.set_surface_translucent(resolved);
        resolved
    }

    /// Attach the accesskit Android adapter to the host `FrustSurfaceView`
    /// (`nativeInitAccessibility`).
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
    /// last frame, routing each to
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
        let app = &mut self.app;
        // An accessibility action is routed through synthesized pointer events,
        // landing in the very same handlers a real tap would — so it needs the
        // ambient `Owner` just as much (see [`under_root_owner`]).
        under_root_owner(|| {
            for (node_id, action) in drained {
                let _ = app.perform_accessibility_action(node_id.0, action);
            }
        });
        performed
    }

    /// Publish the current accessibility tree to the adapter, if it
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
    /// delivery paths (mirrors the desktop shell's `apply_theme`).
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
        // Override-wins rule: while an app-forced theme override
        // is active, this platform-appearance report must not flip brightness.
        // A seeded *default* is deliberately not pinned that way: `self.theme`
        // still IS that base (`base_theme` seeded it and nothing replaced it),
        // so flipping its brightness in place re-derives light/dark from the
        // design system's own theme.
        follow_platform_brightness(&mut self.theme, self.theme_override_active, platform);
        // Frame-gate input: an appearance change must force the next
        // frame to run so the re-themed tree repaints. `push_theme` below also
        // marks LAYOUT|PAINT change flags, so this is belt-and-suspenders with
        // `change_flags_pending` — but it maps the appearance edit onto its own
        // `theme_or_appearance_changed` input directly.
        self.appearance_dirty = true;
        self.push_theme();
    }

    /// `nativeAppIsDark`: whether the APP's currently active theme resolves to
    /// [`Brightness::Dark`] right now — the read half of the appearance seam,
    /// closing the gap [`Self::set_appearance`] alone left open: that call is
    /// Kotlin→Rust only (the device's `uiMode` in, nothing back out), so the
    /// Kotlin system-bar icon contrast (`FrustSurfaceView.
    /// updateSystemBarsAppearance`) had no way to ask what the app actually
    /// ended up rendering — it just re-read the same device signal, which
    /// silently disagrees with `self.theme.brightness` whenever an app-forced
    /// override (`frust::set_app_theme`) is active (the override-wins rule —
    /// see [`follow_platform_brightness`]) or a design system's seeded default
    /// diverges from the platform preference.
    ///
    /// Deliberately returns the narrowest possible payload — one `bool`, not a
    /// serialized `Theme` or a `ColorScheme` snapshot: system-bar icon
    /// contrast only ever needs light-vs-dark, and every other theme field
    /// already has its own delivery path (`RenderRoot::set_theme` /
    /// `provide_context`) that has nothing to do with the JNI boundary.
    ///
    /// Reads `self.theme.brightness` directly rather than
    /// `self.platform_brightness`: the former is the value **actually in
    /// effect** after the override-wins rule resolves (what the app is really
    /// showing), the latter is only the device's last report, which is
    /// exactly the value this seam exists to stop Kotlin from trusting on its
    /// own. Kotlin polls this once per frame ([`FrustSurfaceView.
    /// pollAppBrightness`]) in addition to calling it right after
    /// `nativeSetAppearance` (`surfaceCreated`/`onConfigurationChanged`), so a
    /// runtime `set_app_theme`/`clear_app_theme` call — which has no device
    /// `Configuration` event of its own — still reaches the status bar within
    /// one frame instead of only at the next config change.
    pub(crate) fn is_dark_theme(&self) -> bool {
        app_is_dark(&self.theme)
    }

    /// `nativeSetReduceMotion`: apply the platform's reduced-motion
    /// accessibility preference to the active theme's `MotionScheme` and
    /// re-push it to both delivery paths — the reduced-motion twin of
    /// [`Self::set_appearance`], travelling the identical transport (a JNI
    /// entry → a `Theme` edit → [`Self::push_theme`]) over a different sensor.
    ///
    /// `reduce` is Kotlin's `Settings.Global.ANIMATOR_DURATION_SCALE == 0f`
    /// read (see `FrustSurfaceView.reduceMotionEnabled`); the setting is NOT a
    /// `Configuration` field, so Kotlin observes it with a `ContentObserver`
    /// plus a re-read on every resume rather than through
    /// `onConfigurationChanged`.
    ///
    /// The OS value is a floor over the theme's own authored token, not a
    /// replacement ([`effective_reduce_motion`]) — including while an app-forced
    /// override is active, unlike the brightness path's override-wins rule
    /// (that helper's doc has the reasoning). No explicit redraw is scheduled:
    /// the continuous Choreographer loop already repaints every tick, and
    /// `appearance_dirty` keeps the frame gate from skipping the tick that
    /// carries the change.
    ///
    /// Unchanged-value calls return early, unlike [`Self::set_appearance`]:
    /// Kotlin re-pushes this on every resume (and a `ContentObserver` can fire
    /// more than once per real change), where a config-change-driven appearance
    /// push is rare — without the guard every resume would pay a `push_theme`'s
    /// forced relayout for nothing. Sound because `os_reduce_motion` is the
    /// only writer of the token outside a whole-`Theme` install, and that
    /// install re-applies the floor itself.
    pub(crate) fn set_reduce_motion(&mut self, reduce: bool) {
        if self.os_reduce_motion == reduce {
            return;
        }
        self.os_reduce_motion = reduce;
        self.theme.motion.reduce_motion =
            effective_reduce_motion(self.authored_reduce_motion, reduce);
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
    /// install failure, read by
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
        // dimensions and open the resume-warmup window.
        self.surface_dirty = true;
        self.frame_gate.note_resumed();
        // The new surface may come up at new dimensions/density — republish the
        // window-shape context (self-guarded, so a same-size recreate is silent).
        self.push_window_metrics();
        // The old surface (and everything Kotlin's `FrustSurfaceView` composited
        // behind it) is gone — replay Create+Update for every currently-live
        // platform-view slot so the native side rebuilds its whole
        // sibling-view hierarchy from scratch rather than assuming any prior
        // placement survived. The replay supersedes every held batch, and the
        // frames those batches were paired with belong to the surface that just
        // went away — so the pairing goes with it.
        self.platform_view_state.reset_for_surface_recreate();
        self.platform_view_due.clear();
        // ...and with it the tail's hold: neither stage may outlive the
        // frames it refers to (mirroring the pairing above).
        self.sync_tail.clear();
        // The new surface re-resolves its alpha mode from scratch;
        // the render thread stores the outcome when it installs. Re-push
        // whatever is known now — the next `frame` re-reads it, so a
        // downgrade lands within one frame of the install.
        self.sync_translucent_resolved();
    }

    /// Record the window + physical size backing a freshly (re)created surface.
    ///
    /// Assigning `window` last drops the previous [`NativeWindow`] (releasing it)
    /// — safe here because the previous surface was already torn down inside the
    /// preceding `on_surface_created_from_android_window`.
    ///
    /// `density` is this configuration's `displayMetrics.density`: stored raw
    /// and re-sanitized at every use (layout/paint/insets),
    /// so a config change that alters the device pixel ratio takes effect on the
    /// next frame. Stored raw for the same reason `nativeInit`'s `scale` is —
    /// `sanitize_scale` runs once per frame at the point of use.
    pub(crate) fn set_window(&mut self, window: NativeWindow, physical: (u32, u32), density: f32) {
        self.physical = physical;
        self.scale = density;
        self.window = Some(window);
        // Surface (re)creation: force the next frame to run (and lay out at the
        // new dimensions) and open the gate's resume-warmup window — the first
        // ticks after a surface swap must not be gated away.
        self.surface_dirty = true;
        self.frame_gate.note_resumed();
        // See `split_recreate_surface`'s matching call: a new surface means a
        // fresh native-view hierarchy on the Kotlin side, and the
        // release-gate pairing refers to the old surface's frames.
        //
        // A recreate can also land at new dimensions/density — republish the
        // window-shape context (self-guarded, so a same-size recreate is silent).
        self.push_window_metrics();
        self.platform_view_state.reset_for_surface_recreate();
        self.platform_view_due.clear();
        // ...and with it the tail's hold: neither stage may outlive the
        // frames it refers to (mirroring the pairing above).
        self.sync_tail.clear();
        // Inline recreate: the renderer on this thread already holds the new
        // surface, so this reads its freshly RESOLVED translucency
        // — a recreate that fell back to opaque degrades to Mode A here rather
        // than punching black holes for the rest of the process.
        self.sync_translucent_resolved();
    }

    /// Resize the live surface in place (same window, new dimensions). Safe: no
    /// raw pointers — inline reconfigures the renderer's swapchain directly; the
    /// split sends a [`RenderCommand::SurfaceChanged`] command.
    ///
    /// `density` re-sanitizes and stores the display's device pixel ratio for
    /// this configuration (see [`Self::set_window`]).
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
        // size, and open the warmup window — same rationale as
        // `set_window`.
        self.surface_dirty = true;
        self.frame_gate.note_resumed();
        // New logical size and/or density: republish the window-shape context
        // (self-guarded, so a `surfaceChanged` re-reporting identical
        // dimensions publishes nothing).
        self.push_window_metrics();
    }

    /// `nativeOnInsetsChanged`: convert the platform's physical-px per-edge insets
    /// to logical px with the stored scale and push them onto the render root.
    /// Mirrors [`Self::set_appearance`]'s two-path
    /// delivery shape but for insets: [`AppTree::set_insets`] threads them into
    /// layout/paint (widget path — a `SafeArea`'s `LayoutCtx::window_insets`), and
    /// [`Self::push_insets`] re-`provide_context`s them for app code
    /// (`use_context::<WindowInsets>()` in `Component::build`).
    ///
    /// `physical` is the eight-value pack `logical_insets` expects (`view_padding`
    /// then `view_insets`, each l/t/r/b — see [`logical_insets`]). No-op-guarded on
    /// `PartialEq`: a shell that re-reports unchanged insets neither relayouts nor
    /// re-provides. On a real change, `RenderRoot::set_insets` marks `LAYOUT |
    /// PAINT` pending, which the frame gate already treats as
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
        // The composite window-shape context carries a copy of these insets, so
        // an insets change is also a metrics change (self-guarded).
        self.push_window_metrics();
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

    /// Re-`provide_context` the window's [`WindowMetrics`](frust_core::WindowMetrics)
    /// for app-side `use_context::<WindowMetrics>()` reads — **only when it
    /// actually changed** ([`WindowMetricsPublisher::poll`] returns `None`
    /// otherwise).
    ///
    /// Mirrors [`Self::push_insets`]/[`Self::push_theme`]'s re-provide shape,
    /// with two deliberate differences:
    ///
    /// - **Guarded, never per-frame.** `WindowMetrics` is delivered *alongside*
    ///   the standalone `WindowInsets` context (which keeps working untouched),
    ///   not through it, and it is not pushed into the render root at all — so
    ///   nothing else rate-limits it. `provide_context` is a plain map insert
    ///   that notifies nothing, but an unconditional per-frame re-provide would
    ///   still pay a lock write plus an allocation every frame across the FFI
    ///   boundary for no observable benefit. The publisher's change detection
    ///   is what keeps a static window quiet.
    /// - **Called from the entry points where the inputs move**, not from
    ///   [`Self::frame`]: `nativeOnSurfaceChanged` → [`Self::resize_surface`] /
    ///   [`Self::set_window`] / [`Self::split_recreate_surface`] (size + density)
    ///   and `nativeOnInsetsChanged` → [`Self::set_insets`] (insets). `frame`
    ///   only reads what those already stored.
    ///
    /// Units: the size is converted from the surface's physical px with the same
    /// `sanitize_scale`d density every other consumer uses, and `self.insets` is
    /// already logical (`logical_insets` ran in [`Self::set_insets`]) — so the
    /// published metrics are logical throughout, matching iOS and desktop.
    fn push_window_metrics(&mut self) {
        let scale = sanitize_scale(self.scale);
        let Some(metrics) = self.window_metrics.poll(self.physical, scale, self.insets) else {
            return; // unchanged — no re-provide, no app-wide rebuild
        };
        match ReactiveRuntime::get() {
            Some(rt) => rt.with_owner(|| provide_context(metrics)),
            None => provide_context(metrics),
        }
    }

    /// Tear the surface down (`surfaceDestroyed`): drop the surface **first**,
    /// then release the [`NativeWindow`] it borrowed.
    ///
    /// The ordering is a hard correctness contract in the render-thread split:
    /// the render thread owns the surface built from `self.window`'s raw pointer,
    /// so [`SplitExecutor::destroy_surface_barrier`] sends a barriered
    /// `SurfaceDestroyed` and **blocks until the render thread has acknowledged**
    /// dropping that surface. Only after that ack returns do we set
    /// `self.window = None`, releasing the `ANativeWindow` — so the render thread
    /// can never touch the window after it is released (a known Android
    /// surface-lifecycle-race hazard). The inline path drops its surface
    /// synchronously above, so the same window-after-surface order holds there.
    /// Force every currently-visible platform-view slot to hide
    /// (`nativeOnPause`) — delegates to
    /// [`PlatformViewState::suspend_all`].
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
    /// declares that frame dropped). Kotlin needs no change: it applies exactly
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
    /// [`PlatformViewState::acknowledge`], and drops the matching release-gate
    /// bookkeeping so it tracks the live backlog rather than growing for the
    /// process lifetime.
    pub(crate) fn acknowledge_platform_view_commands(&mut self, generation: u64) {
        self.platform_view_state.acknowledge(generation);
        self.platform_view_due.acknowledge(generation);
    }

    /// This handle's device pixel ratio, sanitized the same way every other
    /// scale-consuming call site does (`dispatch_touch`/`set_insets`/
    /// `frame`'s layout step) — the one value `crate::jni_glue`'s
    /// `nativePlatformViewCommands` needs to convert the differ's logical-px
    /// rects to physical px at the FFI boundary, without exposing the raw
    /// `scale` field (private to this module) across the `jni_glue`/`app`
    /// module boundary.
    pub(crate) fn sanitized_scale(&self) -> f64 {
        sanitize_scale(self.scale)
    }

    pub(crate) fn destroy_surface(&mut self) {
        match &mut self.executor {
            FrameExecutor::Inline(inline) => inline.renderer.on_surface_destroyed(),
            FrameExecutor::Split(split) => split.destroy_surface_barrier(),
        }
        // Barrier complete (split) / surface dropped (inline): releasing the
        // window is now safe.
        self.window = None;
    }

    /// Deliver one touch contact to the tree, converting the incoming
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
        // Frame-gate latch: an event between frames must force the
        // next frame to run so the tree reflects the dispatch. Set even on a
        // no-op dispatch — correctness beats savings, and the gate defaults to
        // "must run" when in doubt.
        self.events_since_last_frame = true;

        // Gesture markers for the scroll-sync onset measurement: the
        // tail's own display-frame counter stamped at the start and
        // end of a gesture, so the frames between the first touch and the hold
        // reaching its depth can be read straight out of a trace instead of
        // eyeballed against logcat wall-clock stamps. Down/Up only (a Move line
        // per frame would drown the trace it serves), and behind the same
        // `perf::enabled()` switch as every other instrumented line here.
        if matches!(core_phase, PointerPhase::Down | PointerPhase::Up) && perf::enabled() {
            log::info!(
                "frust-perf platform-view gesture phase={} frame={}",
                if core_phase == PointerPhase::Down {
                    "down"
                } else {
                    "up"
                },
                self.sync_tail.display_frame(),
            );
        }

        // Pointer resampling: buffer the raw sample (stamped
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
            let app = &mut self.app;
            let _ = under_root_owner(|| app.event(&event));
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
        // Frame-gate latch: an IME edit between frames forces the next
        // frame to run (see `dispatch_touch`).
        self.events_since_last_frame = true;
        let app = &mut self.app;
        let _ = under_root_owner(|| app.ime_apply(state));
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
    /// focus-routed key path a hardware Enter would, so a widget's
    /// submit/newline handling stays in one place.
    pub(crate) fn ime_action(&mut self, _action: i32) {
        let event = InputEvent::Key(KeyEvent {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers::default(),
            repeat: false,
        });
        // Frame-gate latch: a soft-keyboard action forces the next
        // frame to run (see `dispatch_touch`).
        self.events_since_last_frame = true;
        let app = &mut self.app;
        let _ = under_root_owner(|| app.event(&event));
    }

    /// Run one frame: rebuild → layout → paint → render, mirroring the desktop
    /// shell's `RedrawRequested` path but driven by Choreographer.
    ///
    /// `frame_time_nanos` is Kotlin's Choreographer `frameTimeNanos` for this
    /// tick (already clamped non-negative at the JNI boundary — see
    /// [`crate::jni_glue::native_on_frame`]), the shell-owned monotonic clock
    /// threaded into [`FrameTime`] (`frust-core` never reads a clock
    /// itself).
    ///
    /// A no-op when the surface isn't `SurfaceReady` (Kotlin keeps posting frames
    /// across surface loss; this makes those cheap). On `FrameOutcome::SurfaceLost`
    /// the machine has already dropped the surface; recovery waits for the next
    /// `surfaceChanged`/`surfaceCreated` rather than recreating mid-frame.
    ///
    /// # Frame gate
    ///
    /// The pass order is contract-critical: **pump first**, then gather every
    /// [`FrameInputs`] signal, then [`FrameGate::decide`]. On a [`Skip`] the
    /// rebuild/layout/paint/encode/present passes are all bypassed (only a
    /// `skipped` [`FramePasses`] is recorded); on a [`Run`] the passes proceed
    /// as before, with the layout pass itself finer-gated on the drained
    /// [`ChangeFlags`](frust_core::view::ChangeFlags). Every input either
    /// reads a tree accessor or a handle-side latch cleared here — see the
    /// inline comments at each gather site for what each signal means.
    ///
    /// The pump → gather → decide ordering and the latch lifecycle are asserted
    /// only by inspection, not a unit test: this whole module is
    /// `#[cfg(target_os = "android")]` (it needs a live GPU surface + JNI handle
    /// to construct an [`AndroidAppHandle`]), so it never runs under the host
    /// `cargo test --workspace`. The decision logic it drives *is* exhaustively
    /// host-tested where it lives — `frust_shell_common::frame_gate`'s
    /// `decide`/warmup/kill-switch tests — so what stays unverified here is only
    /// the wiring, which the `cargo check --target aarch64-linux-android` gate
    /// compile-checks and a manual device checklist exercises.
    ///
    /// [`Skip`]: frust_shell_common::FrameDecision::Skip
    /// [`Run`]: frust_shell_common::FrameDecision::Run
    pub(crate) fn frame(&mut self, frame_time_nanos: u64) {
        // ---------------------------------------------------------------
        // Per-tick input gathering (contract order):
        // pump FIRST, then gather every FrameInputs signal, THEN decide.
        // ---------------------------------------------------------------

        // Advance the scroll-sync tail ahead of EVERY early
        // return in this function — the frame gate's skip, the surface-ready
        // bail — because a held geometry batch ages in **display** frames, and
        // the Choreographer keeps delivering those while frust produces
        // nothing. That is what makes the settle case structural: a batch left
        // over from the last frame of a fling lands on schedule even though the
        // scroll produced no further frust frame.
        let acquire_wait_us = self.executor.acquire_wait_us();
        let tail_trace = self.sync_tail.tick(TailSignals {
            frame_time_nanos: frame_time_nanos as i64,
            expected_present_delta_nanos: self.frame_timeline_delta_nanos,
            acquire_wait_us,
        });
        // One line per depth-or-regime *change*, plus a rate-limited periodic
        // snapshot even without one (`ScrollSyncTail::tick`'s
        // `DIAGNOSTIC_INTERVAL_DISPLAY_FRAMES`, so the emit-or-not decision
        // stays host-tested rather than living here): a handful of lines per
        // fling, never per frame, behind the same `perf::enabled()` switch as
        // every other instrumented line here. The periodic fallback is what
        // makes a stock `FRUST_TRACE` profile build answer "why didn't the
        // regime latch" on a new device — a change-only line stays silent for
        // the whole session there. `acquire_us` is the render side's
        // acquire-wait EWMA (the regime discriminator), not a raw sample;
        // `expected_present_us`/`period_us` are the depth's numerator/divisor;
        // `depth`/`target` are the ramped-vs-requested hold. `frame=` is the
        // tail's display-frame counter, which the gesture markers in
        // `dispatch_touch` stamp too: subtracting the two is the onset
        // measurement.
        if let Some(trace) = tail_trace
            && perf::enabled()
        {
            log::info!(
                "frust-perf platform-view tail depth={} target={} acquire_bound={} \
                 frame={} expected_present_us={} acquire_us={acquire_wait_us} period_us={}",
                trace.depth,
                trace.target_depth,
                trace.acquire_bound,
                trace.display_frame,
                self.frame_timeline_delta_nanos / 1000,
                (self.sync_tail.period_ms() * 1000.0) as u64,
            );
        }

        // Pump the reactive runtime's local task queue BEFORE anything else: a
        // controller-driven `spawn_local` task must keep draining every
        // Choreographer tick even while the surface is torn down (e.g.
        // mid-rotation) or not yet created, not just once it's ready — otherwise
        // local tasks stall through surface churn. Pumping first also matters
        // for correctness: a signal a just-drained local task
        // writes must be observed by *this* frame's dirty check below.
        crate::jni_glue::pump_reactive_runtime();

        // Poll the app-facing theme override slot once per
        // frame, before the surface-ready gate — theme delivery needs no
        // renderer, so this stays in sync even while the surface is torn down
        // (mirroring the reactive-runtime pump just above). A poll that changes
        // the theme is a frame-gate input (`theme_or_appearance_changed`).
        //
        // Reverting an override lands on the base this shell seeded itself
        // from — the design-system default when one was supplied, else the
        // built-in fallback — with brightness re-derived from the platform's
        // last reported preference rather than inherited from the cleared
        // override; that whole ladder lives in `theme_after_override_poll` so
        // this arm and its unit tests share one implementation. The seeded
        // supplier stays lazy: an unchanged poll never reads the
        // process-global slot.
        let mut theme_or_appearance_changed = false;
        let platform_brightness = self.platform_brightness;
        if let Some((mut theme, override_active)) =
            theme_after_override_poll(self.theme_override.poll(), default_theme, || {
                platform_brightness
            })
        {
            // A whole-`Theme` swap re-bases the reduced-motion floor: the
            // incoming theme carries its own authored token, so record that and
            // re-apply the OS report over it. Without this, a `set_app_theme`/
            // `clear_app_theme` would silently un-reduce motion while the
            // platform setting is still on (`effective_reduce_motion`).
            self.authored_reduce_motion = theme.motion.reduce_motion;
            theme.motion.reduce_motion =
                effective_reduce_motion(self.authored_reduce_motion, self.os_reduce_motion);
            self.theme = theme;
            self.theme_override_active = override_active;
            self.push_theme();
            theme_or_appearance_changed = true;
        }

        // Poll the app-facing pending-font registry once per frame,
        // beside the theme poll above and before the surface-ready gate — the
        // drain needs no renderer, so it stays in sync through surface churn.
        // `drain_into` applies any late-registered fonts to `text_ctx` (clearing
        // the shape cache internally) and returns whether anything registered.
        // On a late drain, force the relayout `register_fonts` documents by
        // re-pushing the currently-active theme through `set_theme` (via
        // `push_theme` — the same LAYOUT|PAINT contract a theme swap uses, no new
        // core API), so `Text`'s layout-baked shaping re-runs against the new
        // faces; that also feeds the frame gate (`change_flags_pending`, plus the
        // explicit `theme_or_appearance_changed` bit here per the default-to-run
        // rule). When nothing is pending this is one cheap `Mutex` check.
        if self.font_registry.drain_into(&mut self.text_ctx) {
            self.push_theme();
            theme_or_appearance_changed = true;
        }

        // Apply accessibility actions queued by assistive tech since the last
        // frame, before the surface-ready gate and before the
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

        // Resolved-translucency sync, before the gate
        // inputs are gathered: a render-thread fallback-to-opaque flips
        // `RenderRoot::set_surface_translucent` to `false`, which marks
        // `ChangeFlags::PAINT` and therefore forces THIS frame to run (via
        // `change_flags_pending` below) and repaint without the hole punch.
        // The returned value also drives the base color further down, so the
        // clear color and the punch contract can never disagree.
        let translucent_resolved = self.sync_translucent_resolved();

        // Reactive signals-dirty, drained only past the surface-ready
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
            // pending signal ORs into the events input (the
            // "never starves the gate" contract; default-to-run rule).
            events_since_last_frame: std::mem::take(&mut self.events_since_last_frame)
                || self.resampler.has_pending(),
            pointer_capture_active: self.app.is_pointer_captured(),
            focus_or_ime_active: self.app.is_focus_active() || self.app.ime_state().is_some(),
            last_needs_frame: self.last_needs_frame,
            last_needs_frame_paced_only: self.last_needs_frame_paced_only,
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

        // Perf instrumentation: the process-wide
        // switch is one cached bool read (`perf::enabled`'s `OnceLock`), not a
        // clock read — every `Instant::now()` below is gated behind it via
        // `bool::then`, so a disabled build/run never reads a timer on this
        // hot path (guard first, per this module's perf convention).
        let perf_on = perf::enabled();

        // Deadline-aware pacing: estimate this frame's
        // target budget from the tick-to-tick delta, updating the stored tick
        // every frame (skip or run) so the estimate always reflects one refresh
        // interval rather than a gap across skipped ticks.
        let frame_interval = resample::frame_interval_nanos(
            self.last_frame_time_nanos.replace(frame_time_nanos),
            frame_time_nanos,
        );

        // Animation pacing (frame-gate pacing): a frame whose ONLY dirtiness is
        // a paced (CosmeticLoop) request is throttled to the active theme's
        // `cosmetic_loop_rate` rather than reproduced every Choreographer tick.
        // `now` is this tick's Choreographer clock (the same domain `paint`
        // consumes below); the interval is `1 / rate` resolved from the live
        // theme so an app that retunes the token re-paces without a restart.
        // Every other FrameInputs signal still forces an immediate Run — pacing
        // never delays real work (see `frame_gate`'s pacing docs).
        let pacing = FramePacing {
            now: FrameTime::from_nanos(frame_time_nanos),
            interval: Duration::from_secs_f32(1.0 / self.theme.motion.cosmetic_loop_rate.hz()),
        };

        if self.frame_gate.decide_paced(inputs, pacing).is_skip() {
            // Skip path: nothing changed — return before rebuild, so
            // CPU/GPU stay near idle. Inline records a `skipped` FramePasses
            // (all-zero pass durations) so the skip counter accumulates in the
            // perf log line. In the render-thread split a Skip sends **nothing**
            // across the channel (the render thread is the
            // single emitter and never sees skipped frames), so `record_skip` is a
            // no-op there. Either way only frame *production* stops; the
            // Choreographer keeps re-posting callbacks, so the loop cadence is
            // unchanged.
            self.executor.record_skip();
            return;
        }

        // ---------------------------------------------------------------
        // Run path: rebuild -> (layout iff needed) -> paint -> encode/present,
        // timed as before.
        // ---------------------------------------------------------------

        // Pointer resampling: drain buffered samples up to
        // this frame's sample instant and feed the interpolated events into the
        // tree BEFORE the rebuild, so the rebuild reflects this frame's
        // resampled input. Uses the same `resample_clock` domain the raw samples
        // were stamped in. A no-op when the resampler is disabled (touches were
        // delivered directly in `dispatch_touch`). The scratch buffer and the
        // tree are borrowed as disjoint fields so the batch can be iterated
        // in place while dispatching.
        if self.resampler.is_enabled() {
            let now_nanos = self.resample_clock.elapsed().as_nanos() as u64;
            self.pointer_scratch.clear();
            self.resampler
                .resample(now_nanos, &mut self.pointer_scratch);
            // One owner install for the whole drained batch (see
            // [`under_root_owner`]), so the per-frame cost stays flat regardless
            // of how many samples landed.
            let app = &mut self.app;
            let scratch = &self.pointer_scratch;
            under_root_owner(|| {
                for sample in scratch {
                    let _ = app.event(&InputEvent::Pointer(*sample));
                }
            });
        }

        // Rebuild under the root `Owner` AND inside the persistent
        // [`TrackedScope`] so every signal read this frame subscribes the scope:
        // a later write to any of them trips `signals_dirty` (drained above into
        // `FrameInputs::signals_dirty`), so the frame gate runs the frame that
        // paints the change. Without the `scope.track` wrap a completed async
        // load's write would notify no subscriber and the gate would skip until a
        // touch forced a `Run` (a device-only "stuck on
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

        // Prompt teardown retire: the rebuild just above
        // is where a removed `platform_view` widget's `View::teardown` runs and
        // reports its slot id. Drain those and dispose each native view right
        // now, instead of waiting out the differ's ~30-frame missing-streak
        // heuristic (which cannot tell a torn-down slot from a culled one). A
        // merely culled slot reports nothing here, so the streak still covers
        // it — that asymmetry is the camera keep-alive contract. Emitted before
        // this frame's `ingest` below, so the Dispose leads the batch; it is a
        // lifecycle command, deliberately NOT paired with a frame (like
        // `suspend_all`), so it releases immediately.
        for slot_id in self.app.take_retired_platform_views() {
            self.platform_view_state.retire(slot_id);
        }

        // Layout-skip seam (see the frame_gate module docs): drain the change
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

        // Publish the accessibility tree post-layout, so node
        // bounds are valid. A cheap no-op unless the tree changed AND a screen
        // reader is active (double-gated inside).
        self.publish_semantics();

        // Push the render side's presented-frame count so a widget measuring FPS
        // reports the presented rate, not its Choreographer paint cadence. A pure
        // observation — `set_presented_frames` marks no ChangeFlags,
        // so a ticking counter never dirties layout NOR feeds the frame gate (the
        // gate decision already ran above and never reads this), keeping the
        // menu-idle behavior intact.
        self.app
            .set_presented_frames(self.executor.presented_frames());

        self.scene.reset();
        let paint_start = perf_on.then(Instant::now);
        {
            let mut builder = SceneBuilder::new(&mut self.scene);
            // HiDPI: lay out in logical pixels, then scale the
            // whole scene by the device pixel ratio for sharp glyphs.
            builder.push_transform(Affine::scale(scale));
            // Shell-owned frame clock (time enters from the shell, never
            // `Instant::now()` inside `frust-core`) — Choreographer's
            // `frameTimeNanos`, forwarded from Kotlin via `nativeOnFrame`.
            let frame_time = FrameTime::from_nanos(frame_time_nanos);
            // The paint pass returns a `needs_frame` continuation signal, the
            // framework's animation seam. The Choreographer keeps posting frames, but
            // the frame gate now decides whether each is *produced* —
            // so this flag is no longer irrelevant: latch it into
            // `last_needs_frame` so an in-flight animation/transition forces the
            // next frame to run (and stops forcing once it settles).
            let outcome = self.app.paint(&mut builder, frame_time);
            self.last_needs_frame = outcome.needs_frame;
            // Latch the aggregated tick-class so the NEXT frame's gate can pace a
            // paced-only decorative loop (see `FrameInputs::last_needs_frame_paced_only`).
            self.last_needs_frame_paced_only = outcome.needs_frame_paced_only;
            builder.pop_transform();
        }
        let paint_time = paint_start.map_or(Duration::ZERO, |t| t.elapsed());

        // Ingest this RUN frame's published platform-view frames into the
        // differ, right after paint — the source paint just
        // populated. Never reached on a Skip (this whole block is behind the
        // gate's early `return` above), so the differ's skip-safety contract
        // (a rect can't "move" during a skip) holds by construction. There is
        // no "push to Kotlin now" path — `nativePlatformViewCommands` is a poll
        // Kotlin drives from its own per-frame callback, mirroring
        // `nativeImeState`/`nativeSystemUiState`.
        //
        // When the differ produced something, pair that batch with the frame
        // that painted it — the one submitted just below, i.e. the submission
        // cursor plus one — so the release gate holds the batch until that
        // frame is on screen. Both statements sit behind the
        // gate's early `return`, so a Skip records nothing AND submits nothing:
        // the recorded id can never run ahead of what will actually be sent.
        if self
            .platform_view_state
            .ingest(self.app.platform_view_frames(), self.app.input_shields())
        {
            let (generation, _) = self.platform_view_state.commands();
            self.platform_view_due
                .record(generation, self.executor.submitted_frame_id() + 1);
        }

        // Hand the finished frame to the render-path executor.
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
        // flip clears correctly.
        let ui = UiSpans {
            rebuild: rebuild_time,
            layout: layout_time,
            paint: paint_time,
            skipped: false,
        };
        // Platform-views translucent mode: a
        // surface that RESOLVED translucent (`translucent_resolved`, read at
        // the top of this frame — not the request latch) must clear to alpha-0,
        // not the theme's opaque surface color, so a native sibling view placed
        // behind it shows through wherever this frame painted nothing (Mode B —
        // see `docs/ARCHITECTURE.md`'s Platform-view flow). Opaque —
        // requested-but-unavailable included: bit-for-bit today's behavior.
        let base_color = crate::ffi_support::base_clear_color(
            translucent_resolved,
            peniko::Color::TRANSPARENT,
            self.theme.scheme().surface,
        );
        let size = SurfaceSize {
            width: self.physical.0,
            height: self.physical.1,
            scale: self.scale as f64,
        };
        let frame_time = FrameTime::from_nanos(frame_time_nanos);
        let encode_time =
            self.executor
                .submit_frame(&mut self.scene, base_color, ui, frame_time, size, perf_on);

        // Deadline-aware pacing overrun: this frame's *work*
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

/// Unit tests for the pure accessibility-tree assembly and the default-theme
/// seed ladder.
///
/// This module is inside the `#[cfg(target_os = "android")]` `app` module, so it
/// only compiles/runs for the Android target — the assembly references
/// `frust_core`/`accesskit` types (and the seed ladder `frust_theme`'s
/// [`Theme`]), all of which are Android-gated
/// dependencies of this crate by deliberate design (see `Cargo.toml`), so it
/// cannot be a host test the way [`crate::ffi_support`]'s pure helpers are —
/// worse, the documented Android compile gate (`cargo check --target
/// aarch64-linux-android -p frust`) never builds test cfg either, so nothing
/// below is even type-checked without an explicit `--all-targets`.
///
/// The theme ladder is therefore guarded off-device by two things that DO run
/// on every `cargo test --workspace`: the desktop shell's twin of each ladder
/// test (`frust-shell-desktop`'s `app_handler::tests`), and
/// `crates/frust/tests/theme_ladder_conformance.rs`, whose source scan pins
/// this shell's arms to the extracted helpers and pins those helpers' bodies
/// identical to the desktop copies the twin tests exercise.
#[cfg(test)]
mod tests {
    use super::{
        app_is_dark, base_theme, effective_reduce_motion, follow_platform_brightness,
        theme_after_override_poll, tree_update_from_semantics,
    };
    use frust_core::SemanticsUpdate;
    use frust_core::accesskit::{Node, NodeId, Role, Tree, TreeId};
    use frust_theme::{Brightness, Theme};

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

    // --- the default-theme precedence ladder (seed / appearance / override) ---
    //
    // Every assertion below drives the SAME ladder helpers the production seed
    // ([`AndroidAppHandle::new`]), appearance (`nativeSetAppearance`) and
    // override-poll ([`AndroidAppHandle::frame`]) arms call — a test that
    // recomputed the ladder in its own body would stay green if one of those
    // arms regressed to an unconditional `Theme::neutral()`. What no assertion
    // here can see is how an arm *composes* those helpers (a seed site passing
    // `None` instead of `default_theme()` drives the same `base_theme`).
    //
    // These do not run in a host `cargo test --workspace` (see this module's
    // own docs), so they are not this ladder's only guard:
    // `crates/frust/tests/theme_ladder_conformance.rs` pins each arm's call
    // site — including the seed site's `base_theme(default_theme())` — AND pins
    // these helpers' bodies identical to the desktop shell's, whose twin of
    // every test below does run on the host.

    #[test]
    fn an_unseeded_shell_starts_on_the_builtin_fallback_at_the_platform_brightness() {
        // Behavior 1. Nothing seeded: the seed site lands on the shell's own
        // built-in, design-language-free floor...
        let mut theme = base_theme(None);
        assert_eq!(theme, Theme::neutral());

        // ...and Kotlin's follow-up `nativeSetAppearance` drives brightness, so
        // the fallback's own starting brightness never leaks onto a
        // dark-preference device.
        follow_platform_brightness(&mut theme, false, Brightness::Light);
        assert_eq!(theme, Theme::neutral().with_brightness(Brightness::Light));
        follow_platform_brightness(&mut theme, false, Brightness::Dark);
        assert_eq!(theme, Theme::neutral().with_brightness(Brightness::Dark));
    }

    #[test]
    fn a_seeded_default_is_the_base_and_still_follows_platform_brightness() {
        // Behavior 2. A design system's `set_default_theme` supplies the base...
        let seeded = Theme::m3_baseline();
        let mut theme = base_theme(Some(seeded.clone()));
        assert_eq!(theme, seeded);
        // ...in place of the built-in floor, not layered over it.
        assert_ne!(theme, Theme::neutral());

        // ...and unlike an app-forced override it does NOT pin brightness: the
        // `set_appearance` arm keeps flipping the seeded base in place.
        follow_platform_brightness(&mut theme, false, Brightness::Dark);
        assert_eq!(theme, seeded.clone().with_brightness(Brightness::Dark));
        follow_platform_brightness(&mut theme, false, Brightness::Light);
        assert_eq!(theme, seeded.with_brightness(Brightness::Light));
    }

    #[test]
    fn an_app_theme_override_beats_a_seeded_default_and_pins_brightness() {
        // Behavior 3. The override poll's `Some(Some(theme))` arm takes the
        // forced theme wholesale. The suppliers panic rather than answer, which
        // proves more than an inequality could: the arm cannot even observe the
        // seeded default or the platform brightness, so no seeded value and no
        // device preference can influence what an override resolves to.
        let forced = Theme::cupertino_baseline().with_brightness(Brightness::Light);
        let decided = theme_after_override_poll(
            Some(Some(forced.clone())),
            || panic!("an active override must not consult the seeded default"),
            || panic!("an active override must not consult the platform brightness"),
        );
        assert_eq!(decided, Some((forced, true)));

        // ...and it keeps winning against a *later* `nativeSetAppearance` flip
        // (the override-wins rule), exactly where the seeded default of the
        // test above followed the platform instead.
        let mut active = Theme::cupertino_baseline().with_brightness(Brightness::Light);
        follow_platform_brightness(&mut active, true, Brightness::Dark);
        assert_eq!(active.brightness, Brightness::Light);
    }

    #[test]
    fn clearing_an_override_reverts_to_the_seeded_default_not_the_builtin() {
        // Behavior 4. The `Some(None)` arm with a design system's default
        // seeded: the revert lands on THAT base at the device's last reported
        // brightness (`platform_brightness`) — not on the built-in fallback,
        // and not on the cleared override's pinned brightness.
        let seeded = Theme::m3_baseline();
        let decided =
            theme_after_override_poll(Some(None), || Some(seeded.clone()), || Brightness::Dark);
        assert_eq!(
            decided,
            Some((seeded.with_brightness(Brightness::Dark), false))
        );
        // Spelled out, since this is the arm the ladder exists for: a seeded
        // shell must NOT revert to the built-in fallback.
        assert_ne!(
            decided.map(|(theme, _)| theme),
            Some(Theme::neutral().with_brightness(Brightness::Dark))
        );
    }

    #[test]
    fn clearing_an_override_with_nothing_seeded_reverts_to_the_builtin() {
        // Behavior 5. The same arm with an empty slot: the built-in fallback at
        // the platform's brightness.
        let decided = theme_after_override_poll(Some(None), || None, || Brightness::Dark);
        assert_eq!(
            decided,
            Some((Theme::neutral().with_brightness(Brightness::Dark), false))
        );
    }

    #[test]
    fn a_poll_reporting_no_change_leaves_the_active_theme_alone() {
        // The `None` arm: no `set_app_theme`/`clear_app_theme` since the last
        // tick, so the shell must not touch its theme — and must not pay the
        // process-global slot read, which would otherwise run on every
        // Choreographer tick.
        let decided = theme_after_override_poll(
            None,
            || panic!("an unchanged poll must not read the process-global default slot"),
            || panic!("an unchanged poll must not read the platform brightness"),
        );
        assert_eq!(decided, None);
    }

    // --- the app-facing brightness getter (`nativeAppIsDark`) ----------------
    //
    // FINDINGS #43's bug half: the Kotlin status-bar icon contrast must follow
    // the APP's theme, not the device's `Configuration.uiMode`. These pin
    // `app_is_dark` — the pure decision `AndroidAppHandle::is_dark_theme`
    // (`nativeAppIsDark`) wraps — against the exact scenario that regresses
    // without this seam: an app-forced override disagreeing with the device.

    #[test]
    fn app_is_dark_tracks_an_active_override_against_a_disagreeing_device() {
        // The device reports light (`isDarkMode == false` in Kotlin terms) but
        // the app forced a dark theme via `frust::set_app_theme` — the exact
        // "permanently-dark app on a light-mode device" case FINDINGS #43
        // observed on-device. `app_is_dark` must report the APP's theme (dark),
        // not the device's (light) — the whole point of this seam existing.
        let device_reports_light = Brightness::Light;
        let mut theme = Theme::cupertino_baseline().with_brightness(Brightness::Dark);
        // Mirrors `AndroidAppHandle::set_appearance`'s call shape: an
        // in-effect override (`override_active = true`) must not let a
        // disagreeing platform report flip the resolved brightness.
        follow_platform_brightness(&mut theme, true, device_reports_light);
        assert!(
            app_is_dark(&theme),
            "an app-forced dark override must stay dark even though the device reports light"
        );
    }

    #[test]
    fn app_is_dark_tracks_an_active_override_the_other_way_too() {
        // The symmetric case: device reports dark, app forced light — icons
        // must stay dark-appropriate (light-on-light is invisible), not follow
        // the device into a light-on-light mismatch.
        let device_reports_dark = Brightness::Dark;
        let mut theme = Theme::m3_baseline().with_brightness(Brightness::Light);
        follow_platform_brightness(&mut theme, true, device_reports_dark);
        assert!(
            !app_is_dark(&theme),
            "an app-forced light override must stay light even though the device reports dark"
        );
    }

    #[test]
    fn app_is_dark_follows_the_device_when_no_override_is_active() {
        // No `set_app_theme` in effect: the pre-existing (correct) behavior —
        // the app's own brightness tracks the platform report, so
        // `app_is_dark` and the device agree, same as before this seam existed.
        let mut theme = Theme::neutral().with_brightness(Brightness::Light);
        follow_platform_brightness(&mut theme, false, Brightness::Dark);
        assert!(app_is_dark(&theme));
        follow_platform_brightness(&mut theme, false, Brightness::Light);
        assert!(!app_is_dark(&theme));
    }

    // --- the reduced-motion floor (`nativeSetReduceMotion`) ------------------

    #[test]
    fn the_os_reduced_motion_report_raises_and_lowers_an_unreduced_theme() {
        // The ordinary case: every shipped baseline authors `false`, so the
        // effective value tracks the OS setting in both directions.
        assert!(effective_reduce_motion(false, true));
        assert!(!effective_reduce_motion(false, false));
    }

    #[test]
    fn a_theme_authored_reduced_stays_reduced_while_the_os_setting_is_off() {
        // The floor's whole point: the OS report may only ever ADD reduction.
        // A theme built with `reduce_motion: true` (a `ThemeBuilder::map_motion`
        // app choice) must not be un-reduced by a device that has the setting
        // off.
        assert!(effective_reduce_motion(true, false));
        assert!(effective_reduce_motion(true, true));
    }

    #[test]
    fn the_floor_applies_to_an_app_forced_override_too() {
        // Unlike brightness, an active `set_app_theme` override does NOT pin
        // this: the frame arm re-bases `authored_reduce_motion` from the
        // incoming theme and re-applies the OS report over it, so a live
        // accessibility toggle still reaches a catalog app that forced its own
        // theme. This asserts the composition that arm performs.
        let forced = Theme::cupertino_baseline();
        let authored = forced.motion.reduce_motion;
        assert!(!authored, "the shipped Cupertino baseline authors `false`");
        let mut active = forced;
        active.motion.reduce_motion = effective_reduce_motion(authored, true);
        assert!(active.motion.reduce_motion);
    }
}
