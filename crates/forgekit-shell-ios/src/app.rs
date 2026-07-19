//! The iOS app runtime: [`IosAppHandle`], the state behind the opaque C handle.
//!
//! This module is `#[cfg(target_os = "ios")]`; it owns the same resources the
//! desktop shell's `ShellHandler` and the Android shell's `AndroidAppHandle` do —
//! a [`RenderContext`], [`SurfaceRenderer`], [`TextContext`], reusable [`Scene`],
//! plus the app tree — but is driven by the generated Swift app's
//! `CADisplayLink`-posted `forgekit_render_frame` calls instead of a winit loop
//! or Choreographer. It contains no `unsafe`; the FFI boundary lives entirely in
//! [`crate::ffi_glue`]. The `State`/`app_logic` erasure it drives
//! ([`AppTree`](forgekit_shell_common::AppTree)) is platform-agnostic and lives
//! in `forgekit-shell-common`.
//!
//! # Layer lifetime contract
//!
//! This handle retains the Swift-owned `CAMetalLayer` as a raw `*mut c_void`
//! (`metal_layer`) so the shell can *recreate* the `wgpu::Surface` after a
//! `SurfaceLost` — iOS never destroys/recreates the layer itself (contrast
//! Android's window cycle), so without the retained pointer a lost surface would
//! be terminal (permanent black screen). Retaining the raw pointer is sound
//! because the layer's ownership stays with Swift and Swift guarantees it
//! outlives this handle: `forgekit_destroy` drops the handle (and with it the
//! `wgpu::Surface`) *before* the view/layer is released. The handle never frees
//! the layer — it only reads the pointer to hand it back to
//! `on_surface_created_from_metal_layer` at the FFI boundary.
//!
//! The `*mut c_void` field makes [`IosAppHandle`] `!Send`/`!Sync` by default,
//! which is exactly right: every `forgekit_*` call is on the UIKit main thread,
//! so the handle is never sent across threads and no auto-trait promise is made
//! about it.

use std::any::Any;
use std::ffi::c_void;
use std::time::{Duration, Instant};

use crate::accessibility::IosA11yAdapter;
use forgekit_core::FrameTime;
use forgekit_core::event::{
    EditingState, ImeState, InputEvent, PointerButton, PointerEvent, PointerPhase,
};
use forgekit_core::insets::WindowInsets;
use forgekit_reactive::{ReactiveRuntime, provide_context};
use forgekit_render::{FrameOutcome, RenderContext, SurfacePhase, SurfaceRenderer};
use forgekit_scene::{Scene, SceneBuilder};
use forgekit_shell_common::perf::{self, FramePasses, FrameStats, StartupSpans};
use forgekit_shell_common::{
    AppTree, FrameGate, FrameInputs, ThemeOverrideWatcher,
    effective_brightness_for_platform_change, logical_insets, logical_size, sanitize_scale,
};
use forgekit_text::TextContext;
use forgekit_theme::{Brightness, Theme};
use kurbo::{Affine, Point, Size};

use crate::ffi_support::TouchPhase;

