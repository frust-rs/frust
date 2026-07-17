//! The Android app runtime: [`AndroidAppHandle`], the state behind the opaque
//! JNI handle.
//!
//! This module is `#[cfg(target_os = "android")]`; it owns the same resources
//! the desktop shell's `ShellHandler` does — a [`RenderContext`],
//! [`SurfaceRenderer`], [`TextContext`], reusable [`Scene`], plus the app tree —
//! but is driven by Choreographer-posted JNI frames instead of a winit loop.
//! It contains no `unsafe`; the FFI boundary lives entirely in
//! [`crate::jni_glue`]. The `State`/`app_logic` erasure it drives
//! ([`AppTree`](forgekit_shell_common::AppTree)) is platform-agnostic and lives
//! in `forgekit-shell-common`.

use std::any::Any;

use forgekit_core::FrameTime;
use forgekit_core::event::{
    EditingState, ImeState, InputEvent, Key, KeyEvent, Modifiers, NamedKey, PointerButton,
    PointerEvent, PointerPhase,
};
use forgekit_reactive::{ReactiveRuntime, provide_context};
use forgekit_render::{FrameOutcome, RenderContext, SurfacePhase, SurfaceRenderer};
use forgekit_scene::{Scene, SceneBuilder};
use forgekit_shell_common::{
    AppTree, ThemeOverrideWatcher, effective_brightness_for_platform_change, logical_size,
    sanitize_scale,
};
use forgekit_text::TextContext;
use forgekit_theme::{Brightness, Theme};
use kurbo::{Affine, Point, Size};
use ndk::native_window::NativeWindow;

use crate::ffi_support::TouchPhase;

