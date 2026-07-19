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
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use accesskit_android::InjectingAdapter;
use accesskit_android::jni as ak_jni;
use forgekit_core::FrameTime;
use forgekit_core::SemanticsUpdate;
use forgekit_core::accesskit::{
    Action, ActionHandler, ActionRequest, ActivationHandler, NodeId, Tree, TreeId, TreeUpdate,
};
use forgekit_core::event::{
    EditingState, ImeState, InputEvent, Key, KeyEvent, Modifiers, NamedKey, PointerButton,
    PointerEvent, PointerPhase,
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
    /// The last window insets pushed to the render root (device-parity task 06),
    /// in logical px. Retained so [`Self::set_insets`] can skip a no-op push
    /// (`WindowInsets` is `PartialEq`) — both the `RenderRoot::set_insets` relayout
    /// and the app-side `provide_context` re-provide only fire on a real change.
    /// Starts zero (no occlusion) until Kotlin's first `nativeOnInsetsChanged`.
    insets: WindowInsets,
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
    /// The accessibility adapter state (phase 6d D3), attached lazily by
    /// `nativeInitAccessibility` after `nativeInit` (see
    /// [`Self::attach_accessibility`]). `None` until then — a11y is best-effort
    /// and its wiring never gates the render path. This field is independent of
    /// the `renderer`/`window` drop-order contract above: [`InjectingAdapter`]'s
    /// `Drop` detaches the delegate through its own retained `JavaVM`, touching
    /// neither the surface nor the window.
    a11y: Option<AndroidA11y>,
    /// Per-frame pass timing (task 08, spec §14 phase 7.A): rebuild/layout/
    /// paint/encode+present durations, aggregated and rate-limit logged
    /// (`forgekit-perf frame ...`) by [`Self::frame`]. Honors the
    /// process-wide [`perf::enabled`] switch — a disabled recorder allocates
    /// and records nothing.
    frame_stats: FrameStats,
    /// The cold-start span recorder begun in
    /// [`crate::jni_glue::create_handle`] (`StartupSpans::begin()`), carried
    /// through so [`Self::frame`]'s first successful render can record
    /// [`perf::SPAN_FIRST_FRAME_PRESENTED`] and emit the one
    /// `forgekit-perf startup ...` summary line. `Option::take`n at that
    /// point, so the field is `None` for the rest of the handle's lifetime —
    /// the latch that guarantees a single emission and costs nothing (no
    /// clock read) on every later frame.
    startup_spans: Option<StartupSpans>,
    /// The skip-frame gate (task 17, spec §14 phase 7): consulted once per
    /// [`Self::frame`] after the per-tick inputs are gathered. When it returns
    /// [`FrameDecision::Skip`](forgekit_shell_common::FrameDecision::Skip) the
    /// frame's rebuild/layout/paint/encode/present passes are all skipped and
    /// only a `skipped` [`FramePasses`] is recorded — CPU/GPU stay near idle
    /// while nothing changes. The Choreographer keeps posting frames regardless
    /// (only frame *production* stops); the loop cadence is unchanged. Honors
    /// the [`FORGEKIT_NO_FRAME_GATE`](forgekit_shell_common::frame_gate::NO_FRAME_GATE_VAR)
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
    /// ([`forgekit_core::PaintOutcome::needs_frame`] — a running animation/
    /// transition). Set from each run frame's paint return, read into
    /// [`FrameInputs::last_needs_frame`] so an in-flight animation keeps
    /// producing frames until it settles (whereupon paint returns `false` and
    /// the gate may skip again).
    last_needs_frame: bool,
    /// Whether the layout pass has run at least once. Until it has, the
    /// layout-skip seam in [`Self::frame`] force-runs layout (a paint before the
    /// first layout would have no valid geometry); after the first layout it is
    /// gated on the drained [`ChangeFlags`](forgekit_core::view::ChangeFlags)
    /// (or a surface resize).
    first_layout_done: bool,
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
/// [`AppTree::perform_accessibility_action`](forgekit_shell_common::AppTree::perform_accessibility_action)
/// itself — so an assistive-tech action can never alias the handle mid-frame.
struct AndroidA11y {
    /// The accesskit_android injecting adapter. On construction it attaches a
    /// `View.AccessibilityDelegate` + `OnHoverListener` to the host
    /// `ForgeKitSurfaceView` (posted to the UI thread), and it pushes
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
    /// [`AppTree::semantics_if_changed`](forgekit_shell_common::AppTree::semantics_if_changed)).
    last_pushed_gen: u64,
}

impl AndroidA11y {
    /// Build the adapter and its shared channels, attaching accessibility to
    /// `host` (the `ForgeKitSurfaceView`). Contains no `unsafe`: the raw-pointer
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
    /// `startup_spans` is the cold-start recorder `create_handle` began
    /// before any of the above (task 08, spec §14 phase 7.A): this method
    /// records [`perf::SPAN_FIRST_REBUILD_DONE`] right after the initial
    /// `rebuild()` below, then carries the recorder into the returned handle
    /// for `Self::frame` to finish (first frame presented + emit).
    pub(crate) fn new(
        render_cx: RenderContext,
        renderer: SurfaceRenderer,
        window: NativeWindow,
        physical: (u32, u32),
        scale: f32,
        mut app: Box<dyn AppTree>,
        mut startup_spans: StartupSpans,
    ) -> Self {
        let theme = Theme::m3_baseline();
        app.set_theme(Box::new(theme.clone()));
        provide_context(theme.clone());
        app.rebuild();
        startup_spans.record(perf::SPAN_FIRST_REBUILD_DONE);
        // Seed the resume-warmup window so the first handful of frames run
        // unconditionally (surface just came up; the first tick's change
        // signals may not yet be observable — see `FrameGate::note_resumed`).
        // The initial `rebuild()` above also leaves change flags pending, which
        // independently forces the first frame to run and to lay out.
        let mut frame_gate = FrameGate::new();
        frame_gate.note_resumed();
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
            insets: WindowInsets::default(),
            theme_override: ThemeOverrideWatcher::new(),
            theme_override_active: false,
            platform_brightness: Brightness::Light,
            a11y: None,
            frame_stats: FrameStats::new(),
            startup_spans: Some(startup_spans),
            frame_gate,
            events_since_last_frame: false,
            surface_dirty: false,
            appearance_dirty: false,
            last_needs_frame: false,
            first_layout_done: false,
        }
    }

    /// Attach the accesskit Android adapter to the host `ForgeKitSurfaceView`
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
    /// [`AppTree::perform_accessibility_action`](forgekit_shell_common::AppTree::perform_accessibility_action).
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
    /// Gated twice over: the [`AppTree::semantics_if_changed`](forgekit_shell_common::AppTree::semantics_if_changed)
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

    /// Resize the live surface in place (same window, new dimensions). Safe: the
    /// renderer's resize path touches no raw pointers.
    ///
    /// `density` re-sanitizes and stores the display's device pixel ratio for
    /// this configuration (device-parity task 06 — see [`Self::set_window`]).
    pub(crate) fn resize(&mut self, physical: (u32, u32), density: f32) {
        self.renderer
            .on_surface_changed(&self.render_cx, physical.0, physical.1);
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
        // Frame-gate latch (task 17): an event between frames must force the
        // next frame to run so the tree reflects the dispatch. Set even on a
        // no-op dispatch — correctness beats savings, and the gate defaults to
        // "must run" when in doubt.
        self.events_since_last_frame = true;
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
    /// threaded into [`FrameTime`] (spec §8: `forgekit-core` never reads a clock
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
    /// [`ChangeFlags`](forgekit_core::view::ChangeFlags). Every input either
    /// reads a tree accessor or a handle-side latch cleared here — see the
    /// inline comments at each gather site for the RESEARCH §C mapping.
    ///
    /// The pump → gather → decide ordering and the latch lifecycle are asserted
    /// only by inspection, not a unit test: this whole module is
    /// `#[cfg(target_os = "android")]` (it needs a live GPU surface + JNI handle
    /// to construct an [`AndroidAppHandle`]), so it never runs under the host
    /// `cargo test --workspace`. The decision logic it drives *is* exhaustively
    /// host-tested where it lives — `forgekit_shell_common::frame_gate`'s
    /// `decide`/warmup/kill-switch tests — so what stays unverified here is only
    /// the wiring, which the `cargo check --target aarch64-linux-android` gate
    /// compile-checks and the manual 7.E device checklist exercises.
    ///
    /// [`Skip`]: forgekit_shell_common::FrameDecision::Skip
    /// [`Run`]: forgekit_shell_common::FrameDecision::Run
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

        if self.phase() != SurfacePhase::SurfaceReady {
            // Surface not ready (Kotlin keeps posting frames across surface
            // loss): nothing to render or gate. The reactive pump + theme poll
            // above already ran so state stays live through surface churn; the
            // event/surface latches are intentionally *not* cleared here so the
            // first ready frame still sees them. No FrameStats row is recorded
            // for a not-ready tick (it never was pre-gate either).
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
            events_since_last_frame: std::mem::take(&mut self.events_since_last_frame),
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

        if self.frame_gate.decide(inputs).is_skip() {
            // Skip path (task 17): nothing changed — return before rebuild, so
            // CPU/GPU stay near idle. Record a `skipped` FramePasses (all-zero
            // pass durations) so the skip counter accumulates in the perf log
            // line; the Choreographer keeps re-posting callbacks, so only frame
            // *production* stops, not the loop cadence.
            self.frame_stats.record(FramePasses {
                skipped: true,
                ..FramePasses::default()
            });
            if self.frame_stats.should_emit() {
                self.frame_stats.emit_log();
            }
            return;
        }

        // ---------------------------------------------------------------
        // Run path: rebuild -> (layout iff needed) -> paint -> encode/present,
        // timed as before (task 08's instrumentation preserved).
        // ---------------------------------------------------------------

        // Rebuild under the root `Owner` so any signal read/`provide_context`
        // during a per-frame rebuild is tracked/scoped correctly, mirroring the
        // desktop shell (`runtime.with_owner(|| ...)`) and `create_handle`'s
        // initial construction. Degrade gracefully to an unwrapped rebuild if
        // the runtime is somehow absent — the frame path must never panic
        // across the JNI boundary.
        let rebuild_start = perf_on.then(Instant::now);
        match ReactiveRuntime::get() {
            Some(rt) => rt.with_owner(|| self.app.rebuild()),
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
            // `Instant::now()` inside `forgekit-core`) — Choreographer's
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

        // Clear to the live theme's surface color rather than a hardcoded
        // white, so a dark-scheme app doesn't render its dark-themed widgets
        // over a white canvas (6e Finding 6).
        let encode_start = perf_on.then(Instant::now);
        let render_result =
            self.renderer
                .render(&self.render_cx, &self.scene, self.theme.scheme().surface);
        let encode_present_time = encode_start.map_or(Duration::ZERO, |t| t.elapsed());

        match render_result {
            // Stale swapchain (e.g. mid-rotation): reconfigured internally; the
            // next Choreographer frame draws against the fresh configuration.
            Ok(FrameOutcome::Redraw) => {}
            // Surface lost: dropped by the machine; wait for surfaceChanged to
            // recreate it (Android pairs loss with a destroy/create cycle).
            Ok(FrameOutcome::SurfaceLost) => {
                log::warn!("forgekit-shell-android: surface lost; awaiting surfaceChanged");
            }
            Ok(FrameOutcome::Rendered) => {
                // First successful present (task 08): close out the cold-start
                // span recorder exactly once. `Option::take` both consumes it
                // and doubles as the latch — every later `Rendered` frame sees
                // `None` here at no cost beyond the `Option` check.
                if let Some(mut spans) = self.startup_spans.take() {
                    spans.record(perf::SPAN_FIRST_FRAME_PRESENTED);
                    spans.emit_log();
                }
            }
            Ok(FrameOutcome::Skipped) => {}
            Err(err) => log::error!("forgekit-shell-android: render error: {err:#}"),
        }

        // Record this frame's pass timings (task 08). `FrameStats::record`/
        // `should_emit`/`emit_log` are themselves cheap no-ops when disabled
        // (the recorder's own `enabled` flag, captured once at construction —
        // see `FrameStats::new`), so no extra gating is needed here beyond the
        // `perf_on`-gated timer reads above.
        self.frame_stats.record(FramePasses {
            rebuild: rebuild_time,
            layout: layout_time,
            paint: paint_time,
            encode_present: encode_present_time,
            skipped: false,
        });
        if self.frame_stats.should_emit() {
            self.frame_stats.emit_log();
        }
    }
}

/// Unit tests for the pure accessibility-tree assembly (phase 6d D3).
///
/// This module is inside the `#[cfg(target_os = "android")]` `app` module, so it
/// only compiles/runs for the Android target — the assembly references
/// `forgekit_core`/`accesskit` types, both of which are Android-gated
/// dependencies of this crate by deliberate design (see `Cargo.toml`), so it
/// cannot be a host test the way [`crate::ffi_support`]'s pure helpers are. The
/// Android compile gate (`cargo check --target aarch64-linux-android`) is the
/// primary check that this path stays correct.
#[cfg(test)]
mod tests {
    use super::tree_update_from_semantics;
    use forgekit_core::SemanticsUpdate;
    use forgekit_core::accesskit::{Node, NodeId, Role, Tree, TreeId};

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