/// Everything a running iOS app needs across frames — the state behind the opaque
/// handle Swift passes back into every C call.
///
/// The `renderer` owns the `wgpu::Surface` and is the only thing torn down on
/// drop; the `metal_layer` pointer it was built from is Swift-owned and merely
/// retained (never freed) here so a lost surface can be recreated (see the module
/// docs' *Layer lifetime contract*).
pub struct IosAppHandle {
    render_cx: RenderContext,
    renderer: SurfaceRenderer,
    text_ctx: TextContext,
    /// Reused across frames; `reset()` each frame rather than reallocated.
    scene: Scene,
    app: Box<dyn AppTree>,
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
    /// Set by `forgekit_pause`/`forgekit_resume`; while paused, `frame()` is a
    /// no-op — Metal command submission from a backgrounded iOS app can get the
    /// process killed (spec §10.2), so this is the Rust-side enforcement point.
    paused: bool,
    /// Consecutive failed surface-recreate attempts in the current `SurfaceLost`
    /// episode. Compared against `ffi_support::MAX_RECREATE_ATTEMPTS` so a
    /// persistently-failing recreate degrades to a logged stop instead of a
    /// per-CADisplayLink-frame retry storm; reset by a successful recreate
    /// ([`Self::set_surface`]).
    recreate_failures: u8,
    /// The app's active theme (M3 baseline). Mirrors the desktop shell's
    /// appearance ownership (task 05): starts [`Brightness::Light`] here and is
    /// flipped by [`Self::set_appearance`] once Swift reports the platform's
    /// real dark-mode preference (`forgekit_set_appearance`, called right after
    /// `forgekit_init` returns a handle and again from `traitCollectionDidChange`
    /// — see `templates/app/ios.tmpl`'s `ForgeKitViewController`).
    theme: Theme,
    /// Polls the process-wide app-facing theme override slot
    /// (`forgekit::set_app_theme`/`clear_app_theme`, task 6c-04) once per
    /// frame (see [`Self::frame`]) — see
    /// `forgekit_shell_common::theme_override`'s module docs.
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
    /// The last window insets pushed to the render root (device-parity task 06),
    /// in logical px. Retained so [`Self::set_insets`] skips a no-op push
    /// (`WindowInsets` is `PartialEq`) — both the relayout and the app-side
    /// `provide_context` re-provide only fire on a real change. Starts zero until
    /// Swift's first `forgekit_set_insets` (safe-area / keyboard-frame report).
    insets: WindowInsets,
    /// The accesskit adapter, attached lazily by `forgekit_init_accessibility`
    /// once Swift has a `ForgeKitView` (UIView) to hand over (phase-6d task 05).
    /// `None` until then — `forgekit_init` only receives the `CAMetalLayer`, which
    /// the accesskit `SubclassingAdapter` cannot subclass. While `Some`, `frame()`
    /// drains its queued a11y actions (pre-rebuild) and pushes the post-layout
    /// semantics tree to it (see [`Self::frame`]).
    a11y: Option<IosA11yAdapter>,
    /// Per-frame pass-timing recorder (spec §14 phase 7.A task 09) — honors
    /// the process-wide [`perf::enabled`] switch itself, so every call
    /// against it is a cheap no-op in a non-perf build; [`Self::frame`]
    /// still gates its own `Instant::now()` reads behind [`perf::enabled`]
    /// separately (no clock reads at all when disabled, not just no
    /// recording).
    frame_stats: FrameStats,
    /// The per-frame skip gate (spec §14 phase 7, task 18): consulted each
    /// CADisplayLink tick to skip the rebuild/layout/paint/encode passes on an
    /// idle frame (nothing changed), so CPU/GPU stay near zero on a static
    /// screen — the iOS counterpart to the Android shell's frame gate (task 17).
    /// Honors the `FORGEKIT_NO_FRAME_GATE` kill switch (resolved once at
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
    /// paint's [`forgekit_core::PaintOutcome::needs_frame`] (an in-flight
    /// animation/transition asking for another frame). Latched at the end of each
    /// frame the gate runs; a skipped frame leaves it untouched. Without it the
    /// gate would skip the follow-up frame a running animation needs.
    last_needs_frame: bool,
    /// Handle-side latch feeding [`FrameInputs::theme_or_appearance_changed`]:
    /// set by [`Self::set_appearance`] on an OS-driven light/dark flip, taken
    /// only past the pause/ready gate (like the signals-dirty drain) so a flip
    /// while backgrounded is observed by the first frame after resume.
    /// `push_theme`'s LAYOUT|PAINT change flags carry correctness either way;
    /// this explicit latch is belt-and-suspenders, mirroring the Android shell.
    appearance_dirty: bool,
    /// The startup-span recorder `ffi_glue::create_handle` began and stashed
    /// here via [`Self::set_startup_spans`] immediately after construction —
    /// held until the first frame this handle actually presents completes
    /// and emits the one-line startup summary (see
    /// [`Self::latch_first_frame_presented`]), then dropped. `None` before
    /// that stash call and after the summary has been emitted once.
    startup_spans: Option<StartupSpans>,
}

