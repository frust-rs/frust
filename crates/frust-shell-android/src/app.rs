//! The Android app runtime: [`AndroidAppHandle`], the state behind the opaque
//! JNI handle.
//!
//! This module is `#[cfg(target_os = "android")]`; it owns the same resources
//! the desktop shell's `ShellHandler` does — a `RenderContext`,
//! `SurfaceRenderer`, [`TextContext`], reusable [`Scene`], plus the app tree —
//! but is driven by Choreographer-posted JNI frames instead of a winit loop.
//! It contains no `unsafe`; the FFI boundary lives entirely in
//! [`crate::jni_glue`]. The `State`/`app_logic` erasure it drives
//! ([`AppTree`](frust_shell_common::AppTree)) is platform-agnostic and lives
//! in `frust-shell-common`.
//!
//! # Module map
//!
//! This root file holds the handle's shape (the cross-frame state), its
//! construction, and its theme/appearance ownership — the precedence ladder
//! (`base_theme` → seeded default → app override) plus every arm that installs a
//! `Theme`. Each remaining concern is one submodule:
//!
//! | Module | Responsibility |
//! |--------|----------------|
//! | [`frame`] | One Choreographer tick: gate inputs, the run/skip decision, the rebuild→layout→paint→submit passes |
//! | [`executor`] | The render-path half: the inline and split executor arms and the painted-scene handoff |
//! | [`render`] | The shared encode→acquire→submit tail and the render side's published signals |
//! | [`surface`] | Surface create/recreate/resize/destroy, resolved translucency, insets and window metrics |
//! | [`platform_view`] | The differ backlog Kotlin polls, behind the release gate and scroll-sync tail |
//! | [`input`] | Between-frame touch/IME delivery and the root-owner event-pass wrap |
//! | [`a11y`] | The accesskit adapter, its shared channels, and the semantics publish |

mod a11y;
mod executor;
mod frame;
mod input;
mod platform_view;
mod render;
mod surface;

pub(crate) use executor::{FrameExecutor, InlineExecutor, PaintedScene, SplitExecutor};
pub(crate) use render::{RenderSignals, render_scene};

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use frust_core::event::PointerEvent;
use frust_core::insets::WindowInsets;
use frust_reactive::{ReactiveRuntime, TrackedScope, provide_context};
use frust_scene::Scene;
use frust_shell_common::font_registry::FontRegistryWatcher;
use frust_shell_common::platform_view::FramePairing;
use frust_shell_common::resample::PointerResampler;
use frust_shell_common::{
    AppTree, FrameGate, PlatformViewState, ThemeOverrideWatcher, WindowMetricsPublisher,
    default_theme, effective_brightness_for_platform_change, sanitize_scale,
};
use frust_text::TextContext;
use frust_theme::{Brightness, Theme};
use ndk::native_window::NativeWindow;

use crate::sync_tail::ScrollSyncTail;

use self::a11y::AndroidA11y;

