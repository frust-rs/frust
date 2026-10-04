//! The iOS app runtime: [`IosAppHandle`], the state behind the opaque C handle.
//!
//! This module is `#[cfg(target_os = "ios")]`; it owns the same resources the
//! desktop shell's `ShellHandler` and the Android shell's `AndroidAppHandle` do —
//! a `RenderContext`, `SurfaceRenderer`, [`TextContext`], reusable [`Scene`],
//! plus the app tree — but is driven by the generated Swift app's
//! `CADisplayLink`-posted `frust_render_frame` calls instead of a winit loop
//! or Choreographer. It contains no `unsafe`; the FFI boundary lives entirely in
//! [`crate::ffi_glue`]. The `State`/`build` erasure it drives
//! ([`AppTree`](frust_shell_common::AppTree)) is platform-agnostic and lives
//! in `frust-shell-common`.
//!
//! # Module map
//!
//! This file holds the handle's fields and its constructor; each submodule owns
//! one seam of the runtime, and every `crate::app::X` path the FFI layer used
//! before the split still resolves through the re-exports below.
//!
//! | Module | Responsibility |
//! |--------|-----------------|
//! | [`executor`] | The render path: the split/inline executors, the scene handoff payload, and the shared encode→acquire→submit tail |
//! | [`frame`] | The `CADisplayLink` frame body and the frame-gate input gathering |
//! | [`theme`] | The default-theme precedence ladder, appearance + reduced-motion arms, theme delivery |
//! | [`input`] | Touch dispatch (raw/resampled), the IME state-sync pair, and the reactive-owner event wrap |
//! | [`a11y`] | The accesskit adapter attach and the generation-gated semantics push |
//! | [`surface`] | Surface + pause/resume lifecycle, resolved-translucency sync, insets/window metrics |
//! | [`present_sync`] | iOS-only: the render→UI present handoff for `presentsWithTransaction` hosts |
//! | [`platform_view`] | The differ's command backlog mapped to the FFI wire shape |
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

mod a11y;
mod executor;
mod frame;
mod input;
mod platform_view;
mod present_sync;
mod surface;
mod theme;

// The FFI layer (`crate::ffi_glue`) builds and drives the render path directly,
// so every type it names stays reachable at its pre-split `crate::app::*` path.
pub(crate) use executor::{
    FrameExecutor, InlineExecutor, PaintedScene, SplitExecutor, render_scene,
};
pub(crate) use present_sync::PresentHandoff;

use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use frust_core::insets::WindowInsets;
use frust_reactive::{TrackedScope, provide_context};
use frust_render::SurfaceAlphaRequest;
use frust_scene::Scene;
use frust_shell_common::font_registry::FontRegistryWatcher;
use frust_shell_common::platform_view::FramePairing;
use frust_shell_common::resample::{PointerResampler, ResampledPointer};
use frust_shell_common::{
    AppTree, FrameGate, PlatformViewState, ThemeOverrideWatcher, WindowMetricsPublisher,
    sanitize_scale,
};
use frust_text::TextContext;
use frust_theme::{Brightness, Theme};

