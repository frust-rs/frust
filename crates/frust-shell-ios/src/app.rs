//! The iOS app runtime: [`IosAppHandle`], the state behind the opaque C handle.
//!
//! This module is `#[cfg(target_os = "ios")]`; it owns the same resources the
//! desktop shell's `ShellHandler` and the Android shell's `AndroidAppHandle` do —
//! a [`RenderContext`], [`SurfaceRenderer`], [`TextContext`], reusable [`Scene`],
//! plus the app tree — but is driven by the generated Swift app's
//! `CADisplayLink`-posted `frust_render_frame` calls instead of a winit loop
//! or Choreographer. It contains no `unsafe`; the FFI boundary lives entirely in
//! [`crate::ffi_glue`]. The `State`/`app_logic` erasure it drives
//! ([`AppTree`](frust_shell_common::AppTree)) is platform-agnostic and lives
//! in `frust-shell-common`.
//!
//! # Layer lifetime contract
//!
//! This handle retains the Swift-owned `CAMetalLayer` as a raw `*mut c_void`
//! (`metal_layer`) so the shell can *recreate* the `wgpu::Surface` after a
//! `SurfaceLost` — iOS never destroys/recreates the layer itself (contrast
//! Android's window cycle), so without the retained pointer a lost surface would
//! be terminal (permanent black screen). Retaining the raw pointer is sound
//! because the layer's ownership stays with Swift and Swift guarantees it
//! outlives this handle: `frust_destroy` drops the handle (and with it the
//! `wgpu::Surface`) *before* the view/layer is released. The handle never frees
//! the layer — it only reads the pointer to hand it back to
//! `on_surface_created_from_metal_layer` at the FFI boundary.
//!
//! The `*mut c_void` field makes [`IosAppHandle`] `!Send`/`!Sync` by default,
//! which is exactly right: every `frust_*` call is on the UIKit main thread,
//! so the handle is never sent across threads and no auto-trait promise is made
//! about it.

use std::any::Any;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::accessibility::IosA11yAdapter;
use frust_core::FrameTime;
use frust_core::event::{
    EditingState, ImeState, InputEvent, PointerButton, PointerEvent, PointerPhase,
};
use frust_core::insets::WindowInsets;
use frust_reactive::{ReactiveRuntime, TrackedScope, provide_context};
use frust_render::{
    AcquireOutcome, DeferredPresent, EncodeOutcome, FrameOutcome, RenderContext,
    SurfaceAlphaRequest, SurfacePhase, SurfaceRenderer,
};
use frust_scene::{Scene, SceneBuilder};
use frust_shell_common::font_registry::FontRegistryWatcher;
use frust_shell_common::perf::{self, FramePasses, FrameStats, RenderSpans, StartupSpans, UiSpans};
use frust_shell_common::platform_view::{FramePairing, ViewCommand};
use frust_shell_common::resample::{self, PointerResampler, RawPointerSample};
use frust_shell_common::{
    AppTree, FrameGate, FrameInputs, FrameMeta, FramePacing, PlatformViewState, RenderCommand,
    RenderSender, SceneFrame, SceneReturnReceiver, SurfaceSize, ThemeOverrideWatcher,
    WindowMetricsPublisher, default_theme, effective_brightness_for_platform_change,
    logical_insets, logical_size, publish_resolved_surface_mode, sanitize_scale,
};
use frust_text::TextContext;
use frust_theme::{Brightness, Theme};
use kurbo::{Affine, Point, Size};
use objc2::rc::autoreleasepool;

use crate::ffi_support::TouchPhase;