impl IosAppHandle {
    /// Assemble a handle with an already-created, `SurfaceReady` renderer.
    ///
    /// Called from [`crate::ffi_glue`] after the surface has been built from the
    /// `CAMetalLayer`; runs the first rebuild so the tree exists before the first
    /// frame. No `unsafe` here — the surface creation happens at the FFI boundary.
    ///
    /// Seeds the M3 baseline theme (Light, until Swift's follow-up
    /// `forgekit_set_appearance` reports the real preference) into both delivery
    /// paths (`AppTree::set_theme` for widgets, `provide_context` for app code)
    /// before the first rebuild, mirroring the desktop shell's `apply_theme`.
    /// Must be called under the root reactive `Owner` (see
    /// `crate::ffi_glue::create_handle`) so `provide_context` isn't a silent
    /// no-op.
    pub(crate) fn new(
        render_cx: RenderContext,
        renderer: SurfaceRenderer,
        metal_layer: *mut c_void,
        physical: (u32, u32),
        scale: f32,
        mut app: Box<dyn AppTree>,
    ) -> Self {
        let theme = Theme::m3_baseline();
        app.set_theme(Box::new(theme.clone()));
        provide_context(theme.clone());
        app.rebuild();
        // The gate honors the `FORGEKIT_NO_FRAME_GATE` kill switch at
        // construction; seed its resume-warmup so the first frames after this
        // handle is built run unconditionally (the surface just became ready and
        // the first tick's change signals may not be observable yet — the same
        // reason `resize`/`set_surface`/`resume` re-open the window).
        let mut frame_gate = FrameGate::new();
        frame_gate.note_resumed();
        Self {
            render_cx,
            renderer,
            text_ctx: TextContext::new(),
            scene: Scene::new(),
            app,
            metal_layer,
            physical,
            scale,
            paused: false,
            recreate_failures: 0,
            theme,
            theme_override: ThemeOverrideWatcher::new(),
            theme_override_active: false,
            platform_brightness: Brightness::Light,
            insets: WindowInsets::default(),
            // Attached later, on the first layout, via `forgekit_init_accessibility`
            // once Swift can supply the UIView (see the field doc).
            a11y: None,
            // Honors `perf::enabled()`'s cache internally; a no-op recorder
            // in a non-perf build (see the field doc).
            frame_stats: FrameStats::new(),
            frame_gate,
            events_since_last_frame: false,
            last_needs_frame: false,
            appearance_dirty: false,
            // Stashed by `ffi_glue::create_handle` right after this call
            // returns (see `Self::set_startup_spans`).
            startup_spans: None,
        }
    }

    /// Stash the startup-span recorder `ffi_glue::create_handle` began (see
    /// its docs) so [`Self::latch_first_frame_presented`] can complete it
    /// once this handle actually presents its first frame. Called exactly
    /// once, immediately after construction returns.
    pub(crate) fn set_startup_spans(&mut self, spans: StartupSpans) {
        self.startup_spans = Some(spans);
    }

    /// Complete the startup-span summary on the first frame this handle
    /// actually presents (`FrameOutcome::Rendered`, reported by
    /// [`Self::frame`]'s `bool` return): records
    /// [`perf::SPAN_FIRST_FRAME_PRESENTED`], emits the one
    /// `forgekit-perf startup ...` summary line, then drops the recorder.
    ///
    /// Idempotent by construction (`Option::take`): a `None` — already
    /// latched, or never stashed (perf disabled, so `ffi_glue::create_handle`
    /// still stashes a disabled recorder whose `emit_log` is itself a
    /// no-op) — is a no-op, so `ffi_glue::render_frame` can call this
    /// unconditionally after every [`Self::frame`] call that returns `true`.
    pub(crate) fn latch_first_frame_presented(&mut self) {
        if let Some(mut spans) = self.startup_spans.take() {
            spans.record(perf::SPAN_FIRST_FRAME_PRESENTED);
            spans.emit_log();
        }
    }

    /// Store the accesskit adapter for the app's `ForgeKitView` (phase-6d task 05).
    ///
    /// Called once from [`crate::ffi_glue::init_accessibility`] on the first
    /// layout, after `forgekit_init` returned this handle. The `unsafe`
    /// construction of the [`IosA11yAdapter`] (dynamically subclassing the view to
    /// implement the UIKit accessibility methods) happens at the FFI boundary in
    /// `ffi_glue` — the sanctioned zone for raw-pointer work — so this method is a
    /// plain, safe store: it just takes the already-constructed adapter. From the
    /// next frame on, `frame()` pushes semantics to it and routes its queued
    /// actions.
    pub(crate) fn attach_accessibility(&mut self, adapter: IosA11yAdapter) {
        self.a11y = Some(adapter);
    }