use crate::accessibility::IosA11yAdapter;

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
    /// thread owns the `RenderContext`/`SurfaceRenderer` + surface and the UI
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
    /// `FrameInputs::signals_dirty`. Without this wrap a completed async load's
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
    /// The app's active theme — the seeded base ([`theme::seed_theme`]: a design
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
    /// frame (see [`Self::poll_theme_override`]) — see
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
    /// override-poll arm in [`Self::poll_theme_override`]). Paired with
    /// [`Self::os_reduce_motion`] through `effective_reduce_motion` so the
    /// OS report is a floor over the authored value rather than a replacement
    /// of it — without this, turning the OS setting back off would have to
    /// guess what the theme originally asked for.
    authored_reduce_motion: bool,
    /// The last window insets pushed to the render root, in logical px. Retained
    /// so [`Self::set_insets`] / [`Self::set_corner_insets`] skip a no-op push
    /// (`WindowInsets` is `PartialEq`) — both the relayout and the app-side
    /// `provide_context` re-provide only fire on a real change.
    ///
    /// A composite fed by **two** FFI calls carrying disjoint halves:
    /// `frust_set_insets` delivers the edges (`view_padding` from the safe
    /// area, `view_insets` from the keyboard frame) and
    /// `frust_set_corner_insets` delivers `corner_insets` (the iPadOS 26+ window
    /// control). Each call replaces only its own half and keeps the other.
    /// Starts zero until Swift's first push; the corners stay zero below
    /// iOS 26.
    insets: WindowInsets,
    /// Change detector for the app-facing [`WindowMetrics`](frust_core::WindowMetrics)
    /// context: seeded before the first rebuild in [`Self::new`] and re-polled
    /// from every entry point where one of its inputs actually moves
    /// ([`Self::resize`]/[`Self::set_surface`], [`Self::set_insets`] and
    /// [`Self::set_corner_insets`]) —
    /// never from [`Self::frame`], which only reads values those entry points
    /// stored. See `surface::IosAppHandle::push_window_metrics` for why the
    /// guard is load-bearing.
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
    /// Handle-side latch feeding `FrameInputs::events_since_last_frame`: set by
    /// [`Self::dispatch_touch`]/[`Self::ime_apply`] whenever a touch/IME event
    /// reaches the tree between frames, read and cleared once per [`Self::frame`].
    /// Ensures a tap/keystroke on an otherwise-idle screen is never skipped.
    events_since_last_frame: bool,
    /// Cache feeding `FrameInputs::focus_or_ime_changed`: the
    /// [`AppTree::focus_ime_generation`] this handle saw as of the last frame
    /// it actually PRODUCED (not the last gathered tick). The **edge** (focus
    /// gained/lost, IME surface published/cleared) is `live != cached`. The
    /// gather-time read in [`Self::frame`] is a non-mutating peek; the commit
    /// happens only past that function's `decide_paced(..).is_skip()` early
    /// return, on a frame that actually runs.
    ///
    /// Unlike `events_since_last_frame` above, this is NOT reset eagerly at
    /// gather time. That latch is an `is_paced_only_frame` disqualifier, so a
    /// tick carrying it can never be skipped — clearing it before the gate
    /// decides is provably harmless. `focus_or_ime_changed` is the one input
    /// that both rides inside a paced decision AND is consumed on read;
    /// draining it on a tick the gate then resolves to Skip would lose the
    /// edge outright — the exact bug a commit-at-gather-time shape had, fixed
    /// by deferring the commit past the skip return. Seeded from the tree
    /// after the constructor's first rebuild so the first tick reports no
    /// spurious edge (it runs on the resume warmup regardless).
    ///
    /// Reading the *level* (`is_focus_active`) here instead is what kept a
    /// focused screen rendering every `CADisplayLink` tick and put caret pacing
    /// out of reach — see `FrameInputs::focus_or_ime_changed`.
    last_focus_ime_gen: u64,
    /// Handle-side latch feeding `FrameInputs::last_needs_frame`: the previous
    /// paint's [`frust_core::PaintOutcome::needs_frame`] (an in-flight
    /// animation/transition asking for another frame). Latched at the end of each
    /// frame the gate runs; a skipped frame leaves it untouched. Without it the
    /// gate would skip the follow-up frame a running animation needs.
    last_needs_frame: bool,
    /// Handle-side latch feeding `FrameInputs::last_needs_frame_paced_only`:
    /// the previous paint's [`frust_core::PaintOutcome::needs_frame_paced_only`]
    /// (its frame request aggregated to [`frust_core::TickClass::CosmeticLoop`]
    /// alone). Lets [`FrameGate::decide_paced`] throttle a paced-only decorative
    /// loop to the theme's `cosmetic_loop_rate` instead of every `CADisplayLink`
    /// tick. Latched beside `last_needs_frame`; a skipped frame leaves it.
    last_needs_frame_paced_only: bool,
    /// Handle-side latch feeding `FramePacing::requested_interval`: the previous
    /// paint's [`frust_core::PaintOutcome::paced_interval`] — the MIN-lattice
    /// fold of every `PaintCtx::request_frame_paced_at` that pass, `None` when
    /// none named an interval. Lets a loop slower than the theme's cap (a ~500ms
    /// caret blink) pace at its own cadence. Latched beside
    /// `last_needs_frame_paced_only`; a skipped frame leaves it.
    last_paced_interval: Option<Duration>,
    /// Handle-side latch feeding `FrameInputs::theme_or_appearance_changed`:
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
    /// `CADisplayLink` vsync clock threaded into `FrameTime`: resampling only
    /// differences timestamps, so one consistent source suffices.
    resample_clock: Instant,
    /// Scratch buffer the resampler drains into each frame, reused (cleared, not
    /// reallocated) so a drag's per-frame resample allocates nothing.
    pointer_scratch: Vec<ResampledPointer>,
    /// The previous `CADisplayLink` tick timestamp (ns), for the deadline-aware
    /// pacing estimate: the tick-to-tick delta is this
    /// frame's deadline budget (see `resample::frame_interval_nanos`). `None`
    /// before the first frame.
    last_frame_time_nanos: Option<u64>,
    /// Running count of frames whose measured work (rebuild+layout+paint+encode,
    /// excluding the vsync present wait) overran the frame-target deadline.
    /// **Instrumentation only** — accumulated and logged
    /// (`frust-perf deadline`) behind `perf::enabled`; never drops work.
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