/// Everything a running Android app needs across frames — the state behind the
/// opaque `jlong` handle the JVM passes back into every native call.
///
/// Field order is load-bearing for drop safety: `renderer` (which owns the
/// `wgpu::Surface` built from `window`'s raw pointer) is declared before
/// `window`, so on drop the surface is torn down before the [`NativeWindow`] it
/// borrows is released (spec §8.1: no surface outlives its window).
pub struct AndroidAppHandle {
    render_cx: RenderContext,
    renderer: SurfaceRenderer,
    text_ctx: TextContext,
    /// Reused across frames; `reset()` each frame rather than reallocated.
    scene: Scene,
    app: Box<dyn AppTree>,
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
    /// `ForgeKitSurfaceView.surfaceCreated`).
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
    pub(crate) fn new(
        render_cx: RenderContext,
        renderer: SurfaceRenderer,
        window: NativeWindow,
        physical: (u32, u32),
        scale: f32,
        mut app: Box<dyn AppTree>,
    ) -> Self {
        let theme = Theme::m3_baseline();
        app.set_theme(Box::new(theme.clone()));
        provide_context(theme.clone());
        app.rebuild();
        Self {
            render_cx,
            renderer,
            text_ctx: TextContext::new(),
            scene: Scene::new(),
            app,
            physical,
            scale,
            window: Some(window),
            theme,
            theme_override: ThemeOverrideWatcher::new(),
            theme_override_active: false,
            platform_brightness: Brightness::Light,
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
        self.app.set_theme(Box::new(self.theme.clone()));
        let theme = self.theme.clone();
        match ReactiveRuntime::get() {
            Some(rt) => rt.with_owner(|| provide_context(theme)),
            None => provide_context(theme),
        }
    }

    /// Push a theme to both delivery paths (mirrors [`Self::set_appearance`]'s
    /// second half) — the shared helper [`Self::frame`]'s theme-override poll
    /// calls, so a forced override and a live appearance change go through one
    /// code path.
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

    /// Access to the render context + renderer for the FFI layer to drive an
    /// `unsafe` surface *creation* (the one lifecycle transition that crosses the
    /// raw-pointer boundary); the safe transitions have their own methods below.
    pub(crate) fn renderer_mut(&mut self) -> (&mut RenderContext, &mut SurfaceRenderer) {
        (&mut self.render_cx, &mut self.renderer)
    }

    /// Record the window + physical size backing a freshly (re)created surface.
    ///
    /// Assigning `window` last drops the previous [`NativeWindow`] (releasing it)
    /// — safe here because the previous surface was already torn down inside the
    /// preceding `on_surface_created_from_android_window` (spec §8.1).
    pub(crate) fn set_window(&mut self, window: NativeWindow, physical: (u32, u32)) {
        self.physical = physical;
        self.window = Some(window);
    }

    /// Resize the live surface in place (same window, new dimensions). Safe: the
    /// renderer's resize path touches no raw pointers.
    pub(crate) fn resize(&mut self, physical: (u32, u32)) {
        self.renderer
            .on_surface_changed(&self.render_cx, physical.0, physical.1);
        self.physical = physical;
    }

    /// Tear the surface down (`surfaceDestroyed`): drop the renderer's surface
    /// first (spec §8.1), then release the [`NativeWindow`] it borrowed.
    pub(crate) fn destroy_surface(&mut self) {
        self.renderer.on_surface_destroyed();
        self.window = None;
    }

    /// The current lifecycle phase (spec §8.1).
    pub(crate) fn phase(&self) -> SurfacePhase {
        self.renderer.phase()
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
        let event = InputEvent::Pointer(PointerEvent {
            phase: core_phase,
            position,
            button: PointerButton::Primary,
        });
        let _ = self.app.event(&event);
    }

    /// Apply a whole editing state pushed by the platform IME (`nativeImeApply`),
    /// routing it to the focused widget as an
    /// [`InputEvent::Ime`]`(`[`ImeEvent::ApplyEditingState`]`)` via
    /// [`AppTree::ime_apply`]. `state`'s indices are UTF-16 code units (the seam
    /// unit); the focused widget converts them. No explicit redraw is scheduled —
    /// the Choreographer loop already posts the next frame (see [`Self::dispatch_touch`]).
    ///
    /// [`ImeEvent::ApplyEditingState`]: forgekit_core::event::ImeEvent::ApplyEditingState
    pub(crate) fn ime_apply(&mut self, state: EditingState) {
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
        let _ = self.app.event(&event);
    }

    /// Run one frame: rebuild → layout → paint → render, mirroring the desktop
    /// shell's `RedrawRequested` path (spec §8) but driven by Choreographer.
    ///
    /// `frame_time_nanos` is Kotlin's Choreographer `frameTimeNanos` for this
    /// tick (already clamped non-negative at the JNI boundary — see
    /// [`crate::jni_glue::native_on_frame`]), the shell-owned monotonic clock
    /// threaded into [`FrameTime`] (spec §8: `forgekit-core` never reads a clock
    /// itself).
    ///
    /// A no-op when the surface isn't `SurfaceReady` (Kotlin keeps posting frames
    /// across surface loss; this makes those cheap). On `FrameOutcome::SurfaceLost`
    /// the machine has already dropped the surface; recovery waits for the next
    /// `surfaceChanged`/`surfaceCreated` rather than recreating mid-frame.
    pub(crate) fn frame(&mut self, frame_time_nanos: u64) {
        // Pump the reactive runtime's local task queue BEFORE the surface-ready
        // gate below: a controller-driven `spawn_local` task must keep draining
        // every Choreographer tick even while the surface is torn down (e.g.
        // mid-rotation) or not yet created, not just once it's ready — otherwise
        // local tasks stall through surface churn.
        crate::jni_glue::pump_reactive_runtime();

        // Poll the app-facing theme override slot (task 6c-04) once per
        // frame, before the surface-ready gate — theme delivery needs no
        // renderer, so this stays in sync even while the surface is torn down
        // (mirroring the reactive-runtime pump just above).
        match self.theme_override.poll() {
            Some(Some(theme)) => {
                self.theme = theme;
                self.theme_override_active = true;
                self.push_theme();
            }
            Some(None) => {
                self.theme = Theme::m3_baseline();
                self.theme.brightness = self.platform_brightness;
                self.theme_override_active = false;
                self.push_theme();
            }
            None => {}
        }

        if self.phase() != SurfacePhase::SurfaceReady {
            return;
        }

        // Rebuild under the root `Owner` so any signal read/`provide_context`
        // during a per-frame rebuild is tracked/scoped correctly, mirroring the
        // desktop shell (`runtime.with_owner(|| ...)`) and `create_handle`'s
        // initial construction. Degrade gracefully to an unwrapped rebuild if
        // the runtime is somehow absent — the frame path must never panic
        // across the JNI boundary.
        match ReactiveRuntime::get() {
            Some(rt) => rt.with_owner(|| self.app.rebuild()),
            None => self.app.rebuild(),
        }

        // Sanitize once per frame; layout and the paint transform below MUST
        // consume this identical value (an untrusted JNI `jfloat` density
        // must never let the two passes disagree — see `sanitize_scale`).
        let scale = sanitize_scale(self.scale);
        let (lw, lh) = logical_size(self.physical.0, self.physical.1, scale);
        let logical = Size::new(lw, lh);
        {
            let text_ctx: &mut dyn Any = &mut self.text_ctx;
            self.app.layout(logical, text_ctx);
        }

        self.scene.reset();
        {
            let mut builder = SceneBuilder::new(&mut self.scene);
            // HiDPI (spec task 08): lay out in logical pixels, then scale the
            // whole scene by the device pixel ratio for sharp glyphs.
            builder.push_transform(Affine::scale(scale));
            // Shell-owned frame clock (spec §8: time enters from the shell, never
            // `Instant::now()` inside `forgekit-core`) — Choreographer's
            // `frameTimeNanos`, forwarded from Kotlin via `nativeOnFrame`.
            let frame_time = FrameTime::from_nanos(frame_time_nanos);
            // The paint pass returns a `needs_frame` continuation signal (spec's
            // v1 animation seam). This shell runs a continuous Choreographer loop
            // that already posts the next frame every tick, so the flag is
            // irrelevant here and deliberately dropped — unlike the desktop shell,
            // whose `ControlFlow::Wait` loop must honor it to keep animating.
            let _ = self.app.paint(&mut builder, frame_time);
            builder.pop_transform();
        }

        match self
            .renderer
            .render(&self.render_cx, &self.scene, peniko::Color::WHITE)
        {
            // Stale swapchain (e.g. mid-rotation): reconfigured internally; the
            // next Choreographer frame draws against the fresh configuration.
            Ok(FrameOutcome::Redraw) => {}
            // Surface lost: dropped by the machine; wait for surfaceChanged to
            // recreate it (Android pairs loss with a destroy/create cycle).
            Ok(FrameOutcome::SurfaceLost) => {
                log::warn!("forgekit-shell-android: surface lost; awaiting surfaceChanged");
            }
            Ok(FrameOutcome::Rendered | FrameOutcome::Skipped) => {}
            Err(err) => log::error!("forgekit-shell-android: render error: {err:#}"),
        }
    }
}