    /// `forgekit_set_appearance`: flip the theme's brightness and re-push it to
    /// both delivery paths (mirrors the desktop shell's `apply_theme`, task 05).
    ///
    /// Runs the `provide_context` re-provide under the process-wide root
    /// [`ReactiveRuntime`]'s owner (fetched fresh here, since — unlike
    /// [`Self::new`] — this call arrives on its own C-ABI entry, not nested
    /// inside `create_handle`'s `with_owner` wrap). No explicit redraw is
    /// scheduled — the continuous `CADisplayLink` loop already repaints every
    /// tick.
    ///
    /// Override-wins rule (task 6c-04): while an app-forced theme override is
    /// active, this platform-appearance report must not flip brightness (see
    /// `effective_brightness_for_platform_change`).
    pub(crate) fn set_appearance(&mut self, dark: bool) {
        let platform = match crate::ffi_support::appearance_from_dark(dark) {
            crate::ffi_support::Appearance::Dark => Brightness::Dark,
            crate::ffi_support::Appearance::Light => Brightness::Light,
        };
        self.platform_brightness = platform;
        self.theme.brightness = effective_brightness_for_platform_change(
            self.theme_override_active,
            self.theme.brightness,
            platform,
        );
        self.push_theme();
        // Frame-gate latch: an OS appearance flip must force the next ready
        // frame to Run (see the `appearance_dirty` field doc).
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

    /// `forgekit_set_insets`: push the platform's per-edge insets onto the render
    /// root (device-parity task 06). Mirrors the Android shell's `set_insets` but
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
    /// PAINT` pending (task 01), which the frame gate already treats as dirty —
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

    /// The retained Swift-owned `CAMetalLayer*` this handle's surface was built
    /// from, used by [`crate::ffi_glue`] to recreate the surface after a
    /// `SurfaceLost`. Returns the raw pointer by value (no borrow) so the FFI
    /// layer can read it before taking a `&mut` via [`renderer_mut`](Self::renderer_mut);
    /// the actual `unsafe` surface creation stays confined to `ffi_glue`.
    pub(crate) fn metal_layer(&self) -> *mut c_void {
        self.metal_layer
    }

    /// Access to the render context + renderer for the FFI layer to drive an
    /// `unsafe` surface *recreation* (the one lifecycle transition that crosses the
    /// raw-pointer boundary), mirroring the Android handle's accessor of the same
    /// name; the safe transitions have their own methods below.
    pub(crate) fn renderer_mut(&mut self) -> (&mut RenderContext, &mut SurfaceRenderer) {
        (&mut self.render_cx, &mut self.renderer)
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
        // signals are observable (spec §14 phase 7, task 18).
        self.frame_gate.note_resumed();
    }

    /// Resize the live surface in place (rotation / bounds change). The
    /// `CAMetalLayer` survives an iOS rotation, so this is always a plain resize —
    /// no recreate path is needed (contrast the Android shell, where a new
    /// `Surface` object forces surface recreation). Safe: the renderer's resize
    /// path touches no raw pointers.
    pub(crate) fn resize(&mut self, physical: (u32, u32), scale: f32) {
        self.renderer
            .on_surface_changed(&self.render_cx, physical.0, physical.1);
        self.physical = physical;
        self.scale = scale;
        // A resize (rotation / bounds change) forces the frame gate's
        // resume-warmup so the next frames re-layout/paint at the new size even
        // if no other change signal fires (spec §14 phase 7, task 18).
        self.frame_gate.note_resumed();
    }

    /// Mark the app paused (`forgekit_pause`): subsequent `frame()`s are no-ops.
    pub(crate) fn pause(&mut self) {
        self.paused = true;
    }

    /// Mark the app resumed (`forgekit_resume`): `frame()`s do work again.
    pub(crate) fn resume(&mut self) {
        self.paused = false;
        // Re-open the frame gate's resume-warmup: the first frames after
        // foregrounding must run unconditionally (a backgrounded app's change
        // signals may have been coalesced away — spec §14 phase 7, task 18).
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

    /// The current lifecycle phase (spec §8.1). `pub(crate)` so [`crate::ffi_glue`]
    /// can gate surface recreation on a `SurfaceLost` phase.
    pub(crate) fn phase(&self) -> SurfacePhase {
        self.renderer.phase()
    }

    /// Deliver one touch contact to the tree (spec §9).
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
        let event = InputEvent::Pointer(PointerEvent {
            phase: core_phase,
            position,
            button: PointerButton::Primary,
        });
        let _ = self.app.event(&event);
        // Latch for the frame gate: a touch between frames must force the next
        // frame to run so the mutated state is reflected (spec §14 phase 7).
        self.events_since_last_frame = true;
    }

    /// Push a whole editing state from the platform IME mirror into the focused
    /// widget (the mobile state-sync path — spec §9). Delegates to
    /// [`AppTree::ime_apply`]; the `EditingState`'s selection/composing indices are
    /// UTF-16 code units (converted to byte offsets by the widget/`forgekit-text`).
    /// The `needs_redraw` in the returned outcome is implicit here — the
    /// `CADisplayLink` loop already ticks the next frame every vsync — so it is
    /// dropped (mirror of [`Self::dispatch_touch`]).
    pub(crate) fn ime_apply(&mut self, state: EditingState) {
        let _ = self.app.ime_apply(state);
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
    /// changed since the last push (phase-6d D3-ios, generation-gated in task 18).
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
    /// shell's `RedrawRequested` path (spec §8) but driven by the Swift
    /// `CADisplayLink`.
    ///
    /// `timestamp_ns` is the `CADisplayLink` tick's `timestamp`
    /// (`CFTimeInterval` seconds) converted to nanoseconds by the Swift caller
    /// — the shell-owned monotonic clock threaded into [`FrameTime`] (spec §8:
    /// `forgekit-core` never reads a clock itself).
    ///
    /// A no-op unless the surface is `SurfaceReady` *and* the app is not paused
    /// (see [`crate::ffi_support::should_render_frame`]). On
    /// `FrameOutcome::SurfaceLost` the machine has already dropped the surface;
    /// this frame becomes a no-op, and the *next* `forgekit_render_frame`/
    /// `forgekit_resize` FFI entry recreates the surface from the retained
    /// `metal_layer` (see [`crate::ffi_glue`]) before rendering resumes.
    ///
    /// Returns whether this call actually presented a frame
    /// (`FrameOutcome::Rendered`) — [`crate::ffi_glue::render_frame`] uses this
    /// to latch the first-presented-frame startup span exactly once (see
    /// [`Self::latch_first_frame_presented`]); every other caller may ignore it.
    pub(crate) fn frame(&mut self, timestamp_ns: u64) -> bool {
        // Pump the UI-thread reactive local-task queue BEFORE the ready/paused
        // gate below: placed after it, queued `spawn_local` completions (e.g. a
        // signal write scheduled from a background task) would stall for as
        // long as the surface stays not-ready/paused instead of draining as
        // soon as the CADisplayLink ticks (phase-5.5 task 08 design). A no-op
        // until `forgekit_init` has installed the runtime.
        if let Some(rt) = ReactiveRuntime::get() {
            rt.pump_local();
        }

        // Poll the app-facing theme override slot (task 6c-04) once per
        // frame, before the ready/paused gate — theme delivery needs no
        // renderer, so this stays in sync even while backgrounded/not-ready
        // (mirroring the reactive-runtime pump just above). Whether it changed is
        // also a frame-gate input (`theme_or_appearance_changed`) captured here.
        let theme_or_appearance_changed = match self.theme_override.poll() {
            Some(Some(theme)) => {
                self.theme = theme;
                self.theme_override_active = true;
                self.push_theme();
                true
            }
            Some(None) => {
                self.theme = Theme::m3_baseline();
                self.theme.brightness = self.platform_brightness;
                self.theme_override_active = false;
                self.push_theme();
                true
            }
            None => false,
        };

        // Pause/ready gate FIRST — a paused/not-ready frame does no work and the
        // frame gate is never even consulted (task 18: the existing early return
        // stays first). Returning here also leaves the signals-dirty flag
        // undrained (it is only `take`n past this gate below), so a tracked-signal
        // write that lands while backgrounded is observed by the first frame
        // after resume rather than being silently consumed on a no-op tick.
        let ready = self.phase() == SurfacePhase::SurfaceReady;
        if !crate::ffi_support::should_render_frame(ready, self.paused) {
            return false;
        }

        // Drain any queued accessibility actions (VoiceOver activations, etc.)
        // BEFORE the rebuild below, so a state change an action makes is picked up
        // by this very frame — the same "mutate now, rebuild next" model touch/IME
        // input uses (spec §9 / phase-6d D2). The handler enqueued these on the
        // main thread; draining takes ownership of the batch so the queue's borrow
        // is dropped before `perform_accessibility_action` re-enters the tree.
        // Whether any action ran is a frame-gate input (`a11y_action_performed`).
        let mut a11y_action_performed = false;
        if let Some(a11y) = self.a11y.as_ref() {
            let actions = a11y.drain_actions();
            a11y_action_performed = !actions.is_empty();
            for req in actions {
                // `target_node.0` is the raw accesskit id the adapter reported;
                // an unknown node or unmodelled action is a benign no-op (see
                // `RenderRoot::perform_accessibility_action`). `needs_redraw` is
                // dropped — the CADisplayLink loop already ticks the next frame.
                let _ = self
                    .app
                    .perform_accessibility_action(req.target_node.0, req.action);
            }
        }

        // Gather the RESEARCH §C OR-list of "something changed" signals and let
        // the frame gate decide whether this frame runs (spec §14 phase 7, task
        // 18 — mirrors the Android shell's task-17 wiring). `signals_dirty` is
        // drained AFTER the pump above (the pump-first ordering contract — see
        // `ReactiveRuntime::take_signals_dirty`) and only now that we are past the
        // ready/paused gate, so a no-op tick never consumes it. The gate honors
        // the `FORGEKIT_NO_FRAME_GATE` kill switch internally (always `Run` when
        // disabled). Correctness over savings: every input defaults toward "run".
        let signals_dirty = ReactiveRuntime::get().is_some_and(|rt| rt.take_signals_dirty());
        let inputs = FrameInputs {
            signals_dirty,
            events_since_last_frame: self.events_since_last_frame,
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
            // Non-draining peek: a skipped frame leaves the flags for the next
            // frame that runs to drain (spec §14 phase 7).
            change_flags_pending: self.app.has_pending_change_flags(),
            // The `appearance_dirty` latch (set by `set_appearance`) is taken
            // only past the pause/ready gate — like the signals-dirty drain —
            // mirroring the Android shell input-for-input.
            theme_or_appearance_changed: theme_or_appearance_changed
                || std::mem::take(&mut self.appearance_dirty),
            // Surface (re)creation/resize is folded into the gate's resume-warmup
            // via `note_resumed` (see `resize`/`set_surface`/`resume`), so it is
            // not threaded as a separate per-frame latch here.
            surface_changed_or_resized: false,
            a11y_action_performed,
            // Driven by the gate's own warmup countdown (`note_resumed`).
            resumed_recently: false,
        };
        // The events latch has now been read into this frame's decision; reset it
        // so the next frame only sees events that arrive from here on.
        self.events_since_last_frame = false;

        if self.frame_gate.decide(inputs).is_skip() {
            // Nothing changed: skip rebuild/layout/paint/encode entirely. Record a
            // skipped-frame stat (ZERO pass durations; counts toward `skipped=` in
            // the perf log line) and return. The CADisplayLink keeps ticking — only
            // frame *production* stops, callbacks don't (the accepted v1 shape,
            // same as Android — see `docs/DEVELOPMENT.md`).
            self.frame_stats.record(FramePasses {
                rebuild: Duration::ZERO,
                layout: Duration::ZERO,
                paint: Duration::ZERO,
                encode_present: Duration::ZERO,
                skipped: true,
            });
            if self.frame_stats.should_emit() {
                self.frame_stats.emit_log();
            }
            return false;
        }

        // Perf instrumentation (spec §14 phase 7.A task 09): read the cached
        // switch exactly once per frame and gate every `Instant::now()` read
        // below behind it — a disabled build takes zero clock reads on this
        // path, not merely a no-op record (`FrameStats::record` itself is
        // also a no-op when disabled, but the timer reads this guard skips
        // are the actual hot-path cost the task's acceptance criteria call
        // out).
        let perf_on = perf::enabled();

        // Rebuild under the root `Owner` so any signal read/`provide_context`
        // during a per-frame rebuild is tracked/scoped correctly, mirroring the
        // desktop shell (`runtime.with_owner(|| ...)`) and `create_handle`'s
        // initial construction. Degrade gracefully to an unwrapped rebuild if
        // the runtime is somehow absent — the frame path must never panic
        // across the C-ABI boundary.
        let rebuild_start = perf_on.then(Instant::now);
        match ReactiveRuntime::get() {
            Some(rt) => rt.with_owner(|| self.app.rebuild()),
            None => self.app.rebuild(),
        }
        let rebuild = rebuild_start.map(|t| t.elapsed()).unwrap_or_default();

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
        // post-layout geometry — spec §9), generation-gated so an unchanged tree
        // is never re-walked or re-pushed (task 18 — see [`Self::publish_semantics`]).
        // Not folded into either pass's timing above/below — it is a11y-conditional
        // work orthogonal to the rebuild/layout/paint/encode split.
        self.publish_semantics();

        let paint_start = perf_on.then(Instant::now);
        self.scene.reset();
        let paint_outcome = {
            let mut builder = SceneBuilder::new(&mut self.scene);
            // HiDPI (spec task 08): lay out in logical pixels, then scale the
            // whole scene by the device pixel ratio for sharp glyphs.
            builder.push_transform(Affine::scale(scale));
            // Shell-owned frame clock (spec §8: time enters from the shell, never
            // `Instant::now()` inside `forgekit-core`) — the `CADisplayLink`
            // timestamp forwarded from Swift.
            let frame_time = FrameTime::from_nanos(timestamp_ns);
            let outcome = self.app.paint(&mut builder, frame_time);
            builder.pop_transform();
            outcome
        };
        let paint = paint_start.map(|t| t.elapsed()).unwrap_or_default();
        // Latch this paint's `needs_frame` continuation signal (spec's v1
        // animation seam) for the NEXT frame's gate: unlike before task 18 — when
        // the continuous CADisplayLink loop let this flag be dropped — the gate
        // would now skip the follow-up frame an in-flight animation/transition
        // needs, so it is fed forward via `FrameInputs::last_needs_frame`.
        self.last_needs_frame = paint_outcome.needs_frame;

        // Clear to the live theme's surface color rather than a hardcoded
        // white, so a dark-scheme app doesn't render its dark-themed widgets
        // over a white canvas (6e Finding 6).
        let encode_start = perf_on.then(Instant::now);
        let render_result =
            self.renderer
                .render(&self.render_cx, &self.scene, self.theme.scheme().surface);
        let encode_present = encode_start.map(|t| t.elapsed()).unwrap_or_default();

        self.frame_stats.record(FramePasses {
            rebuild,
            layout,
            paint,
            encode_present,
            skipped: false,
        });
        if self.frame_stats.should_emit() {
            self.frame_stats.emit_log();
        }

        match render_result {
            // Stale swapchain (e.g. mid-rotation): reconfigured internally; the
            // next CADisplayLink frame draws against the fresh configuration.
            Ok(FrameOutcome::Redraw) => false,
            // Surface lost: dropped by the machine. The next render_frame/resize
            // FFI entry recreates it from the retained `metal_layer` (see
            // `ffi_glue`), bounded by the CADisplayLink cadence — not a busy loop.
            Ok(FrameOutcome::SurfaceLost) => {
                log::warn!("forgekit-shell-ios: surface lost; recreating on next frame/resize");
                false
            }
            Ok(FrameOutcome::Rendered) => true,
            Ok(FrameOutcome::Skipped) => false,
            Err(err) => {
                log::error!("forgekit-shell-ios: render error: {err:#}");
                false
            }
        }
    }
}