impl IosAppHandle {
    /// Assemble a handle with an already-created, `SurfaceReady` renderer.
    ///
    /// Called from [`crate::ffi_glue`] after the surface has been built from the
    /// `CAMetalLayer`; runs the first rebuild so the tree exists before the first
    /// frame. No `unsafe` here — the surface creation happens at the FFI boundary.
    ///
    /// Seeds the base theme ([`theme::seed_theme`]: a design system's
    /// `set_default_theme`, else the built-in `Theme::neutral` fallback,
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
    /// `perf::SPAN_FIRST_REBUILD_DONE` via `FrameExecutor::record_first_rebuild`
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

        // The seeded base theme (`theme::seed_theme`: a design system's
        // `set_default_theme`, else the built-in `Theme::neutral()` fallback),
        // carrying that base's own brightness until Swift's follow-up
        // `frust_set_appearance` overwrites it with the device's real
        // preference. A design system that needs its own font bytes shaped from
        // the first frame registers them through
        // `frust_shell_common::font_registry::register_app_fonts`, which the
        // construction-time drain above applies — the shell itself bundles no
        // fonts.
        let theme = theme::seed_theme();
        // The base's own reduced-motion token, kept so a later OS report (or a
        // later theme swap) can re-derive the effective value instead of
        // guessing what the theme asked for — see `authored_reduce_motion`.
        let authored_reduce_motion = theme.motion.reduce_motion;

        app.set_theme(Box::new(theme.clone()));
        // Thread the surface's RESOLVED translucency into the render root and
        // seed the app-facing RESOLVED slot from the same value, so the first
        // rebuild below paints (and reads) a real verdict rather than a
        // request — see `surface::seed_resolved_translucency`.
        surface::seed_resolved_translucency(app.as_mut(), &translucent_resolved);
        provide_context(theme.clone());
        // Seed the app-facing window-shape context beside the theme, so an
        // `build`/`Component::build` calling `use_context::<WindowMetrics>()`
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
        // Seed the focus/IME edge cache from the just-rebuilt tree, so the first
        // gathered tick reports an edge only if the session actually moved after
        // construction.
        let last_focus_ime_gen = app.focus_ime_generation();
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
            last_focus_ime_gen,
            last_needs_frame: false,
            last_needs_frame_paced_only: false,
            last_paced_interval: None,
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
}