/// Everything a running Android app needs across frames — the state behind the
/// opaque `jlong` handle the JVM passes back into every native call.
///
/// Field order is load-bearing for drop safety: `executor` (which — in the
/// render-thread split — owns the render thread whose `wgpu::Surface` was built
/// from `window`'s raw pointer, and — inline — owns the `SurfaceRenderer`
/// directly) is declared before `window`, so on drop the executor is torn down
/// (the split's `Drop` joins the render thread, dropping its surface) before the
/// `NativeWindow` it borrows is released: no surface outlives its
/// window.
pub struct AndroidAppHandle {
    /// The render-path half of the frame loop: either the
    /// render-thread split ([`FrameExecutor::Split`], the default) — where a
    /// dedicated thread owns the `RenderContext`/`SurfaceRenderer` + surface
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
    /// (`TrackedScope::notify_dirty` → `ReactiveRuntime::mark_signals_dirty`)
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
    /// the `renderer`/`window` drop-order contract above: `InjectingAdapter`'s
    /// `Drop` detaches the delegate through its own retained `JavaVM`, touching
    /// neither the surface nor the window.
    a11y: Option<AndroidA11y>,
    /// The skip-frame gate: consulted once per
    /// [`Self::frame`] after the per-tick inputs are gathered. When it returns
    /// [`FrameDecision::Skip`](frust_shell_common::FrameDecision::Skip) the
    /// frame's rebuild/layout/paint/encode/present passes are all skipped and
    /// only a `skipped` `FramePasses` is recorded — CPU/GPU stay near idle
    /// while nothing changes. The Choreographer keeps posting frames regardless
    /// (only frame *production* stops); the loop cadence is unchanged. Honors
    /// the [`FRUST_NO_FRAME_GATE`](frust_shell_common::frame_gate::NO_FRAME_GATE_VAR)
    /// kill switch (resolved once at construction) — a disabled gate always
    /// runs, matching pre-gate behavior verbatim.
    frame_gate: FrameGate,
    /// Latch: a pointer/IME event reached the tree since the last frame. Set by
    /// [`Self::dispatch_touch`]/[`Self::ime_apply`]/[`Self::ime_action`] (the
    /// JNI event entry points that run *between* frames), read-and-cleared each
    /// frame into `FrameInputs::events_since_last_frame`. This is what keeps a
    /// mid-drag gesture producing frames: Android delivers a continuous stream
    /// of `MotionEvent.ACTION_MOVE`s during a drag, each tripping this latch (so
    /// it also stands in for pointer-capture, which has no `AppTree` accessor —
    /// see [`Self::frame`]'s input-gathering).
    events_since_last_frame: bool,
    /// Latch: the GPU surface was (re)created or resized since the last frame.
    /// Set by [`Self::set_window`]/[`Self::resize`], read-and-cleared each frame
    /// into `FrameInputs::surface_changed_or_resized` (and, in the same frame,
    /// used to force the layout pass so the new dimensions take effect). Those
    /// same lifecycle transitions also open the gate's resume-warmup window (see
    /// `FrameGate::note_resumed`).
    surface_dirty: bool,
    /// Latch: a platform appearance-ish preference changed since the last
    /// frame — the light/dark mode (`nativeSetAppearance` →
    /// [`Self::set_appearance`]) or the reduced-motion setting
    /// (`nativeSetReduceMotion` → [`Self::set_reduce_motion`]).
    /// Read-and-cleared each
    /// frame into `FrameInputs::theme_or_appearance_changed` (OR'd with the
    /// in-frame theme-override poll result). Belt-and-suspenders with the change
    /// flags [`Self::set_appearance`]'s `push_theme` already marks.
    appearance_dirty: bool,
    /// Latch: the previous paint pass asked for another frame
    /// ([`frust_core::PaintOutcome::needs_frame`] — a running animation/
    /// transition). Set from each run frame's paint return, read into
    /// `FrameInputs::last_needs_frame` so an in-flight animation keeps
    /// producing frames until it settles (whereupon paint returns `false` and
    /// the gate may skip again).
    last_needs_frame: bool,
    /// Latch: the previous paint's frame request aggregated to
    /// [`frust_core::TickClass::CosmeticLoop`] alone (a pacable decorative loop
    /// with no concurrent transition —
    /// [`frust_core::PaintOutcome::needs_frame_paced_only`]). Read into
    /// `FrameInputs::last_needs_frame_paced_only` so
    /// `FrameGate::decide_paced` throttles a paced-only frame to the theme's
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
    /// starves the gate (see `PointerResampler::has_pending`).
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
    /// frame's deadline budget (see `resample::frame_interval_nanos`). `None`
    /// before the first frame.
    last_frame_time_nanos: Option<u64>,
    /// Running count of frames whose measured work (rebuild+layout+paint+encode,
    /// excluding the vsync present wait) overran the frame-target deadline.
    /// **Instrumentation only** — accumulated and logged
    /// (`frust-perf deadline`) behind `perf::enabled`; it never drops or
    /// reshapes work.
    deadline_overruns: u64,
    /// The differ turning this handle's
    /// published `PlatformViewFrame`s into the idempotent `ViewCommand`
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
    /// see `FramePairing`'s Lifecycle note.
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
    /// `perf::SPAN_FIRST_REBUILD_DONE` via [`FrameExecutor::record_first_rebuild`]
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
        // Seed both translucency consumers — the render root and the app-facing
        // RESOLVED slot — from the flag as it stands right now, before the first
        // rebuild below; the per-frame [`Self::sync_translucent_resolved`]
        // re-reads it every frame from then on (see
        // [`surface::seed_translucent_resolved`], which owns both pushes beside
        // that per-frame sync).
        surface::seed_translucent_resolved(app.as_mut(), &translucent_resolved);
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
    /// type-erased into the render root (`AppTree::set_theme`) and
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

    /// Poll the app-facing theme-override slot
    /// (`frust::set_app_theme`/`clear_app_theme`) once, applying whatever it
    /// reports to the active theme. Returns whether the active theme moved —
    /// [`Self::frame`]'s `theme_or_appearance_changed` gate input.
    ///
    /// Reverting an override lands on the base this shell seeded itself
    /// from — the design-system default when one was supplied, else the
    /// built-in fallback — with brightness re-derived from the platform's
    /// last reported preference rather than inherited from the cleared
    /// override; that whole ladder lives in [`theme_after_override_poll`] so
    /// this arm and its unit tests share one implementation. The seeded
    /// supplier stays lazy: an unchanged poll never reads the
    /// process-global slot.
    fn poll_theme_override(&mut self) -> bool {
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
            return true;
        }
        false
    }
}

/// Unit tests for the default-theme seed ladder.
///
/// This module is inside the `#[cfg(target_os = "android")]` `app` module, so it
/// only compiles/runs for the Android target — the seed ladder references
/// `frust_theme`'s [`Theme`], an Android-gated dependency of this crate by
/// deliberate design (see `Cargo.toml`), so it cannot be a host test the way
/// [`crate::ffi_support`]'s pure helpers are — worse, the documented Android
/// compile gate (`cargo check --target aarch64-linux-android -p frust`) never
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
        app_is_dark, base_theme, effective_reduce_motion, follow_platform_brightness,
        theme_after_override_poll,
    };
    use frust_theme::{Brightness, Theme};

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