/// Everything a running iOS app needs across frames — the state behind the opaque
/// handle Swift passes back into every C call.
///
/// The `executor` owns the render path: in the render-thread
/// split ([`FrameExecutor::Split`], the default) a dedicated thread owns the
/// `RenderContext`/`SurfaceRenderer` + the `wgpu::Surface` and this UI thread only
/// hands it finished scenes; in the inline fallback ([`FrameExecutor::Inline`],
/// `FRUST_NO_RENDER_THREAD`) the renderer lives on this thread. Either way the
/// `wgpu::Surface` is torn down before the `metal_layer` it was built from — in
/// the split, [`SplitExecutor`]'s `Drop` joins the render thread (dropping its
/// surface) before `frust_destroy` returns, and the `metal_layer` pointer is
/// Swift-owned and merely retained (never freed) here so a lost surface can be
/// recreated (see the module docs' *Layer lifetime contract*).
pub struct IosAppHandle {
    /// The render-path half of the frame loop: either the
    /// render-thread split ([`FrameExecutor::Split`], default) — where a dedicated
    /// thread owns the [`RenderContext`]/[`SurfaceRenderer`] + surface and the UI
    /// thread only hands it finished scenes — or the pre-split inline fallback
    /// ([`FrameExecutor::Inline`], `FRUST_NO_RENDER_THREAD`) where the renderer
    /// lives on this UI thread. Chosen once at construction.
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
    /// The Swift-owned `CAMetalLayer*` this handle's surface was built from,
    /// retained so the shell can recreate the surface after a `SurfaceLost` (iOS
    /// keeps the same layer for the app's whole lifetime). Read-only from Rust's
    /// side — never dropped/freed here (see the module docs' lifetime contract);
    /// only handed back to `on_surface_created_from_metal_layer` at the FFI
    /// boundary in [`crate::ffi_glue`], where the `unsafe` stays confined.
    metal_layer: *mut c_void,
    /// The current surface's physical (pixel) size, updated on create/resize and
    /// divided by `scale` to lay out in logical pixels.
    physical: (u32, u32),
    /// Display scale (`UIScreen.scale` / the layer's `contentsScale`), the device
    /// pixel ratio the whole scene is scaled by so glyphs rasterise sharp. Stored
    /// raw and sanitised once per frame.
    scale: f32,
    /// Set by `frust_pause`/`frust_resume`; while paused, `frame()` is a
    /// no-op — Metal command submission from a backgrounded iOS app can get the
    /// process killed, so this is the Rust-side enforcement point.
    paused: bool,
    /// Consecutive failed surface-recreate attempts in the current `SurfaceLost`
    /// episode. Compared against `ffi_support::MAX_RECREATE_ATTEMPTS` so a
    /// persistently-failing recreate degrades to a logged stop instead of a
    /// per-CADisplayLink-frame retry storm; reset by a successful recreate
    /// ([`Self::set_surface`]).
    recreate_failures: u8,
    /// The app's active theme — the seeded base ([`base_theme`]: a design
    /// system's `set_default_theme`, else the built-in fallback) until an
    /// app-forced override replaces it. Mirrors the desktop shell's
    /// appearance ownership: starts [`Brightness::Light`] here and is
    /// flipped by [`Self::set_appearance`] once Swift reports the platform's
    /// real dark-mode preference (`frust_set_appearance`, called right after
    /// `frust_init` returns a handle and again from `traitCollectionDidChange`
    /// — see `platform/ios/FrustEmbedding/Sources/FrustEmbedding/FrustViewController.swift`).
    theme: Theme,
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
    /// (`UIAccessibility.isReduceMotionEnabled`, delivered by
    /// `frust_set_reduce_motion` → [`Self::set_reduce_motion`]), tracked
    /// independently of `self.theme.motion.reduce_motion` for the same reason
    /// [`Self::platform_brightness`] is: a theme swap replaces the token and
    /// this latch is what re-raises the floor over the new theme. Starts
    /// `false` (no OS report yet — Swift pushes the real value right after
    /// `frust_init`, alongside `frust_set_appearance`).
    os_reduce_motion: bool,
    /// The **active theme's own** `motion.reduce_motion` token, captured every
    /// time a whole `Theme` is installed ([`Self::new`]'s seed and the
    /// override-poll arm in [`Self::frame`]). Paired with
    /// [`Self::os_reduce_motion`] through [`effective_reduce_motion`] so the
    /// OS report is a floor over the authored value rather than a replacement
    /// of it — without this, turning the OS setting back off would have to
    /// guess what the theme originally asked for.
    authored_reduce_motion: bool,
    /// The last window insets pushed to the render root, in logical px. Retained
    /// so [`Self::set_insets`] skips a no-op push
    /// (`WindowInsets` is `PartialEq`) — both the relayout and the app-side
    /// `provide_context` re-provide only fire on a real change. Starts zero until
    /// Swift's first `frust_set_insets` (safe-area / keyboard-frame report).
    insets: WindowInsets,
    /// Change detector for the app-facing [`WindowMetrics`](frust_core::WindowMetrics)
    /// context: seeded before the first rebuild in [`Self::new`] and re-polled
    /// from every entry point where one of its inputs actually moves
    /// ([`Self::resize`]/[`Self::set_surface`], and [`Self::set_insets`]) —
    /// never from [`Self::frame`], which only reads values those entry points
    /// stored. See [`Self::push_window_metrics`] for why the guard is
    /// load-bearing.
    window_metrics: WindowMetricsPublisher,
    /// The accesskit adapter, attached lazily by `frust_init_accessibility`
    /// once Swift has a `FrustView` (UIView) to hand over.
    /// `None` until then — `frust_init` only receives the `CAMetalLayer`, which
    /// the accesskit `SubclassingAdapter` cannot subclass. While `Some`, `frame()`
    /// drains its queued a11y actions (pre-rebuild) and pushes the post-layout
    /// semantics tree to it (see [`Self::frame`]).
    a11y: Option<IosA11yAdapter>,
    /// The per-frame skip gate: consulted each
    /// CADisplayLink tick to skip the rebuild/layout/paint/encode passes on an
    /// idle frame (nothing changed), so CPU/GPU stay near zero on a static
    /// screen — the iOS counterpart to the Android shell's frame gate.
    /// Honors the `FRUST_NO_FRAME_GATE` kill switch (resolved once at
    /// construction — a set switch makes every frame run, pre-gate behavior
    /// verbatim). Its resume-warmup is (re)opened on resume / resize / surface
    /// recreation via [`FrameGate::note_resumed`] so the first ticks after those
    /// transitions always run (their change signals may not be observable yet).
    frame_gate: FrameGate,
    /// Handle-side latch feeding [`FrameInputs::events_since_last_frame`]: set by
    /// [`Self::dispatch_touch`]/[`Self::ime_apply`] whenever a touch/IME event
    /// reaches the tree between frames, read and cleared once per [`Self::frame`].
    /// Ensures a tap/keystroke on an otherwise-idle screen is never skipped.
    events_since_last_frame: bool,
    /// Handle-side latch feeding [`FrameInputs::last_needs_frame`]: the previous
    /// paint's [`frust_core::PaintOutcome::needs_frame`] (an in-flight
    /// animation/transition asking for another frame). Latched at the end of each
    /// frame the gate runs; a skipped frame leaves it untouched. Without it the
    /// gate would skip the follow-up frame a running animation needs.
    last_needs_frame: bool,
    /// Handle-side latch feeding [`FrameInputs::last_needs_frame_paced_only`]:
    /// the previous paint's [`frust_core::PaintOutcome::needs_frame_paced_only`]
    /// (its frame request aggregated to [`frust_core::TickClass::CosmeticLoop`]
    /// alone). Lets [`FrameGate::decide_paced`] throttle a paced-only decorative
    /// loop to the theme's `cosmetic_loop_rate` instead of every `CADisplayLink`
    /// tick. Latched beside `last_needs_frame`; a skipped frame leaves it.
    last_needs_frame_paced_only: bool,
    /// Handle-side latch feeding [`FrameInputs::theme_or_appearance_changed`]:
    /// set by [`Self::set_appearance`] on an OS-driven light/dark flip and by
    /// [`Self::set_reduce_motion`] on a reduced-motion toggle, taken
    /// only past the pause/ready gate (like the signals-dirty drain) so a flip
    /// while backgrounded is observed by the first frame after resume.
    /// `push_theme`'s LAYOUT|PAINT change flags carry correctness either way;
    /// this explicit latch is belt-and-suspenders, mirroring the Android shell.
    appearance_dirty: bool,
    /// Pointer-event resampler — the iOS counterpart to the
    /// Android shell's field: buffers raw touch samples and emits
    /// frame-boundary-interpolated `Move`s while Down/Up/Cancel pass through
    /// losslessly. Disabled by the `FRUST_NO_RESAMPLE` kill switch (resolved
    /// once at construction), in which case [`Self::dispatch_touch`] delivers
    /// touches directly. Its pending-input signal ORs into the frame gate's
    /// `events_since_last_frame` so a too-new sample never starves the gate.
    resampler: PointerResampler,
    /// The monotonic epoch every resampler timestamp is measured from — both the
    /// raw samples ([`Self::dispatch_touch`]) and the per-frame sample query
    /// ([`Self::frame`]) are stamped from this one `Instant`, sharing a clock
    /// domain (see `resample`'s *Clock domain* note). Decoupled from the
    /// `CADisplayLink` vsync clock threaded into [`FrameTime`]: resampling only
    /// differences timestamps, so one consistent source suffices.
    resample_clock: Instant,
    /// Scratch buffer the resampler drains into each frame, reused (cleared, not
    /// reallocated) so a drag's per-frame resample allocates nothing.
    pointer_scratch: Vec<PointerEvent>,
    /// The previous `CADisplayLink` tick timestamp (ns), for the deadline-aware
    /// pacing estimate: the tick-to-tick delta is this
    /// frame's deadline budget (see [`resample::frame_interval_nanos`]). `None`
    /// before the first frame.
    last_frame_time_nanos: Option<u64>,
    /// Running count of frames whose measured work (rebuild+layout+paint+encode,
    /// excluding the vsync present wait) overran the frame-target deadline.
    /// **Instrumentation only** — accumulated and logged
    /// (`frust-perf deadline`) behind [`perf::enabled`]; never drops work.
    deadline_overruns: u64,
    /// The surface-alpha **request** this handle's surface was (or, for the
    /// split, will be) created with — latched once at
    /// construction from `SurfaceModeWatcher::current()` (read in
    /// [`crate::ffi_glue::create_handle`], before any surface exists) and read
    /// back by [`crate::ffi_glue::recover_surface`] so an inline surface
    /// recreate asks for the same alpha mode the initial surface did.
    ///
    /// **A request, not an outcome**: it does NOT drive the paint contract —
    /// [`Self::translucent_resolved`] does.
    surface_alpha: SurfaceAlphaRequest,
    /// Whether this handle's surface **actually came up** translucent — the
    /// RESOLVED capability behind [`Self::frame`]'s base-color swap and
    /// `RenderRoot::set_surface_translucent`.
    ///
    /// `frust-render` resolves [`Self::surface_alpha`] against the platform's
    /// advertised alpha modes and can silently fall back to an opaque
    /// swapchain; keying the transparent base color and the `platform_view`
    /// hole punch off the request alone would then `DestOut`-zero real pixels
    /// and present black rectangles. So the "fixed before the surface exists,
    /// never changes" invariant is **false** — a (re)install can downgrade
    /// this, and every install path re-resolves it.
    ///
    /// An `Arc<AtomicBool>` (the `fatal`/`presented` cross-thread pattern)
    /// because in the default render-thread split the surface is created — and
    /// self-healed after a `SurfaceLost` — on the RENDER thread, while this (UI)
    /// thread owns the `RenderRoot` and the per-frame base color. **Seeded from
    /// the request at construction** so the capable common case renders Mode B
    /// from frame 1 with no flicker; only a real resolution may downgrade it,
    /// observed within one frame (see [`Self::sync_translucent_resolved`]).
    translucent_resolved: Arc<AtomicBool>,
    /// The platform-view command differ: turns the
    /// tree's per-paint-pass [`frust_core::widget::PlatformViewFrame`]s into
    /// the idempotent command backlog `frust_platform_view_commands_json`
    /// serves. Fed via [`AppTree::platform_view_frames`] after every RUN
    /// frame's paint (see [`Self::frame`]) — never on a gate-skipped frame,
    /// per [`PlatformViewState::ingest`]'s skip-safety contract.
    platform_views: PlatformViewState,
    /// The release gate pairing each published command batch with the frust
    /// frame that painted its geometry (the shared
    /// [`FramePairing`]) — **only consulted while
    /// [`Self::present_sync`] is on**, which is what makes it correct here:
    /// the UI thread presents the frames itself, so it knows exactly which one
    /// just landed.
    ///
    /// Unlike Android, where the gate is always on, iOS holds geometry only in
    /// the present-sync configuration. Ungated, iOS presents eagerly off the
    /// render thread, and holding the geometry back is the WRONG sign of
    /// correction there (the native view already lags the frust surface, and the
    /// frame-id gate measures as a no-op on iOS — it submits/presents 1:1 with no
    /// skipped frames to remove). Paired with the deferred
    /// present it is the right one: the geometry waits for the frame it belongs
    /// to, and that frame lands in the very transaction that releases it.
    platform_view_due: FramePairing,
    /// The id of the last frame **this thread presented** through
    /// [`Self::present_pending_frame`] — the release gate's "is that frame on
    /// screen yet?" input. UI-thread-owned (a plain `u64`, no atomic): under
    /// present-sync the render thread never presents, so there is no second
    /// writer. Stays `0` — and is never read — outside present-sync.
    presented_frame_id: u64,
    /// Whether present-sync is active for this handle: the host armed it *and*
    /// the render-thread split is engaged (the inline path presents on this
    /// thread already, so it neither parks frames nor advances
    /// [`Self::presented_frame_id`] — gating geometry on it there would stall
    /// the backlog behind a frame id that never moves). Fixed at construction.
    present_sync: bool,
}

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
    fn store(&self, frame_id: u64, frame: DeferredPresent) {
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

/// The render-path half of the iOS frame loop: either the
/// render-thread split ([`Self::Split`], default) or the pre-split inline fallback
/// ([`Self::Inline`], `FRUST_NO_RENDER_THREAD`). Chosen once at construction from
/// [`render_thread_enabled`](frust_shell_common::render_thread_enabled) and owned
/// by [`IosAppHandle`].
pub(crate) enum FrameExecutor {
    /// Pre-split fallback: the [`RenderContext`]/[`SurfaceRenderer`] and all perf
    /// recording live on the UI thread, and the encode→acquire→submit tail runs
    /// synchronously inside [`IosAppHandle::frame`]. Boxed — it owns the whole
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
    /// [`IosAppHandle::frame`]'s not-ready early return in place of `renderer.phase()`.
    /// Always `true` after construction on iOS (the `CAMetalLayer` is permanent
    /// and the render thread self-heals a lost surface — no UI-side destroy path).
    surface_active: bool,
    /// Monotonically increasing per-frame id stamped into [`FrameMeta`].
    frame_id: u64,
    /// Fatal-signal flag: the render thread stores `true` here
    /// if its **first** surface install fails — unrecoverable (an incapable
    /// GPU/driver can't change mid-process). The UI thread reads it via
    /// [`IosAppHandle::render_fatal`] each `frust_render_frame` and returns
    /// [`FRAME_FATAL`](crate::ffi_support::FRAME_FATAL) so Swift latches
    /// `initFailed` + invalidates its `CADisplayLink`. One clone here, one in the
    /// render thread ([`crate::ffi_glue::render_loop`]).
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
    /// Running count of presented frames: one clone here (read by the
    /// UI thread before paint via `FrameExecutor::presented_frames`), one on the
    /// render thread ([`crate::ffi_glue::render_loop`], which bumps it on each
    /// `FrameOutcome::Rendered`). A plain `Arc<AtomicU64>` — no channel/protocol,
    /// mirroring the `fatal`-flag pattern.
    presented: Arc<AtomicU64>,
    /// The present-sync handoff slot: one clone here (the UI
    /// thread takes and presents from it in
    /// [`IosAppHandle::present_pending_frame`]), one on the render thread
    /// ([`crate::ffi_glue::render_loop`], which parks each submitted frame in
    /// it instead of presenting when armed). Inert — never written, always
    /// empty — when the host did not arm present-sync, which is the default.
    /// See [`PresentHandoff`].
    present: Arc<PresentHandoff>,
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
    fn resize(&mut self, size: SurfaceSize) {
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
    fn pause_barrier(&mut self) {
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
    fn resume(&mut self) {
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
    /// Whether a live surface exists — the [`IosAppHandle::frame`] not-ready gate.
    /// Inline reads the renderer's phase directly; the split tracks a UI-side
    /// `surface_active` flag (the render thread owns the real phase).
    fn has_surface(&self) -> bool {
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
    fn presented_frames(&self) -> u64 {
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
    fn submitted_frame_id(&self) -> u64 {
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
    fn take_surface_reinstalled(&self) -> bool {
        match self {
            FrameExecutor::Inline(_) => false,
            FrameExecutor::Split(split) => split.surface_reinstalled.swap(false, Ordering::AcqRel),
        }
    }

    /// Record the `first_rebuild_done` startup milestone after the initial
    /// rebuild. Inline records it here on the UI thread; the split records it
    /// render-side when the first scene arrives (see [`crate::ffi_glue::render_loop`]),
    /// so this is a no-op there.
    fn record_first_rebuild(&mut self) {
        if let FrameExecutor::Inline(inline) = self {
            inline.record_first_rebuild();
        }
    }

    /// Record a gate-skipped frame. Inline accumulates it in its UI-side
    /// `FrameStats`; the split sends **nothing** on a skip (the render thread is
    /// the single emitter and never sees skipped frames — split mode records no
    /// skip frames; known, logged for 11.E), so this is a no-op there.
    fn record_skip(&mut self) {
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
    })
}

/// The base [`Theme`] this shell seeds itself from: the design-system-supplied
/// default (`frust_shell_common::set_default_theme`, read back through
/// [`default_theme`]) when a plugin seeded one, else the shell's own built-in
/// fallback.
///
/// A seeded default supplies only the *starting point*: unlike an app-forced
/// override (`set_app_theme`) it does not pin brightness — every call site
/// still derives `brightness` from the platform's own preference
/// (`frust_set_appearance` → [`IosAppHandle::platform_brightness`]) against
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
/// ([`IosAppHandle::platform_brightness`]), never the cleared override's own
/// pinned one.
///
/// Extracted so the `clear_app_theme` arm in [`IosAppHandle::frame`] and its
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
/// per-frame poll arm in [`IosAppHandle::frame`] and its unit tests.
///
/// Returns `None` when the poll reported no change (the shell leaves its theme
/// alone), else the new active theme paired with whether an app-forced override
/// is now pinning it ([`IosAppHandle::theme_override_active`]).
///
/// `seeded`/`platform` are suppliers rather than values because only the
/// cleared-override arm needs them: reading the process-global default slot
/// ([`default_theme`] — a `Mutex` lock plus a whole-`Theme` clone) would
/// otherwise become per-frame cost on every `CADisplayLink` tick, for a poll
/// that reports "nothing changed" on all but a handful of frames.
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
/// `frust_set_appearance` arm ([`IosAppHandle::set_appearance`]) and the
/// seed-ladder tests share one implementation.
fn follow_platform_brightness(theme: &mut Theme, override_active: bool, platform: Brightness) {
    theme.brightness =
        effective_brightness_for_platform_change(override_active, theme.brightness, platform);
}

/// The reduced-motion **floor** rule: the OS's accessibility preference
/// (`UIAccessibility.isReduceMotionEnabled`, reported through
/// `frust_set_reduce_motion`) is OR'd over the active theme's own authored
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
/// touch/IME handler exactly as it does from `Component::build`.
///
/// Every path that reaches [`AppTree::event`](frust_shell_common::AppTree::event)
/// — touch dispatch (raw and frame-resampled), `ime_apply`, and a queued
/// accessibility action — routes through this. Without it `Owner::current()` is
/// `None` for the whole pass (`Owner::with` restores the previous owner when the
/// rebuild wrap returns), so a handler's `use_context::<Theme>()` silently
/// resolves to `None`.
///
/// Three deliberate properties, mirrored in the Android shell and pinned by the
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
///   unwrapped if the runtime is somehow absent — this path is reached across the
///   C-ABI boundary, where an unwind is undefined behavior.
fn under_root_owner<R>(pass: impl FnOnce() -> R) -> R {
    match ReactiveRuntime::get() {
        Some(rt) => rt.with_owner(pass),
        None => pass(),
    }
}

/// Map one differ [`ViewCommand`] onto the host-testable
/// [`crate::ffi_support::PlatformViewCommand`] shape, converting its logical,
/// absolute-window rect/clip into physical px (`* scale`) — the one place
/// this crate crosses from `frust_shell_common`'s `kurbo::Rect`-typed
/// vocabulary into the FFI-boundary-safe plain-`f64` one (see
/// [`crate::ffi_support::PvRect`]'s doc comment for why `kurbo` can't appear
/// in `ffi_support` itself).
fn to_pv_command(cmd: &ViewCommand, scale: f64) -> crate::ffi_support::PlatformViewCommand {
    fn to_pv_rect(rect: kurbo::Rect, scale: f64) -> crate::ffi_support::PvRect {
        crate::ffi_support::PvRect {
            x: rect.x0 * scale,
            y: rect.y0 * scale,
            w: rect.width() * scale,
            h: rect.height() * scale,
        }
    }
    match cmd {
        ViewCommand::Create {
            slot_id,
            view_type,
            params_json,
            interactive,
        } => crate::ffi_support::PlatformViewCommand::Create {
            slot_id: *slot_id,
            view_type: view_type.clone(),
            params_json: params_json.clone(),
            interactive: *interactive,
        },
        ViewCommand::Update {
            slot_id,
            rect,
            clip,
            visible,
            shields,
        } => crate::ffi_support::PlatformViewCommand::Update {
            slot_id: *slot_id,
            rect: to_pv_rect(*rect, scale),
            clip: clip.map(|c| to_pv_rect(c, scale)),
            visible: *visible,
            shields: shields.iter().map(|s| to_pv_rect(*s, scale)).collect(),
        },
        ViewCommand::UpdateParams {
            slot_id,
            params_json,
        } => crate::ffi_support::PlatformViewCommand::UpdateParams {
            slot_id: *slot_id,
            params_json: params_json.clone(),
        },
        ViewCommand::Dispose { slot_id } => {
            crate::ffi_support::PlatformViewCommand::Dispose { slot_id: *slot_id }
        }
    }
}

impl IosAppHandle {
    /// Assemble a handle with an already-created, `SurfaceReady` renderer.
    ///
    /// Called from [`crate::ffi_glue`] after the surface has been built from the
    /// `CAMetalLayer`; runs the first rebuild so the tree exists before the first
    /// frame. No `unsafe` here — the surface creation happens at the FFI boundary.
    ///
    /// Seeds the base theme ([`base_theme`]: a design system's
    /// `set_default_theme`, else the built-in [`Theme::neutral`] fallback,
    /// carrying that base's own brightness until Swift's follow-up
    /// `frust_set_appearance`
    /// reports the real preference; called synchronously right after
    /// `frust_init` returns, before the display link starts, so a
    /// light-preference device still ends up light before the first
    /// frame is presented) into both delivery paths (`AppTree::set_theme` for
    /// widgets, `provide_context` for app code)
    /// before the first rebuild, mirroring the desktop shell's `apply_theme`.
    /// Must be called under the root reactive `Owner` (see
    /// `crate::ffi_glue::create_handle`) so `provide_context` isn't a silent
    /// no-op.
    ///
    /// `text_ctx` is the [`TextContext`] `create_handle` already resolved
    /// — the pre-built one from its own background font-preload
    /// thread when it finished in time, or a synchronous fallback otherwise —
    /// so this method never itself pays the font-DB load cost.
    ///
    /// `executor` is the render-path half [`crate::ffi_glue::create_handle`]
    /// already built: the render-thread split
    /// ([`FrameExecutor::Split`], with the render thread already spawned and its
    /// initial `SurfaceCreated` sent) or the inline fallback
    /// ([`FrameExecutor::Inline`], with the surface + early startup spans already
    /// created on this UI thread). The initial `rebuild()` below records
    /// [`perf::SPAN_FIRST_REBUILD_DONE`] via [`FrameExecutor::record_first_rebuild`]
    /// — inline records it here, the split records it render-side on the first
    /// handed-off scene.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        executor: FrameExecutor,
        mut text_ctx: TextContext,
        metal_layer: *mut c_void,
        physical: (u32, u32),
        scale: f32,
        surface_alpha: SurfaceAlphaRequest,
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
        // carrying that base's own brightness until Swift's follow-up
        // `frust_set_appearance` overwrites it with the device's real
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
        // the platform-view hole-punch clears each Mode B slot's rect only when
        // the surface really came up translucent, not merely requested. At
        // construction the split's surface may
        // still be installing render-side, so this reads the request-seeded
        // flag; `frame`'s per-frame `sync_translucent_resolved` re-reads it and
        // downgrades within one frame of a fallback.
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
        // `None`. Insets start at zero here — Swift's first `frust_set_insets`
        // arrives after `frust_init` and re-polls this publisher (see
        // `set_insets`). No `ReactiveRuntime::get()` wrap: like the theme seed
        // above, this runs nested inside `create_handle`'s `with_owner`.
        let mut window_metrics = WindowMetricsPublisher::new();
        if let Some(metrics) =
            window_metrics.poll(physical, sanitize_scale(scale), WindowInsets::default())
        {
            provide_context(metrics);
        }
        app.rebuild();
        let mut executor = executor;
        executor.record_first_rebuild();
        // The gate honors the `FRUST_NO_FRAME_GATE` kill switch at
        // construction; seed its resume-warmup so the first frames after this
        // handle is built run unconditionally (the surface just became ready and
        // the first tick's change signals may not be observable yet — the same
        // reason `resize`/`set_surface`/`resume` re-open the window).
        let mut frame_gate = FrameGate::new();
        frame_gate.note_resumed();
        // Present-sync is active only in the split with the host's latch set
        // (see the `present_sync` field doc). Resolved once here rather than
        // re-matched per frame.
        let present_sync = match &executor {
            FrameExecutor::Split(split) => split.present.is_armed(),
            FrameExecutor::Inline(_) => false,
        };
        Self {
            executor,
            text_ctx,
            font_registry,
            scene: Scene::new(),
            app,
            scope: TrackedScope::new(),
            metal_layer,
            physical,
            scale,
            paused: false,
            recreate_failures: 0,
            theme,
            theme_override: ThemeOverrideWatcher::new(),
            theme_override_active: false,
            platform_brightness: Brightness::Light,
            // No OS reduced-motion report yet (Swift pushes one right after
            // `frust_init`, beside `frust_set_appearance`), so the seeded
            // base's own token IS the effective value — `effective_reduce_motion`
            // against a `false` OS latch is the identity, which is why the seed
            // above needs no fix-up.
            os_reduce_motion: false,
            authored_reduce_motion,
            insets: WindowInsets::default(),
            window_metrics,
            // Attached later, on the first layout, via `frust_init_accessibility`
            // once Swift can supply the UIView (see the field doc).
            a11y: None,
            frame_gate,
            events_since_last_frame: false,
            last_needs_frame: false,
            last_needs_frame_paced_only: false,
            appearance_dirty: false,
            resampler: PointerResampler::new(),
            resample_clock: Instant::now(),
            pointer_scratch: Vec::new(),
            last_frame_time_nanos: None,
            deadline_overruns: 0,
            surface_alpha,
            translucent_resolved,
            platform_views: PlatformViewState::new(),
            platform_view_due: FramePairing::new(),
            presented_frame_id: 0,
            present_sync,
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
    ///   recreate (the renderer still describes whatever surface is live).
    /// - **Split** (default): the render thread stored the resolution when it
    ///   installed (or self-healed) the surface; this is a plain atomic load.
    ///
    /// Cheap enough to call every frame: one enum match plus one atomic load,
    /// and [`AppTree::set_surface_translucent`] is no-op-if-unchanged (marking
    /// `ChangeFlags::PAINT` only on an actual flip — which is what makes a
    /// downgrade repaint without the punch).
    ///
    /// Also this shell's single publish point for the app-facing RESOLVED slot
    /// (`frust::resolved_surface_mode()`), mirroring Android: app
    /// code polls that slot during rebuild, so it must be current *before* the
    /// rebuild this frame leads into, and publishing here (rather than at the
    /// install/self-heal sites) keeps one UI-thread beat as the source for both
    /// the render root and the app. One uncontended `Mutex` store per frame.
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

    /// Store the accesskit adapter for the app's `FrustView`.
    ///
    /// Called once from [`crate::ffi_glue::init_accessibility`] on the first
    /// layout, after `frust_init` returned this handle. The `unsafe`
    /// construction of the [`IosA11yAdapter`] (dynamically subclassing the view to
    /// implement the UIKit accessibility methods) happens at the FFI boundary in
    /// `ffi_glue` — the sanctioned zone for raw-pointer work — so this method is a
    /// plain, safe store: it just takes the already-constructed adapter. From the
    /// next frame on, `frame()` pushes semantics to it and routes its queued
    /// actions.
    pub(crate) fn attach_accessibility(&mut self, adapter: IosA11yAdapter) {
        self.a11y = Some(adapter);
    }

    /// `frust_set_appearance`: flip the theme's brightness and re-push it to
    /// both delivery paths (mirrors the desktop shell's `apply_theme`).
    ///
    /// Runs the `provide_context` re-provide under the process-wide root
    /// [`ReactiveRuntime`]'s owner (fetched fresh here, since — unlike
    /// [`Self::new`] — this call arrives on its own C-ABI entry, not nested
    /// inside `create_handle`'s `with_owner` wrap). No explicit redraw is
    /// scheduled — the continuous `CADisplayLink` loop already repaints every
    /// tick.
    ///
    /// Override-wins rule: while an app-forced theme override is
    /// active, this platform-appearance report must not flip brightness (see
    /// `effective_brightness_for_platform_change`). A seeded *default* is
    /// deliberately not pinned that way: [`Self::theme`] still IS that base
    /// ([`base_theme`] seeded it and nothing replaced it), so flipping its
    /// brightness in place re-derives light/dark from the design system's own
    /// theme.
    pub(crate) fn set_appearance(&mut self, dark: bool) {
        let platform = match crate::ffi_support::appearance_from_dark(dark) {
            crate::ffi_support::Appearance::Dark => Brightness::Dark,
            crate::ffi_support::Appearance::Light => Brightness::Light,
        };
        self.platform_brightness = platform;
        follow_platform_brightness(&mut self.theme, self.theme_override_active, platform);
        self.push_theme();
        // Frame-gate latch: an OS appearance flip must force the next ready
        // frame to Run (see the `appearance_dirty` field doc).
        self.appearance_dirty = true;
    }

    /// `frust_set_reduce_motion`: apply the platform's reduced-motion
    /// accessibility preference to the active theme's `MotionScheme` and
    /// re-push it to both delivery paths — the reduced-motion twin of
    /// [`Self::set_appearance`], travelling the identical transport (a C-ABI
    /// entry → a `Theme` edit → [`Self::push_theme`]) over a different sensor.
    ///
    /// `reduce` is Swift's `UIAccessibility.isReduceMotionEnabled` read; it is
    /// not a `UITraitCollection` trait, so Swift sources it from
    /// `UIAccessibility.reduceMotionStatusDidChangeNotification` (plus a
    /// re-read on foreground) rather than `traitCollectionDidChange`.
    ///
    /// The OS value is a floor over the theme's own authored token, not a
    /// replacement ([`effective_reduce_motion`]) — including while an app-forced
    /// override is active, unlike the brightness path's override-wins rule
    /// (that helper's doc has the reasoning). No explicit redraw is scheduled:
    /// the continuous `CADisplayLink` loop already repaints every tick, and
    /// `appearance_dirty` keeps the frame gate from skipping the tick that
    /// carries the change (and survives a toggle made while backgrounded — the
    /// latch is only taken past the pause/ready gate).
    ///
    /// Unchanged-value calls return early, unlike [`Self::set_appearance`]:
    /// Swift re-pushes this on every foreground (and the notification can fire
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
        self.push_theme();
        self.appearance_dirty = true;
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

    /// `frust_set_insets`: push the platform's per-edge insets onto the render
    /// root. Mirrors the Android shell's `set_insets` but
    /// with an **identity scale**: UIKit's `safeAreaInsets` and keyboard frame are
    /// already in **logical points** (the same space `dispatch_touch` passes
    /// through with no scale division — the documented iOS/Android coordinate
    /// asymmetry), so this uses `logical_insets(.., 1.0)` rather than dividing by
    /// the display scale like Android's physical-px path.
    ///
    /// `logical` is the eight-value pack [`logical_insets`] expects (`view_padding`
    /// then `view_insets`, each l/t/r/b — Swift assembles `view_padding` from
    /// `safeAreaInsets` and `view_insets` from the keyboard frame). No-op-guarded
    /// on `PartialEq`: a re-report of unchanged insets neither relayouts nor
    /// re-provides. On a real change `RenderRoot::set_insets` marks `LAYOUT |
    /// PAINT` pending, which the frame gate already treats as dirty —
    /// no new gate input needed. The continuous `CADisplayLink` loop repaints the
    /// next tick with no extra wake.
    pub(crate) fn set_insets(&mut self, logical: [f64; 8]) {
        // iOS insets are already logical points (see the method doc): identity
        // scale, unlike Android's device-px `sanitize_scale(self.scale)` divisor.
        let insets = logical_insets(logical, 1.0);
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
    ///   [`Self::frame`]: `frust_resize` → [`Self::resize`] /
    ///   [`Self::set_surface`] (drawable size + scale) and `frust_set_insets` →
    ///   [`Self::set_insets`] (safe area / keyboard). `frame` only reads what
    ///   those already stored.
    ///
    /// Units: the size is converted from the drawable's physical px with the
    /// same `sanitize_scale`d display scale layout uses, and `self.insets` is
    /// already logical (UIKit hands over points, which
    /// [`Self::set_insets`] passes through `logical_insets(.., 1.0)`) — so the
    /// published metrics are logical throughout, matching Android and desktop.
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

    /// The retained Swift-owned `CAMetalLayer*` this handle's surface was built
    /// from, used by [`crate::ffi_glue`] to recreate the surface after a
    /// `SurfaceLost` in the inline path. Returns the raw pointer by value (no
    /// borrow) so the FFI layer can read it before taking a `&mut` via
    /// [`inline_renderer_mut`](Self::inline_renderer_mut); the actual `unsafe`
    /// surface creation stays confined to `ffi_glue`. (In the split the render
    /// thread keeps its own copy of the pointer and self-heals render-side.)
    pub(crate) fn metal_layer(&self) -> *mut c_void {
        self.metal_layer
    }

    /// The surface-alpha request this handle's surface was created with,
    /// read by [`crate::ffi_glue::recover_surface`]
    /// so an inline surface recreate requests the same alpha mode the initial
    /// surface did (see the field doc).
    pub(crate) fn surface_alpha(&self) -> SurfaceAlphaRequest {
        self.surface_alpha
    }

    /// Re-emit `Create`+`Update` for every currently-live platform-view slot,
    /// for the inline path's successful surface
    /// recreate ([`crate::ffi_glue::recover_surface`]) to call. Delegates to
    /// [`PlatformViewState::reset_for_surface_recreate`].
    pub(crate) fn reset_platform_views_for_surface_recreate(&mut self) {
        self.platform_views.reset_for_surface_recreate();
        // The replay supersedes every held batch, and the frames those batches
        // were paired with belong to the surface that just went away — so the
        // pairing goes with it.
        self.platform_view_due.clear();
    }

    /// `frust_platform_view_commands_json`'s core: acknowledge `ack_generation`
    /// (compacting the differ's backlog), then
    /// serialize whatever remains into the wire JSON both mobile shells'
    /// peek getters return verbatim (`frust-shell-android`'s
    /// `nativePlatformViewCommands` shares the byte-identical
    /// schema). `None` on the no-change fast path.
    ///
    /// Converts each command's logical, absolute-window rect/clip into
    /// **physical** px (`* scale`) at this FFI boundary — the
    /// physical-at-FFI/logical-inside rule, applied outbound (matches every
    /// other outbound-geometry seam in this shell).
    pub(crate) fn platform_view_commands_json(&mut self, ack_generation: u64) -> Option<String> {
        self.platform_views.acknowledge(ack_generation);
        // Keep the gate's bookkeeping in step with the differ's: a batch
        // the host has applied needs no pairing.
        self.platform_view_due.acknowledge(ack_generation);
        // Under present-sync, serve only the prefix whose producing frame is on
        // screen — with the present issued from this thread, "on screen" means
        // "presented in this very transaction" (see `platform_view_due`).
        // Otherwise serve the whole backlog, byte-for-byte as before.
        let (generation, commands) = if self.present_sync {
            let releasable = self
                .platform_view_due
                .releasable_generation(self.presented_frame_id, self.executor.submitted_frame_id());
            self.platform_views.commands_up_to(releasable)
        } else {
            self.platform_views.commands()
        };
        let scale = sanitize_scale(self.scale) as f64;
        let mapped: Vec<crate::ffi_support::PlatformViewCommand> = commands
            .iter()
            .map(|cmd| to_pv_command(cmd, scale))
            .collect();
        crate::ffi_support::platform_view_commands_json(generation, ack_generation, &mapped)
    }

    /// Whether the render-thread split is engaged (as opposed to the inline
    /// fallback) — the FFI layer branches surface *recovery* on this: the split
    /// self-heals a lost surface render-side (the render loop recreates from the
    /// retained layer pointer), so the UI-side `should_recreate_surface` recovery
    /// in `frust_render_frame`/`frust_resize` applies only to the inline path.
    pub(crate) fn executor_is_split(&self) -> bool {
        matches!(self.executor, FrameExecutor::Split(_))
    }

    /// Whether the render thread signalled a fatal, unrecoverable first-surface
    /// install failure, read by
    /// [`crate::ffi_glue::render_frame`] to tell Swift to latch `initFailed` and
    /// invalidate its `CADisplayLink`. The split reads its shared `fatal` flag;
    /// the inline fallback never faults here — a failed first install returns
    /// `Err` from `create_handle`, so `frust_init` yields a null handle and no
    /// frame is ever driven — so it is always `false`.
    pub(crate) fn render_fatal(&self) -> bool {
        match &self.executor {
            FrameExecutor::Split(split) => split.fatal.load(Ordering::Acquire),
            FrameExecutor::Inline(_) => false,
        }
    }

    /// Access to the inline render context + renderer for the FFI layer to drive an
    /// `unsafe` surface *recreation* (the one lifecycle transition that crosses the
    /// raw-pointer boundary). `None` in the render-thread split — there the render
    /// thread owns the renderer and recovery is render-side (see
    /// [`crate::ffi_glue::render_loop`]).
    pub(crate) fn inline_renderer_mut(
        &mut self,
    ) -> Option<(&mut RenderContext, &mut SurfaceRenderer)> {
        match &mut self.executor {
            FrameExecutor::Inline(inline) => Some((&mut inline.render_cx, &mut inline.renderer)),
            FrameExecutor::Split(_) => None,
        }
    }

    /// The current surface's physical (pixel) size, used by [`crate::ffi_glue`] to
    /// recreate a lost surface at its last-known dimensions on a `render_frame`
    /// (a `resize` supplies fresh dimensions instead).
    pub(crate) fn physical(&self) -> (u32, u32) {
        self.physical
    }

    /// The current display scale, retained across a surface recreation on a
    /// `render_frame` (see [`physical`](Self::physical)).
    pub(crate) fn scale(&self) -> f32 {
        self.scale
    }

    /// Record the physical size + scale backing a freshly recreated surface.
    ///
    /// The `metal_layer` never changes (iOS keeps the same layer for the app's
    /// lifetime), so — unlike Android's `set_window` — there is no window to swap:
    /// this only refreshes the layout inputs after a successful recreate in
    /// [`crate::ffi_glue`]. Safe: touches no raw pointers.
    pub(crate) fn set_surface(&mut self, physical: (u32, u32), scale: f32) {
        self.physical = physical;
        self.scale = scale;
        // A successful recreate ends the SurfaceLost episode; future losses get a
        // fresh retry budget.
        self.recreate_failures = 0;
        // A surface recreation forces the frame gate's resume-warmup: the first
        // ticks against the fresh surface must run even before their change
        // signals are observable.
        self.frame_gate.note_resumed();
        // The recreate may land at new dimensions/scale — republish the
        // window-shape context (self-guarded, so a same-size recreate is silent).
        self.push_window_metrics();
        // The recreated surface re-resolved its alpha mode from scratch (review
        // M1) — on this (inline) path the renderer on this thread already holds
        // it, so a recreate that fell back to opaque degrades to Mode A right
        // here rather than punching black holes for the rest of the process.
        self.sync_translucent_resolved();
    }

    /// Resize the live surface in place (rotation / bounds change). The
    /// `CAMetalLayer` survives an iOS rotation, so this is always a plain resize —
    /// no recreate path is needed (contrast the Android shell, where a new
    /// `Surface` object forces surface recreation). Safe: no raw pointers — inline
    /// reconfigures the renderer's swapchain directly; the split routes the
    /// layer-owned in-place resize as a [`RenderCommand::SurfaceChanged`] command.
    pub(crate) fn resize(&mut self, physical: (u32, u32), scale: f32) {
        match &mut self.executor {
            FrameExecutor::Inline(inline) => {
                inline
                    .renderer
                    .on_surface_changed(&inline.render_cx, physical.0, physical.1)
            }
            FrameExecutor::Split(split) => split.resize(SurfaceSize {
                width: physical.0,
                height: physical.1,
                scale: scale as f64,
            }),
        }
        self.physical = physical;
        self.scale = scale;
        // A resize (rotation / bounds change) forces the frame gate's
        // resume-warmup so the next frames re-layout/paint at the new size even
        // if no other change signal fires.
        self.frame_gate.note_resumed();
        // New logical size and/or scale: republish the window-shape context —
        // this is the path a device rotation takes, so it is what flips the
        // derived `Orientation` for app code (self-guarded, so a `frust_resize`
        // re-reporting identical dimensions publishes nothing).
        self.push_window_metrics();
    }

    /// Mark the app paused (`frust_pause`): subsequent `frame()`s are no-ops.
    ///
    /// In the render-thread split this **barriers on the render thread's ack
    /// before returning**: it sends a `Pause` command
    /// and blocks until the render thread has quiesced (moved to `Paused`, dropped
    /// any leftover scene, and acked). Only then does this return, so the app
    /// never backgrounds while the render thread might still submit Metal work —
    /// which can get a suspended process killed. The `self.paused` flag is the
    /// UI-side belt-and-suspenders (it also no-ops `frame()`); the ack is the real
    /// cross-thread guarantee. Set `paused` first so a `frame()` racing in cannot
    /// hand off a new scene while the barrier is in flight.
    pub(crate) fn pause(&mut self) {
        self.paused = true;
        // Backgrounding path: hide every live
        // platform-view slot immediately rather than waiting out the
        // ordinary missing-streak Hide (paint doesn't run while paused, so
        // `ingest` never drives that path — see `PlatformViewState::suspend_all`).
        self.platform_views.suspend_all();
        // While backgrounded nothing is painted or presented, so a pairing
        // recorded before the pause would hold this hide behind a frame that
        // never lands. The gate is a smoothing device, not a correctness
        // barrier — drop it so the hide goes out on the next poll (the Android
        // shell's `suspend_platform_views` does the same).
        self.platform_view_due.clear();
        if let FrameExecutor::Split(split) = &mut self.executor {
            split.pause_barrier();
            // Present-sync: the barrier has returned, so the render
            // thread has quiesced and can park nothing more. Drop whatever it
            // parked last — a backgrounded app must issue no Metal work (the
            // same rule the barrier itself exists for), and the next foreground
            // frame produces a fresh drawable anyway.
            split.present.clear();
        }
    }

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

    /// Mark the app resumed (`frust_resume`): `frame()`s do work again. In the
    /// split, sends a fire-and-forget `Resume` so the render thread leaves `Paused`
    /// and resumes submitting.
    pub(crate) fn resume(&mut self) {
        self.paused = false;
        if let FrameExecutor::Split(split) = &mut self.executor {
            split.resume();
        }
        // Re-open the frame gate's resume-warmup: the first frames after
        // foregrounding must run unconditionally (a backgrounded app's change
        // signals may have been coalesced away).
        self.frame_gate.note_resumed();
    }

    /// Whether the app is paused. `pub(crate)` so [`crate::ffi_glue`] can gate
    /// surface recreation on it — recreation is real Metal work and must obey the
    /// same backgrounded-no-GPU rule as frame submission.
    pub(crate) fn paused(&self) -> bool {
        self.paused
    }

    /// Consecutive failed recreate attempts this `SurfaceLost` episode (see the
    /// field doc).
    pub(crate) fn recreate_failures(&self) -> u8 {
        self.recreate_failures
    }

    /// Record one failed surface-recreate attempt (saturating; the predicate cap
    /// makes values past `MAX_RECREATE_ATTEMPTS` unreachable in practice).
    pub(crate) fn record_recreate_failure(&mut self) {
        self.recreate_failures = self.recreate_failures.saturating_add(1);
    }

    /// The inline renderer's current lifecycle phase, or `None` in the
    /// split (the render thread owns the phase). `pub(crate)` so [`crate::ffi_glue`]
    /// can gate the inline path's surface recreation on a `SurfaceLost` phase — the
    /// split self-heals render-side, so this returning `None` is exactly the "no
    /// UI-side recovery" signal there.
    pub(crate) fn inline_phase(&self) -> Option<SurfacePhase> {
        match &self.executor {
            FrameExecutor::Inline(inline) => Some(inline.renderer.phase()),
            FrameExecutor::Split(_) => None,
        }
    }

    /// Deliver one touch contact to the tree.
    ///
    /// **Coordinate asymmetry vs Android:** UIKit's `touch.location(in:)` is
    /// already in **logical points**, so — unlike the Android shell, which
    /// receives physical pixels and divides by the display density — this path
    /// passes `x`/`y` straight through with no scale division. First-touch only
    /// in v1: the Swift side forwards a single contact as
    /// [`PointerButton::Primary`]. The redraw is implicit — the `CADisplayLink`
    /// loop posts a frame every vsync, so the mutated state is picked up on the
    /// next `frame()` without an explicit schedule (contrast the desktop shell's
    /// `request_redraw`).
    pub(crate) fn dispatch_touch(&mut self, phase: TouchPhase, x: f32, y: f32) {
        let position = Point::new(x as f64, y as f64);
        let core_phase = match phase {
            TouchPhase::Began => PointerPhase::Down,
            TouchPhase::Moved => PointerPhase::Move,
            TouchPhase::Ended => PointerPhase::Up,
            TouchPhase::Cancelled => PointerPhase::Cancel,
        };
        // Latch for the frame gate: a touch between frames must force the next
        // frame to run so the mutated state is reflected.
        self.events_since_last_frame = true;

        // Pointer resampling: buffer the raw sample (stamped
        // on the shared resample clock) for [`Self::frame`] to emit a
        // frame-boundary-interpolated position; Down/Up/Cancel still pass
        // through losslessly. When the kill switch disabled the resampler,
        // deliver directly instead — pre-resampling behavior verbatim.
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

    /// Push a whole editing state from the platform IME mirror into the focused
    /// widget (the mobile state-sync path). Delegates to
    /// [`AppTree::ime_apply`]; the `EditingState`'s selection/composing indices are
    /// UTF-16 code units (converted to byte offsets by the widget/`frust-text`).
    /// The `needs_redraw` in the returned outcome is implicit here — the
    /// `CADisplayLink` loop already ticks the next frame every vsync — so it is
    /// dropped (mirror of [`Self::dispatch_touch`]).
    pub(crate) fn ime_apply(&mut self, state: EditingState) {
        let app = &mut self.app;
        let _ = under_root_owner(|| app.ime_apply(state));
        // Latch for the frame gate: an IME edit between frames must force the
        // next frame to run (mirror of [`Self::dispatch_touch`]).
        self.events_since_last_frame = true;
    }

    /// The IME surface the focused widget published (editing state + caret), for
    /// the FFI layer to serialize back to the Swift `UITextInput` bridge.
    /// Delegates to [`AppTree::ime_state`]; `None` when nothing is focused.
    pub(crate) fn ime_state(&self) -> Option<ImeState> {
        self.app.ime_state()
    }

    /// Publish the current accessibility tree to the iOS accesskit adapter, if it
    /// changed since the last push (generation-gated).
    /// Must run **after** [`Self::frame`]'s layout so node bounds are valid.
    ///
    /// Gated on the semantics generation exactly like the desktop/Android
    /// adapters (see `docs/CODE_STANDARDS.md`'s Semantics Conventions): the
    /// [`AppTree::semantics_if_changed`] check skips reassembling+re-pushing an
    /// unchanged tree, so a static screen pays no per-frame semantics cost — this
    /// closes the iOS adapter's former "recompute-on-every-active-frame" fallback.
    /// The adapter's own `update_if_active` is a second, finer gate: it pushes
    /// only while an assistive technology is active. The assembled tree is also
    /// snapshotted inside [`IosA11yAdapter::publish`] so a late-activating AT
    /// (one that connects on a static screen this gate would otherwise skip) is
    /// served real content immediately — the Android adapter's `tree_snapshot`
    /// design.
    fn publish_semantics(&mut self) {
        // Cheap generation gate first (immutable `a11y` borrow, released before
        // the `&mut self.app` call below), mirroring the Android shell's
        // `publish_semantics`.
        let last_gen = match self.a11y.as_ref() {
            Some(a11y) => a11y.last_pushed_gen(),
            None => return,
        };
        let Some(update) = self.app.semantics_if_changed(last_gen) else {
            return; // tree unchanged since the last push
        };
        let current_gen = self.app.semantics_generation();
        if let Some(a11y) = self.a11y.as_mut() {
            a11y.publish(update, current_gen);
        }
    }

    /// Run one frame: rebuild → layout → paint → render, mirroring the desktop
    /// shell's `RedrawRequested` path but driven by the Swift
    /// `CADisplayLink`.
    ///
    /// `timestamp_ns` is the `CADisplayLink` tick's `timestamp`
    /// (`CFTimeInterval` seconds) converted to nanoseconds by the Swift caller
    /// — the shell-owned monotonic clock threaded into [`FrameTime`]
    /// (`frust-core` never reads a clock itself).
    ///
    /// A no-op unless the surface is ready *and* the app is not paused (see
    /// [`crate::ffi_support::should_render_frame`]). Readiness comes from the
    /// executor: inline reads the renderer's phase, the split reads its UI-side
    /// `surface_active` mirror. On `FrameOutcome::SurfaceLost` (surfaced
    /// render-side in the split, or from the inline tail) the surface is dropped;
    /// recovery recreates it from the retained `metal_layer` — render-side on the
    /// same wakeup in the split, or on the next `frust_render_frame`/`frust_resize`
    /// FFI entry in the inline path (see [`crate::ffi_glue`]).
    ///
    /// The render tail is off this thread in the split — the first-presented-frame
    /// startup span is recorded render-side inside [`render_scene`] (the render
    /// thread is the single perf emitter), so this method no longer returns
    /// anything.
    pub(crate) fn frame(&mut self, timestamp_ns: u64) {
        // Pump the UI-thread reactive local-task queue BEFORE the ready/paused
        // gate below: placed after it, queued `spawn_local` completions (e.g. a
        // signal write scheduled from a background task) would stall for as
        // long as the surface stays not-ready/paused instead of draining as
        // soon as the CADisplayLink ticks. A no-op
        // until `frust_init` has installed the runtime.
        if let Some(rt) = ReactiveRuntime::get() {
            rt.pump_local();
        }

        // Poll the app-facing theme override slot once per
        // frame, before the ready/paused gate — theme delivery needs no
        // renderer, so this stays in sync even while backgrounded/not-ready
        // (mirroring the reactive-runtime pump just above). Whether it changed is
        // also a frame-gate input (`theme_or_appearance_changed`) captured here.
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
        // beside the theme poll above and before the pause/ready gate — the drain
        // needs no renderer, so it stays in sync while backgrounded/not-ready.
        // `drain_into` applies any late-registered fonts to `text_ctx` (clearing
        // the shape cache internally) and returns whether anything registered. On
        // a late drain, force the relayout `register_fonts` documents by
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

        // Pause/ready gate FIRST — a paused/not-ready frame does no work and the
        // frame gate is never even consulted (the existing early return
        // stays first). Returning here also leaves the signals-dirty flag
        // undrained (it is only `take`n past this gate below), so a tracked-signal
        // write that lands while backgrounded is observed by the first frame
        // after resume rather than being silently consumed on a no-op tick.
        let ready = self.executor.has_surface();
        if !crate::ffi_support::should_render_frame(ready, self.paused) {
            return;
        }

        // Resolved-translucency sync, before the gate
        // inputs are gathered below: a render-thread fallback-to-opaque (or a
        // self-healed recreate that resolved differently) flips
        // `RenderRoot::set_surface_translucent` to `false`, which marks
        // `ChangeFlags::PAINT` and so forces THIS frame to run
        // (`change_flags_pending`) and repaint without the hole punch. The
        // returned value also drives the base color below, so the clear color
        // and the punch contract can never disagree.
        let translucent_resolved = self.sync_translucent_resolved();

        // Drain any queued accessibility actions (VoiceOver activations, etc.)
        // BEFORE the rebuild below, so a state change an action makes is picked up
        // by this very frame — the same "mutate now, rebuild next" model touch/IME
        // input uses. The handler enqueued these on the
        // main thread; draining takes ownership of the batch so the queue's borrow
        // is dropped before `perform_accessibility_action` re-enters the tree.
        // Whether any action ran is a frame-gate input (`a11y_action_performed`).
        let mut a11y_action_performed = false;
        if let Some(a11y) = self.a11y.as_ref() {
            let actions = a11y.drain_actions();
            a11y_action_performed = !actions.is_empty();
            // An action is routed through synthesized pointer events, landing in
            // the very same handlers a real tap would — so the whole batch runs
            // under the reactive root owner (see [`under_root_owner`]).
            let app = &mut self.app;
            under_root_owner(|| {
                for req in actions {
                    // `target_node.0` is the raw accesskit id the adapter
                    // reported; an unknown node or unmodelled action is a benign
                    // no-op (see `RenderRoot::perform_accessibility_action`).
                    // `needs_redraw` is dropped — the CADisplayLink loop already
                    // ticks the next frame.
                    let _ = app.perform_accessibility_action(req.target_node.0, req.action);
                }
            });
        }

        // Render-side surface self-heal, taken past the
        // ready/paused gate like the drains below so a signal can never be
        // consumed on a no-op tick: the render thread recreated the surface from
        // the retained `CAMetalLayer` on its own (nothing external re-drives
        // creation on iOS — see `SplitExecutor::surface_reinstalled`), so this
        // frame must actually run and repaint the fresh swapchain. Replay every
        // live platform-view slot too, mirroring the inline path's
        // `recover_surface`: the replay supersedes any batch still held by the
        // present-sync release gate and clears the pairing, so geometry paired
        // with the frame that was lost cannot strand now that a resting screen
        // produces no further frames to release it.
        let surface_reinstalled = self.executor.take_surface_reinstalled();
        if surface_reinstalled {
            self.reset_platform_views_for_surface_recreate();
        }

        // Gather the OR-list of "something changed" signals and let
        // the frame gate decide whether this frame runs — mirrors the Android
        // shell's own frame-gate wiring. `signals_dirty` is
        // drained AFTER the pump above (the pump-first ordering contract — see
        // `ReactiveRuntime::take_signals_dirty`) and only now that we are past the
        // ready/paused gate, so a no-op tick never consumes it. The gate honors
        // the `FRUST_NO_FRAME_GATE` kill switch internally (always `Run` when
        // disabled). Correctness over savings: every input defaults toward "run".
        let signals_dirty = ReactiveRuntime::get().is_some_and(|rt| rt.take_signals_dirty());
        let inputs = FrameInputs {
            signals_dirty,
            // Pending buffered pointer samples (too new for this tick's instant)
            // keep frames running until drained — the resampler's pending signal
            // ORs into the events input (the "never starves the
            // gate" contract; default-to-run rule).
            events_since_last_frame: self.events_since_last_frame || self.resampler.has_pending(),
            // Both read straight from the retained tree's `RenderRoot` state: a
            // mid-drag gesture or a focused/IME-active field must keep painting.
            // `ime_state().is_some()` is OR'd in as belt-and-braces, exactly as
            // on Android (the two frame() bodies stay input-for-input
            // comparable): a published IME surface must keep frames running
            // even if the focus path and the published surface ever disagree
            // for a frame (they converge one event pass later by contract).
            pointer_capture_active: self.app.is_pointer_captured(),
            focus_or_ime_active: self.app.is_focus_active() || self.app.ime_state().is_some(),
            last_needs_frame: self.last_needs_frame,
            last_needs_frame_paced_only: self.last_needs_frame_paced_only,
            // Non-draining peek: a skipped frame leaves the flags for the next
            // frame that runs to drain.
            change_flags_pending: self.app.has_pending_change_flags(),
            // The `appearance_dirty` latch (set by `set_appearance`) is taken
            // only past the pause/ready gate — like the signals-dirty drain —
            // mirroring the Android shell input-for-input.
            theme_or_appearance_changed: theme_or_appearance_changed
                || std::mem::take(&mut self.appearance_dirty),
            // A UI-driven surface (re)creation/resize is folded into the gate's
            // resume-warmup via `note_resumed` (see `resize`/`set_surface`/
            // `resume`), so it needs no per-frame latch. What DOES need one is
            // the split's render-side self-heal, which never passes through a
            // UI-thread entry point at all (see the self-heal handling above).
            surface_changed_or_resized: surface_reinstalled,
            a11y_action_performed,
            // Driven by the gate's own warmup countdown (`note_resumed`).
            resumed_recently: false,
        };
        // The events latch has now been read into this frame's decision; reset it
        // so the next frame only sees events that arrive from here on.
        self.events_since_last_frame = false;

        // Deadline-aware pacing: estimate this frame's target
        // budget from the tick-to-tick delta, updating the stored tick every
        // frame (skip or run) so the estimate reflects one refresh interval
        // rather than a gap across skipped ticks.
        let frame_interval = resample::frame_interval_nanos(
            self.last_frame_time_nanos.replace(timestamp_ns),
            timestamp_ns,
        );

        // Animation pacing (frame-gate pacing): a frame whose ONLY dirtiness is
        // a paced (CosmeticLoop) request is throttled to the active theme's
        // `cosmetic_loop_rate` rather than reproduced every `CADisplayLink`
        // tick. `now` is this tick's display-link clock (the same domain `paint`
        // consumes below); the interval is `1 / rate` resolved from the live
        // theme so a retuned token re-paces live. Every other FrameInputs signal
        // still forces an immediate Run — pacing never delays real work.
        let pacing = FramePacing {
            now: FrameTime::from_nanos(timestamp_ns),
            interval: Duration::from_secs_f32(1.0 / self.theme.motion.cosmetic_loop_rate.hz()),
        };

        if self.frame_gate.decide_paced(inputs, pacing).is_skip() {
            // Nothing changed: skip rebuild/layout/paint/encode entirely. Inline
            // records a `skipped` FramePasses (ZERO pass durations; counts toward
            // `skipped=` in the perf log line); the render-thread split sends
            // **nothing** across the channel on a skip (the render thread is the
            // single emitter and never sees skipped frames — split mode records no
            // skip frames, a known limitation), so `record_skip` is a no-op
            // there. Either way the CADisplayLink keeps ticking — only frame
            // *production* stops, callbacks don't (the accepted v1 shape, same as
            // Android — see `docs/DEVELOPMENT.md`).
            self.executor.record_skip();
            return;
        }

        // Perf instrumentation: read the cached
        // switch exactly once per frame and gate every `Instant::now()` read
        // below behind it — a disabled build takes zero clock reads on this
        // path, not merely a no-op record (`FrameStats::record` itself is
        // also a no-op when disabled, but the timer reads this guard skips
        // are the actual hot-path cost).
        let perf_on = perf::enabled();

        // Pointer resampling: drain buffered samples up to
        // this frame's sample instant and feed the interpolated events into the
        // tree BEFORE the rebuild, so the rebuild reflects this frame's resampled
        // input (same `resample_clock` domain the raw samples were stamped in). A
        // no-op when disabled (touches went straight through in `dispatch_touch`).
        // The scratch buffer and the tree are borrowed as disjoint fields so the
        // batch can be iterated in place while dispatching.
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
        // the frame path must never panic across the C-ABI boundary.
        let rebuild_start = perf_on.then(Instant::now);
        match ReactiveRuntime::get() {
            Some(rt) => {
                let scope = &self.scope;
                let app = &mut self.app;
                rt.with_owner(|| scope.track(|| app.rebuild()));
            }
            None => self.app.rebuild(),
        }
        let rebuild = rebuild_start.map(|t| t.elapsed()).unwrap_or_default();

        // Prompt teardown retire: the rebuild just above
        // is where a removed `platform_view` widget's `View::teardown` runs and
        // reports its slot id. Drain those and dispose each native view right
        // now, instead of waiting out the differ's ~30-frame missing-streak
        // heuristic (which cannot tell a torn-down slot from a culled one). A
        // merely culled slot reports nothing here, so the streak still covers
        // it — that asymmetry is the camera keep-alive contract. Mirrors the
        // Android shell's placement exactly; the resulting batch is a lifecycle
        // command, deliberately NOT paired with a frame (like `suspend_all`), so
        // the present-sync release gate never holds it.
        for slot_id in self.app.take_retired_platform_views() {
            self.platform_views.retire(slot_id);
        }

        // Change-flag DRAIN — the fix for "the iOS frame
        // gate never idles at rest".
        //
        // `FrameInputs::change_flags_pending` above reads
        // `has_pending_change_flags()`, a deliberately NON-draining peek, so a
        // frame the gate SKIPS leaves the dirtiness for the next frame that runs.
        // Draining is the running frame's job — and this
        // shell never did it: `RenderRoot::pending` is only ever cleared by
        // `take_change_flags`, so from the very first frame on (construction's
        // `push_theme` marks LAYOUT|PAINT, and the first rebuild marks
        // LAYOUT|PAINT again) that input latched `true` forever and every
        // `CADisplayLink` tick therefore decided `Run`. Measured on an iPhone SE
        // before this drain: ~44-59 fps of full pipeline work on a screen where
        // nothing changes, `skipped=0` on every raw line, against Android's zero
        // frames in 60 s on the same page.
        //
        // Android drains at exactly this point in its own frame body, as the
        // input to its layout-skip seam (`take_change_flags().needs_layout()`),
        // which is why its gate does idle. iOS still relayouts every `Run`
        // unconditionally (the intra-frame layout skip is not wired here — see
        // `docs/ARCHITECTURE.md`'s iOS frame pipeline), so the drained flags are
        // deliberately dropped rather than gating the layout call below: this
        // frame runs both passes regardless, so nothing is lost by clearing
        // them. Anything that marks flags between frames (`set_theme`,
        // `set_insets`, `set_surface_translucent`, a rebuild) still forces the
        // next frame to run, exactly as on Android.
        let _drained = self.app.take_change_flags();

        // Sanitize once per frame; layout and the paint transform below MUST
        // consume this identical value (an untrusted `f32` scale from the FFI
        // boundary must never let the two passes disagree — see `sanitize_scale`).
        let scale = sanitize_scale(self.scale);
        let (lw, lh) = logical_size(self.physical.0, self.physical.1, scale);
        let logical = Size::new(lw, lh);
        let layout_start = perf_on.then(Instant::now);
        {
            let text_ctx: &mut dyn Any = &mut self.text_ctx;
            self.app.layout(logical, text_ctx);
        }
        let layout = layout_start.map(|t| t.elapsed()).unwrap_or_default();

        // Push the accessibility tree AFTER layout (node bounds come from the
        // post-layout geometry), generation-gated so an unchanged tree
        // is never re-walked or re-pushed (see [`Self::publish_semantics`]).
        // Not folded into either pass's timing above/below — it is a11y-conditional
        // work orthogonal to the rebuild/layout/paint/encode split.
        self.publish_semantics();

        // Push the render side's presented-frame count so a widget measuring FPS
        // reports the presented rate, not its `CADisplayLink` paint cadence (task
        // 10). A pure observation — `set_presented_frames` marks no ChangeFlags,
        // so a ticking counter never dirties layout NOR feeds the frame gate (the
        // gate decision already ran above and never reads this), keeping a
        // menu-idle screen at zero frames.
        self.app
            .set_presented_frames(self.executor.presented_frames());

        let paint_start = perf_on.then(Instant::now);
        self.scene.reset();
        let paint_outcome = {
            let mut builder = SceneBuilder::new(&mut self.scene);
            // HiDPI: lay out in logical pixels, then scale the
            // whole scene by the device pixel ratio for sharp glyphs.
            builder.push_transform(Affine::scale(scale));
            // Shell-owned frame clock (time enters from the shell, never
            // `Instant::now()` inside `frust-core`) — the `CADisplayLink`
            // timestamp forwarded from Swift.
            let frame_time = FrameTime::from_nanos(timestamp_ns);
            let outcome = self.app.paint(&mut builder, frame_time);
            builder.pop_transform();
            outcome
        };
        let paint = paint_start.map(|t| t.elapsed()).unwrap_or_default();

        // Platform-view differ ingest: feed this RUN
        // frame's published frames into the command backlog
        // `frust_platform_view_commands_json` serves. Only ever called on a
        // frame that actually painted (never on the `Skip` `return` above) —
        // `PlatformViewState::ingest`'s skip-safety contract.
        //
        // Under present-sync, pair whatever the differ produced with the frame
        // that painted it — the one submitted just below, i.e. the submission
        // cursor plus one — so the release gate holds that geometry until this
        // thread presents that frame (the same shape as the
        // Android shell's always-on pairing). Both statements sit
        // behind the gate's early `return`, so a Skip records nothing AND
        // submits nothing: the recorded id can never run ahead of what will
        // actually be sent.
        let produced = self
            .platform_views
            .ingest(self.app.platform_view_frames(), self.app.input_shields());
        if produced && self.present_sync {
            let (generation, _) = self.platform_views.commands();
            self.platform_view_due
                .record(generation, self.executor.submitted_frame_id() + 1);
        }

        // Latch this paint's `needs_frame` continuation signal (the v1
        // animation seam) for the NEXT frame's gate: unlike before — when
        // the continuous CADisplayLink loop let this flag be dropped — the gate
        // would now skip the follow-up frame an in-flight animation/transition
        // needs, so it is fed forward via `FrameInputs::last_needs_frame`.
        self.last_needs_frame = paint_outcome.needs_frame;
        // Latch the aggregated tick-class for the NEXT frame's gate so a
        // paced-only decorative loop can be throttled (see
        // `FrameInputs::last_needs_frame_paced_only`).
        self.last_needs_frame_paced_only = paint_outcome.needs_frame_paced_only;

        // Hand the finished frame to the render-path executor.
        // The inline fallback runs the encode→acquire→submit tail synchronously
        // here (via the shared [`render_scene`]) and returns its encode span; the
        // split moves the painted scene out (replacing `self.scene` with a fresh
        // empty one) into a [`SceneFrame`] and hands it across the channel for the
        // render thread to encode/acquire/present, returning `Duration::ZERO`
        // (encode is off-thread). Either way [`render_scene`] is the single place
        // the folded [`FramePasses`] is recorded, the first-encode/first-frame
        // startup milestones are stamped, and SurfaceLost/Redraw are surfaced —
        // the exact pre-split tail, only relocated. The clear color (the live
        // theme's surface color, not white) rides *with* the scene so a mid-frame
        // theme flip clears correctly.
        let ui = UiSpans {
            rebuild,
            layout,
            paint,
            skipped: false,
        };
        // Mode B translucent base clear: a surface that RESOLVED translucent
        // (`translucent_resolved`,
        // read at the top of this frame — not the request latch) clears to
        // alpha-0 instead of the theme's opaque surface color, so a native
        // sibling view placed behind this one shows through wherever the tree
        // paints nothing (Mode B paint contract — the app must paint every
        // chrome surface explicitly). Opaque —
        // requested-but-refused included — is bit-for-bit today's behavior.
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
        let meta_time = FrameTime::from_nanos(timestamp_ns);
        let encode_time =
            self.executor
                .submit_frame(&mut self.scene, base_color, ui, meta_time, size, perf_on);

        // Deadline-aware pacing overrun: this frame's *work*
        // (everything but the vsync `present` wait, which is expected to block)
        // overrunning the tick-to-tick budget is counted and logged. Gated behind
        // `perf_on` so a non-perf build logs nothing; instrumentation only — no
        // work is dropped on the strength of this. In the render-thread split
        // `encode_time` is zero (encode is off-thread), so `work` reduces to the UI
        // thread's real budget — rebuild+layout+paint — which is exactly what the
        // UI thread is now responsible for hitting.
        if perf_on {
            let work = rebuild + layout + paint + encode_time;
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

/// Unit tests for the default-theme seed ladder.
///
/// This module is inside the `#[cfg(target_os = "ios")]` `app` module, so it
/// only compiles/runs for an iOS target — the ladder references `frust_theme`'s
/// [`Theme`] and `frust_shell_common`, both iOS-gated dependencies of this crate
/// by deliberate design (see `Cargo.toml`), so it cannot be a host test the way
/// [`crate::ffi_support`]'s pure helpers are — worse, the documented iOS
/// compile gate (`cargo check --target aarch64-apple-ios-sim -p frust`) never
/// builds test cfg either, so nothing below is even type-checked without an
/// explicit `--all-targets`.
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
        base_theme, effective_reduce_motion, follow_platform_brightness, theme_after_override_poll,
    };
    use frust_theme::{Brightness, Theme};

    // Every assertion below drives the SAME ladder helpers the production seed
    // ([`IosAppHandle::new`]), appearance (`frust_set_appearance`) and
    // override-poll ([`IosAppHandle::frame`]) arms call — a test that
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

        // ...and Swift's follow-up `frust_set_appearance` drives brightness, so
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

        // ...and it keeps winning against a *later* `frust_set_appearance` flip
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
        // `CADisplayLink` tick.
        let decided = theme_after_override_poll(
            None,
            || panic!("an unchanged poll must not read the process-global default slot"),
            || panic!("an unchanged poll must not read the platform brightness"),
        );
        assert_eq!(decided, None);
    }

    // --- the reduced-motion floor (`frust_set_reduce_motion`) ---------------

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
