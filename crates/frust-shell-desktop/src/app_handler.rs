//! The winit 0.30 [`ApplicationHandler`] that drives a Frust app in a desktop
//! preview window (spec §12.9).
//!
//! Ownership mirrors the shared platform bootstrap (spec §10.3): the shell owns
//! the [`RenderContext`], per-window [`SurfaceRenderer`], the shared
//! [`TextContext`], the [`RenderRoot`], and the application `State`/`app_logic`.
//! Each on-demand frame runs the framework passes in Masonry order (the v0
//! subset): rebuild → layout → paint → render.
//!
//! It also owns an `accesskit_winit` [`Adapter`] (phase 6d D3): created in
//! `resumed` with the same [`EventLoopProxy`](winit::event_loop::EventLoopProxy)
//! the [`ShellUserEvent`] wake mechanism already uses (extended with an
//! [`ShellUserEvent::Accessibility`] variant), forwarded every
//! [`WindowEvent`] via [`Adapter::process_event`] (its documented contract:
//! *"This must be called whenever a new window event is received"*), and fed a
//! fresh [`accesskit::TreeUpdate`](frust_core::accesskit::TreeUpdate) —
//! built from [`RenderRoot::semantics`]/[`RenderRoot::semantics_if_changed`] —
//! post-layout in `RedrawRequested`. Platform `ActionRequest`s arrive back
//! through the same proxy and route into
//! [`RenderRoot::perform_accessibility_action`]. Adapter creation cannot panic
//! without a real platform a11y bus present: `accesskit_winit`'s unix backend
//! only lazily connects to AT-SPI/D-Bus and stays inert (`update_if_active`
//! becomes a no-op) until an assistive-technology client actually activates it
//! — safe to construct in a headless/no-AT-client CI environment.

use std::any::Any;
use std::sync::Arc;
use std::time::Instant;

use accesskit_winit::{
    Adapter, Event as AccessibilityEvent, WindowEvent as AccessibilityWindowEvent,
};
use anyhow::{Context, Result};
use frust_core::FrameTime;
use frust_core::RenderRoot;
use frust_core::SemanticsUpdate;
use frust_core::accesskit::{Tree, TreeId, TreeUpdate};
use frust_core::event::{
    ImeEvent, InputEvent, Key, KeyEvent, Modifiers, NamedKey, PointerButton, PointerEvent,
    PointerPhase, ScrollDelta,
};
use frust_core::view::View;
use frust_reactive::{FrameWaker, ReactiveRuntime, TrackedScope, provide_context};
use frust_render::{EncodeOutcome, FrameOutcome, RenderContext, SurfacePhase, SurfaceRenderer};
use frust_scene::{Scene, SceneBuilder};
use frust_shell_common::perf::{
    FramePasses, FrameStats, SPAN_ADAPTER_READY, SPAN_DEVICE_READY, SPAN_FIRST_ENCODE_DONE,
    SPAN_FIRST_FRAME_PRESENTED, SPAN_FIRST_REBUILD_DONE, SPAN_INIT_ENTRY,
    SPAN_PIPELINE_CACHE_RESTORED, SPAN_RENDERER_READY, StartupSpans,
};
use frust_shell_common::{ThemeOverrideWatcher, effective_brightness_for_platform_change};
use frust_text::TextContext;
use frust_theme::{Brightness, Theme};
use kurbo::{Affine, Point, Size};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalPosition, LogicalSize};
use winit::event::{ElementState, Ime, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key as WinitKey, ModifiersState, NamedKey as WinitNamedKey};
use winit::window::{Theme as WinitTheme, Window, WindowAttributes, WindowId};

/// Initial preview-window size, in logical pixels.
const INITIAL_SIZE: LogicalSize<u32> = LogicalSize::new(800, 600);
const WINDOW_TITLE: &str = "Frust";

/// User events posted to the desktop event loop from off the UI thread.
///
/// Winit's [`ControlFlow::Wait`] idles the loop until an event arrives, so a
/// signal write on a background thread would otherwise never surface. The
/// [`FrameWaker`] installed in [`run_desktop`] fires on the clean→dirty edge of
/// a tracked signal and sends one of these through the loop's `EventLoopProxy`,
/// waking the loop to pump local tasks and schedule a redraw. Kept an enum (not
/// a unit type) so future user-driven events can be added without changing the
/// loop's user-event type.
///
/// [`ShellUserEvent::Accessibility`] (phase 6d D3) is the second producer of
/// this same proxy: `accesskit_winit`'s [`Adapter::with_event_loop_proxy`]
/// requires its `T: From<accesskit_winit::Event>` bound, satisfied below. Its
/// payload (an [`accesskit::ActionRequest`](frust_core::accesskit::ActionRequest)
/// wrapper) is neither `Copy` nor `PartialEq`, so this enum can no longer
/// derive those either — every existing use already matched/constructed it by
/// value, so nothing downstream needed to change.
#[derive(Debug)]
enum ShellUserEvent {
    /// One or more tracked signals became dirty since the last frame — pump the
    /// UI-thread local task queue and request a redraw.
    SignalsDirty,
    /// An `accesskit_winit` adapter event: an initial-tree pull, a platform
    /// accessibility action request, or a deactivation notice — see
    /// [`ShellHandler::handle_accessibility_event`].
    Accessibility(AccessibilityEvent),
}

impl From<AccessibilityEvent> for ShellUserEvent {
    fn from(event: AccessibilityEvent) -> Self {
        ShellUserEvent::Accessibility(event)
    }
}

/// Run `app_logic` over `state` in a desktop preview window until it is closed.
///
/// Blocks the calling thread on the winit event loop. The event loop is
/// on-demand ([`ControlFlow::Wait`]): frames are produced only in response to a
/// redraw request (state change on rebuild, or a resize), never free-running
/// (spec §8).
pub fn run_desktop<State, Logic, V>(state: State, app_logic: Logic) -> Result<()>
where
    State: 'static,
    V: View<State>,
    Logic: FnMut(&mut State) -> V + 'static,
{
    // Install the stderr `log::Log` sink once, so `frust-shell-common::perf`'s
    // `log::info!` startup-span/frame-stats lines below are actually visible
    // (desktop had no logger at all before this — see `logger`'s module docs).
    crate::logger::init_once();

    // Perf instrumentation (spec §14 phase 7.A, task 10): `run_desktop` is the
    // desktop shell's init-entry point, mirroring the mobile shells'
    // `nativeInit`/`frust_init` span (see `frust_shell_common::perf`'s
    // module docs). A no-op recorder unless `FRUST_TRACE` is set (compile-
    // or runtime-side) — see `perf::enabled`'s docs.
    let mut startup_spans = StartupSpans::begin();
    startup_spans.record(SPAN_INIT_ENTRY);

    // winit 0.30 has no `EventLoop::<T>::new()` — the typed-user-event loop is
    // built through the builder only.
    let event_loop = EventLoop::<ShellUserEvent>::with_user_event().build()?;
    event_loop.set_control_flow(ControlFlow::Wait);

    // A tracked signal write on any thread fires this waker, which sends a user
    // event through the proxy to wake the `Wait` loop. `send_event` fails only
    // once the loop has closed (a shutdown race) — benign, so its result is
    // ignored. `ReactiveRuntime::init` must run on this (the UI) thread: it
    // claims the thread as owner of the local task queue `pump_local` drains.
    let proxy = event_loop.create_proxy();
    // A second clone is kept on the handler (`accesskit_proxy`) so `resumed` can
    // hand the accesskit_winit `Adapter` its own event-loop proxy — the waker
    // closure below consumes its own clone, so neither producer starves.
    let accesskit_proxy = proxy.clone();
    let waker: FrameWaker = Arc::new(move || {
        let _ = proxy.send_event(ShellUserEvent::SignalsDirty);
    });
    let runtime = ReactiveRuntime::init(waker);

    let mut handler = ShellHandler {
        state,
        app_logic,
        runtime,
        scope: TrackedScope::new(),
        root: RenderRoot::new(),
        text_ctx: TextContext::new(),
        render_cx: RenderContext::new(),
        scene: Scene::new(),
        cursor: Point::ZERO,
        modifiers: Modifiers::default(),
        compose: ComposeLatch::default(),
        ime_sync: ImeSync::default(),
        window: None,
        accesskit_proxy,
        adapter: None,
        // The generation `RenderRoot::new()` starts at (0); the first rebuild
        // always dirties it to a different value (a first `rebuild_view` always
        // returns non-empty `ChangeFlags` — see `RenderRoot::rebuild`), so the
        // first post-layout `semantics_if_changed` check below is guaranteed to
        // see a change and push the initial tree.
        semantics_seen: 0,
        // Starts in `NoSurface`; the surface is created in `resumed` once the
        // window exists, and driven through the §8.1 lifecycle from there.
        renderer: SurfaceRenderer::new(),
        fatal: None,
        epoch: Instant::now(),
        // M3 baseline, Light until `resumed` seeds the window's real preference.
        theme: Theme::m3_baseline(),
        theme_seeded: false,
        theme_override: ThemeOverrideWatcher::new(),
        theme_override_active: false,
        startup_spans,
        renderer_spans_recorded: false,
        first_rebuild_recorded: false,
        first_encode_recorded: false,
        first_frame_recorded: false,
        frame_stats: FrameStats::new(),
    };
    event_loop.run_app(&mut handler)?;
    finish(handler.fatal)
}

/// Turns a post-loop `ShellHandler::fatal` into the `run_desktop` result.
///
/// Pulled out of `run_desktop` so the "surface a stashed init error as `Err`"
/// behavior is unit-testable without driving a real winit event loop.
fn finish(fatal: Option<anyhow::Error>) -> Result<()> {
    match fatal {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

/// Convert a winit physical-pixel position into logical (density-independent)
/// pixels by the window's `scale_factor`.
///
/// winit reports `CursorMoved`/`PixelDelta` in **physical** pixels (verified
/// empirically on macOS — a HiDPI window reports positions at 2× the logical
/// point value); the widget tree lays out and hit-tests in the same logical
/// space the layout pass uses (spec §10.3), so every pointer coordinate is
/// divided by the scale factor at the shell boundary. Pulled out as a free
/// function so the conversion is unit-testable without a live window.
fn physical_to_logical(x: f64, y: f64, scale: f64) -> Point {
    Point::new(x / scale, y / scale)
}

/// Map a winit [`WinitNamedKey`] to our editing-semantics [`NamedKey`] set
/// (task 51), or `None` for a named key we carry no editing semantics for
/// (function keys, media keys, etc. — those fall through to `KeyboardInput`
/// being dropped rather than misreported as text).
fn map_named_key(key: WinitNamedKey) -> Option<NamedKey> {
    Some(match key {
        WinitNamedKey::Enter => NamedKey::Enter,
        WinitNamedKey::Backspace => NamedKey::Backspace,
        WinitNamedKey::Delete => NamedKey::Delete,
        WinitNamedKey::ArrowLeft => NamedKey::ArrowLeft,
        WinitNamedKey::ArrowRight => NamedKey::ArrowRight,
        WinitNamedKey::ArrowUp => NamedKey::ArrowUp,
        WinitNamedKey::ArrowDown => NamedKey::ArrowDown,
        WinitNamedKey::Home => NamedKey::Home,
        WinitNamedKey::End => NamedKey::End,
        WinitNamedKey::Escape => NamedKey::Escape,
        WinitNamedKey::Tab => NamedKey::Tab,
        _ => return None,
    })
}

/// Map winit's window [`WinitTheme`] (`None` when the platform can't report a
/// preference) to a [`Brightness`], defaulting to [`Brightness::Light`].
///
/// Pulled out as a free function so the seed-and-flip mapping is unit-testable
/// without a live window (winit's `Window::theme()` needs a real window).
fn brightness_from_winit(theme: Option<WinitTheme>) -> Brightness {
    match theme {
        Some(WinitTheme::Dark) => Brightness::Dark,
        _ => Brightness::Light,
    }
}

/// Map winit's [`ModifiersState`] bitflags to our [`Modifiers`] chord.
fn map_modifiers(state: ModifiersState) -> Modifiers {
    Modifiers {
        shift: state.shift_key(),
        ctrl: state.control_key(),
        alt: state.alt_key(),
        meta: state.super_key(),
    }
}

/// Map a winit `KeyboardInput`'s fields into our [`KeyEvent`], or `None` when
/// it produces nothing worth dispatching.
///
/// Takes the individual fields off `winit::event::KeyEvent` (rather than the
/// struct itself, whose `platform_specific` field is private to winit and so
/// can't be constructed in this crate's tests) so the mapping stays a pure,
/// directly unit-testable function.
///
/// Named keys ([`WinitKey::Named`]) map through [`map_named_key`]; anything
/// else falls back to the key's resolved `text` (already dead-key/smart-quote
/// resolved by the platform) as [`Key::Character`] — **unless** `composing` is
/// set, in which case the character is dropped (the dedupe rule: while an IME
/// [`Ime::Preedit`] is active, `Ime::Commit` is the authoritative source for the
/// composed text, not `KeyboardInput.text`). `repeat` passes through unfiltered
/// — callers decide whether to honor auto-repeat. Only key-down (`Pressed`)
/// events map; releases produce `None` (spec: `KeyEvent` has no up/down phase).
fn map_key_event(
    logical_key: &WinitKey,
    text: Option<&str>,
    state: ElementState,
    repeat: bool,
    modifiers: Modifiers,
    composing: bool,
) -> Option<KeyEvent> {
    if state != ElementState::Pressed {
        return None;
    }
    let key = match logical_key {
        WinitKey::Named(named) => Key::Named(map_named_key(*named)?),
        _ => {
            if composing {
                return None;
            }
            Key::Character(text?.to_string())
        }
    };
    Some(KeyEvent {
        key,
        modifiers,
        repeat,
    })
}

/// Tracks whether an IME preedit (marked-text) composition is currently in
/// progress, purely from the `WindowEvent::Ime` stream — the dedupe latch
/// [`map_key_event`] consults so `KeyboardInput`-derived `Character` events are
/// suppressed while `Ime::Preedit`/`Ime::Commit` are the authoritative source
/// (winit reportedly withholds `KeyboardInput` during preedit on macOS, but we
/// dedupe defensively cross-platform per the task's research note).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct ComposeLatch {
    active: bool,
}

impl ComposeLatch {
    /// Observe one `WindowEvent::Ime` variant, updating the latch and mapping
    /// it to our [`ImeEvent`] in the same pass (the mapping and the
    /// dedupe-state transition are the same decision, so they live together).
    ///
    /// An empty-text `Preedit` closes the latch (per winit's contract it
    /// precedes a `Commit`); `Commit`/`Enabled`/`Disabled` also close it — each
    /// ends or restarts a composition session.
    fn observe(&mut self, ime: &Ime) -> ImeEvent {
        match ime {
            Ime::Preedit(text, cursor) => {
                self.active = !text.is_empty();
                ImeEvent::Compose {
                    text: text.clone(),
                    cursor: *cursor,
                }
            }
            Ime::Commit(text) => {
                self.active = false;
                ImeEvent::Commit(text.clone())
            }
            Ime::Enabled => {
                self.active = false;
                ImeEvent::Enabled
            }
            Ime::Disabled => {
                self.active = false;
                ImeEvent::Disabled
            }
        }
    }

    /// Whether a `KeyboardInput`-derived `Character` event should be dropped
    /// because a composition is currently in progress.
    fn is_composing(&self) -> bool {
        self.active
    }
}

/// Cached view of what we last told winit about the platform IME, so
/// [`ShellHandler::sync_ime`] only calls `set_ime_allowed`/`set_ime_cursor_area`
/// on an actual change rather than every dispatched event.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
struct ImeSync {
    /// The last `set_ime_allowed` value sent to winit.
    allowed: bool,
    /// The last `set_ime_cursor_area` position/size sent to winit, if any.
    cursor_area: Option<(LogicalPosition<f64>, LogicalSize<f64>)>,
}

/// Owns everything a running desktop app needs across frames.
struct ShellHandler<State: 'static, Logic, V: View<State>> {
    state: State,
    app_logic: Logic,
    /// The process-wide reactive runtime, initialized on this (UI) thread in
    /// [`run_desktop`]. Per-frame rebuilds run under its root [`Owner`], and
    /// [`ReactiveRuntime::pump_local`] drains the UI-thread local task queue on
    /// each wake and frame.
    runtime: &'static ReactiveRuntime,
    /// Records which signals the last rebuild read, so a later write to any of
    /// them dirties the scope and (via the frame waker) wakes the loop for one
    /// more frame. Re-tracked from scratch every rebuild.
    scope: TrackedScope,
    root: RenderRoot<State, V>,
    text_ctx: TextContext,
    render_cx: RenderContext,
    /// Reused across frames; `reset()` each frame rather than reallocated.
    scene: Scene,
    /// Last known cursor position in logical pixels, updated on every
    /// `CursorMoved`. `MouseInput` (button press/release) carries no position of
    /// its own, so it reuses this — mirroring how winit models the two events.
    cursor: Point,
    /// Current modifier chord, updated by `ModifiersChanged` — which winit
    /// delivers *before* the `KeyboardInput` that uses it, so this is always
    /// current by the time a key event is mapped.
    modifiers: Modifiers,
    /// Tracks whether an IME preedit composition is in progress, so
    /// `KeyboardInput`-derived `Character` events can be deduped against it.
    compose: ComposeLatch,
    /// What we last told winit about the platform IME (`set_ime_allowed`/
    /// `set_ime_cursor_area`), so [`ShellHandler::sync_ime`] only calls them on
    /// an actual change.
    ime_sync: ImeSync,
    /// Created lazily in `resumed()` (macOS requires window creation there).
    window: Option<Arc<Window>>,
    /// A clone of the wake proxy handed to the `accesskit_winit` [`Adapter`] at
    /// creation (phase 6d D3) — kept separate from the `FrameWaker`'s own clone
    /// (`run_desktop`) so `resumed` can construct the adapter without needing
    /// to unpick it from the (already-moved-into-a-closure) waker.
    accesskit_proxy: winit::event_loop::EventLoopProxy<ShellUserEvent>,
    /// The `accesskit_winit` platform adapter (phase 6d D3), created once in
    /// `resumed` alongside the window (it must be constructed before the
    /// window is first shown — see [`Adapter::with_event_loop_proxy`]'s
    /// contract). `None` only before the first `resumed` call.
    adapter: Option<Adapter>,
    /// The [`RenderRoot::semantics_generation`] value last pushed to the
    /// adapter (phase 6d D3's dirty gate) — compared every `RedrawRequested`
    /// via [`RenderRoot::semantics_if_changed`] so an unchanged tree is never
    /// re-walked/re-pushed.
    semantics_seen: u64,
    /// The §8.1 surface lifecycle machine; empty (`NoSurface`) until `resumed()`
    /// creates the surface, and torn down on `suspended()`.
    renderer: SurfaceRenderer,
    /// Set when `resumed` hits an unrecoverable init error; `event_loop.exit()`
    /// only stops the loop (it can't return an `Err`), so the error is stashed
    /// here and re-raised by `run_desktop` once `run_app` returns.
    fatal: Option<anyhow::Error>,
    /// The shell-owned monotonic epoch the per-frame [`FrameTime`] is measured
    /// from. `frust-core` never reads a clock itself (spec §8: time enters from
    /// the shell) — the desktop shell samples `epoch.elapsed()` at paint and hands
    /// the nanosecond delta to [`RenderRoot::paint`]. Task 06 leaves this the
    /// desktop clock; only the mobile shells swap in a platform vsync timestamp.
    epoch: Instant,
    /// The app's active theme (M3 baseline). The shell owns the appearance
    /// state: its `brightness` is seeded from the window's reported preference
    /// in `resumed` and flipped live on `WindowEvent::ThemeChanged`. On every
    /// change the shell re-boxes it into [`RenderRoot::set_theme`] (so widgets
    /// read it via `PaintCtx::theme_as`) and `provide_context`s a clone under
    /// the root owner (so app code reads it via `use_context::<Theme>()`).
    theme: Theme,
    /// Whether the initial theme has been pushed to the render root / context
    /// yet — seeded once in the first `resumed`, before the first rebuild.
    theme_seeded: bool,
    /// Polls the process-wide app-facing theme override slot
    /// (`frust::set_app_theme`/`clear_app_theme`, task 6c-04) once per
    /// frame, before rebuild in `RedrawRequested` — see
    /// `frust_shell_common::theme_override`'s module docs.
    theme_override: ThemeOverrideWatcher,
    /// Whether an app-forced theme override is currently active. While `true`,
    /// `WindowEvent::ThemeChanged` must not flip `self.theme`'s brightness —
    /// the override wins entirely until `clear_app_theme` runs (see
    /// `effective_brightness_for_platform_change`).
    theme_override_active: bool,
    /// Perf instrumentation (spec §14 phase 7.A, task 10): named startup
    /// milestones from `run_desktop`'s entry epoch, emitted once as a single
    /// summary line after the first frame presents. A no-op recorder unless
    /// `FRUST_TRACE` is set - see `frust_shell_common::perf`'s module
    /// docs.
    startup_spans: StartupSpans,
    /// Whether `SPAN_ADAPTER_READY`/`SPAN_DEVICE_READY`/`SPAN_RENDERER_READY`
    /// have already been recorded. Desktop has no finer-grained visibility
    /// into `on_surface_created`'s internals than "it returned" (adapter,
    /// device, and renderer creation all happen inside that one async call -
    /// see `frust-render::RenderContext::ensure_device`/`install_surface`),
    /// so all three spans are recorded together at that single observable
    /// boundary, on the first successful surface creation only - a later
    /// suspend/resume surface recreation must not re-record them.
    renderer_spans_recorded: bool,
    /// Whether `SPAN_FIRST_REBUILD_DONE` has already been recorded - the
    /// desktop shell rebuilds every redraw-requested frame, but the span is
    /// only meaningful once, for the app's first rebuild.
    first_rebuild_recorded: bool,
    /// Whether `SPAN_FIRST_ENCODE_DONE` has already been recorded - the shell
    /// encodes every redraw-requested frame, but the first-frame-decomposition
    /// span (task 10.A) is only meaningful once, for the app's first encode when
    /// something was actually encoded. Distinct from `first_frame_recorded`
    /// because a first encode can precede the first successful present
    /// (an intervening `Redraw` reconfigure frame).
    first_encode_recorded: bool,
    /// Whether `SPAN_FIRST_FRAME_PRESENTED` has already been recorded (and
    /// `StartupSpans::emit_log` fired) - set on the first
    /// `FrameOutcome::Rendered`.
    first_frame_recorded: bool,
    /// Perf instrumentation (task 10): per-pass frame timings
    /// (rebuild/layout/paint/encode/present), aggregated into rolling
    /// percentiles and emitted periodically while frames are actually being
    /// produced - desktop is dirty-driven (spec §8), so this piggybacks on
    /// `RedrawRequested` rather than a timer; see `FrameStats::should_emit`'s
    /// docs for the emit cadence.
    frame_stats: FrameStats,
}

impl<State, Logic, V> ShellHandler<State, Logic, V>
where
    State: 'static,
    V: View<State>,
    Logic: FnMut(&mut State) -> V + 'static,
{
    /// Deliver one input event to the tree and schedule a frame if it dirtied
    /// state. This is the dirty-driven half of the desktop model (spec §8): the
    /// event pass never repaints, it only sets `needs_redraw`, which we turn into
    /// a single `request_redraw()` so the `Wait` loop wakes for exactly one frame.
    ///
    /// Also re-syncs the platform IME ([`ShellHandler::sync_ime`]) after every
    /// dispatch (task 54): a focus change, blur, or caret move can all happen as
    /// a side effect of any event, not just keyboard/IME ones.
    fn dispatch(&mut self, window: &Window, event: InputEvent) {
        let outcome = self.root.event(&mut self.state, &event);
        self.sync_ime(window);
        if outcome.needs_redraw {
            window.request_redraw();
        }
    }

    /// Query [`RenderRoot::ime_state`] and push any *changed* IME-relevant
    /// state to winit: `set_ime_allowed` on an active-transition, and
    /// `set_ime_cursor_area` (logical caret rect) when active and the caret
    /// moved. Both calls are gated behind [`ImeSync`]'s cache so a steady-state
    /// focused field with an unmoving caret doesn't re-issue them every event.
    fn sync_ime(&mut self, window: &Window) {
        let ime_state = self.root.ime_state();
        let active = ime_state.as_ref().is_some_and(|s| s.active);

        if active != self.ime_sync.allowed {
            window.set_ime_allowed(active);
            self.ime_sync.allowed = active;
            if !active {
                self.ime_sync.cursor_area = None;
            }
        }

        if active && let Some(caret) = ime_state.and_then(|s| s.caret) {
            let area = (
                LogicalPosition::new(caret.x0, caret.y0),
                LogicalSize::new(caret.width(), caret.height()),
            );
            if self.ime_sync.cursor_area != Some(area) {
                window.set_ime_cursor_area(area.0, area.1);
                self.ime_sync.cursor_area = Some(area);
            }
        }
    }

    /// Push the current [`Theme`] to both delivery paths and schedule a repaint.
    ///
    /// 1. `RenderRoot::set_theme` boxes a clone so widgets recover it during
    ///    layout/paint via `PaintCtx::theme_as` (the type-erased widget path).
    /// 2. `provide_context` under the reactive root owner re-provides a clone so
    ///    a `Component::build`'s `use_context::<Theme>()` resolves it on the next
    ///    rebuild (the app-code path). Re-providing under the same owner replaces
    ///    the previous value, so a live dark-mode flip is observed next frame.
    ///
    /// Called once to seed the theme in `resumed` and again on every
    /// `WindowEvent::ThemeChanged`.
    fn apply_theme(&mut self, window: &Window) {
        self.root.set_theme(Box::new(self.theme.clone()));
        let theme = self.theme.clone();
        self.runtime.with_owner(move || provide_context(theme));
        window.request_redraw();
    }

    /// Handle one `accesskit_winit` adapter event delivered through the
    /// event-loop proxy (phase 6d D3).
    ///
    /// * `InitialTreeRequested` — the adapter activated (an AT client
    ///   connected) and has no tree yet: pull one fresh
    ///   [`RenderRoot::semantics`] pass and push it, recording the generation so
    ///   the next `RedrawRequested` dirty-check doesn't immediately re-push the
    ///   same tree.
    /// * `ActionRequested` — forward straight to
    ///   [`RenderRoot::perform_accessibility_action`] (the same synthesized
    ///   pointer-event path a real tap/click would take — see
    ///   `docs/ARCHITECTURE.md`'s Semantics pass); request a redraw if it
    ///   mutated state.
    /// * `AccessibilityDeactivated` — the AT client detached; nothing to clean
    ///   up (`Adapter::update_if_active` already becomes a no-op on its own).
    fn handle_accessibility_event(&mut self, event: AccessibilityEvent) {
        let Some(adapter) = self.adapter.as_mut() else {
            return;
        };
        match event.window_event {
            AccessibilityWindowEvent::InitialTreeRequested => {
                let update = self.root.semantics();
                self.semantics_seen = self.root.semantics_generation();
                adapter.update_if_active(|| build_tree_update(&update));
            }
            AccessibilityWindowEvent::ActionRequested(request) => {
                let outcome = self.root.perform_accessibility_action(
                    &mut self.state,
                    request.target_node,
                    request.action,
                );
                if outcome.needs_redraw
                    && let Some(window) = self.window.as_ref()
                {
                    window.request_redraw();
                }
            }
            AccessibilityWindowEvent::AccessibilityDeactivated => {}
        }
    }
}

/// Build an accesskit [`TreeUpdate`] from one [`RenderRoot::semantics`] pull
/// (phase 6d D3): a full-tree push every time (stable node ids make this valid
/// — see `frust_core::semantics`'s module docs), rooted with
/// [`TreeId::ROOT`] and the update's already-root-defaulted focus id
/// ([`SemanticsUpdate::focus_id`]). Pure and unit-testable without a live
/// window/adapter.
fn build_tree_update(update: &SemanticsUpdate) -> TreeUpdate {
    TreeUpdate {
        nodes: update.nodes.clone(),
        tree: Some(Tree::new(update.root)),
        tree_id: TreeId::ROOT,
        focus: update.focus_id(),
    }
}

impl<State, Logic, V> ApplicationHandler<ShellUserEvent> for ShellHandler<State, Logic, V>
where
    State: 'static,
    V: View<State>,
    Logic: FnMut(&mut State) -> V + 'static,
{
    /// A tracked-signal write from any thread routes here via the frame waker →
    /// [`winit::event_loop::EventLoopProxy::send_event`]. Pump the UI-thread
    /// local task queue first (a completing local task may have driven the
    /// write), then request a redraw so the next frame re-runs `app_logic` and
    /// re-tracks. The waker can fire before the window exists (an early
    /// background spawn), so a redraw is only requested when a window is present
    /// — the first rebuild after `resumed` re-tracks regardless.
    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: ShellUserEvent) {
        self.runtime.pump_local();
        match event {
            ShellUserEvent::SignalsDirty => {
                if let Some(window) = self.window.as_ref() {
                    window.request_redraw();
                }
            }
            ShellUserEvent::Accessibility(event) => self.handle_accessibility_event(event),
        }
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        // The window is created once and reused; the surface, by contrast, is
        // (re)created here every time we resume — `suspended()` tears it down,
        // mirroring Android's surfaceDestroyed/surfaceCreated so the §8.1
        // machine stays exercised on desktop too.
        if self.window.is_none() {
            // `with_visible(false)`: the accesskit_winit adapter must be created
            // before the window is ever shown (its documented contract — see
            // `Adapter::with_event_loop_proxy`), so the window stays hidden until
            // the adapter below is constructed, then is made visible.
            let attrs = WindowAttributes::default()
                .with_title(WINDOW_TITLE)
                .with_inner_size(INITIAL_SIZE)
                .with_visible(false);
            match event_loop.create_window(attrs) {
                Ok(window) => {
                    self.adapter = Some(Adapter::with_event_loop_proxy(
                        event_loop,
                        &window,
                        self.accesskit_proxy.clone(),
                    ));
                    window.set_visible(true);
                    self.window = Some(Arc::new(window));
                }
                Err(err) => {
                    self.fatal =
                        Some(anyhow::Error::from(err).context("frust: failed to create window"));
                    event_loop.exit();
                    return;
                }
            }
        }

        let window = self
            .window
            .clone()
            .expect("window was just created or already present");

        if self.renderer.phase() != SurfacePhase::SurfaceReady {
            let size = window.inner_size();
            let (width, height) = (size.width.max(1), size.height.max(1));

            // Load the persisted pipeline cache before surface creation (task 14).
            // On macOS (Metal) `pipeline_cache_data()` will be `None`, so this is
            // a no-op there; on Linux/Windows (Vulkan) it seeds the shader-pipeline
            // compilation to near-zero on warm starts.
            let cache_data = crate::cache::load_cache();
            // Warm-start signal for the first-frame decomposition (task 10.A):
            // a restored cache blob is a hit; its absence a cold miss.
            let pipeline_cache_hit = cache_data.is_some();
            if cache_data.is_some() {
                self.renderer.set_initial_pipeline_cache_data(cache_data);
            }

            // spec §11: single-threaded here — the CPU/GPU render-thread split
            // lands in a later phase; for the preview shell one thread suffices.
            if let Err(err) = pollster::block_on(self.renderer.on_surface_created(
                &mut self.render_cx,
                window.clone(),
                width,
                height,
            ))
            .context("frust: failed to create render surface")
            {
                self.fatal = Some(err);
                event_loop.exit();
                return;
            }

            // Save the updated pipeline cache on a background thread (task 14).
            // All failures are logged-and-ignored; cache is best-effort.
            if let Some(cache) = self.renderer.pipeline_cache_data() {
                std::thread::spawn(move || {
                    crate::cache::save_cache(&cache);
                });
            }

            // Perf instrumentation (task 10): adapter/device/renderer creation
            // all happen inside the single `on_surface_created` call above -
            // see `renderer_spans_recorded`'s docs for why all three spans land
            // at this one boundary. Only the first surface creation counts as
            // "startup"; a later suspend/resume recreation is not re-recorded.
            if !self.renderer_spans_recorded {
                self.startup_spans.record(SPAN_ADAPTER_READY);
                self.startup_spans.record(SPAN_DEVICE_READY);
                // First-frame decomposition (task 10.A): record the pipeline-cache
                // milestone only on a warm-start hit — its presence/absence in the
                // startup line attributes a slow first frame to shader-pipeline
                // compilation vs a warm cache (the `RENDERER_READY` delta that
                // follows brackets the surface/pipeline build cost).
                if pipeline_cache_hit {
                    self.startup_spans.record(SPAN_PIPELINE_CACHE_RESTORED);
                }
                self.startup_spans.record(SPAN_RENDERER_READY);
                self.renderer_spans_recorded = true;
            }
        }

        // Seed the theme once, before the first rebuild: read the window's
        // reported light/dark preference into the M3 baseline, then push it to
        // the render root and the reactive context (`apply_theme` also requests
        // the redraw that drives the first frame). Live changes arrive later via
        // `WindowEvent::ThemeChanged`.
        if !self.theme_seeded {
            self.theme.brightness = brightness_from_winit(window.theme());
            self.theme_seeded = true;
            self.apply_theme(&window);
        } else {
            window.request_redraw();
        }
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        // Drop the surface on suspend (spec §8.1): rare on macOS, but keeps the
        // NoSurface path exercised on desktop and matches Android's lifecycle.
        self.renderer.on_surface_destroyed();
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        // Drain any UI-thread local tasks that became runnable while dispatching
        // this batch of events, before the loop parks on `Wait`. Cheap no-op
        // when the queue is empty; a task that writes a signal here re-dirties a
        // scope and fires the waker, which re-arms the loop rather than parking.
        self.runtime.pump_local();
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(window) = self.window.clone() else {
            return;
        };

        // Every `WindowEvent` must reach the accesskit adapter (its documented
        // `process_event` contract — phase 6d D3): it derives root-window
        // bounds/focus state from `Moved`/`Resized`/`Focused` regardless of
        // what the match below does with the same event.
        if let Some(adapter) = self.adapter.as_mut() {
            adapter.process_event(&window, &event);
        }

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),

            WindowEvent::Resized(size) => {
                // A resize with no live surface is dropped by the machine.
                self.renderer
                    .on_surface_changed(&self.render_cx, size.width, size.height);
                window.request_redraw();
            }

            // Live dark-mode toggle: flip the theme's brightness and re-push it
            // to both delivery paths (`apply_theme` requests the repaint). The
            // next paint resolves the dark scheme through `PaintCtx::theme_as`
            // and the next rebuild sees the new `use_context::<Theme>()` value.
            //
            // Override-wins rule (task 6c-04): while an app-forced theme
            // override is active, this platform change must not flip
            // brightness — `effective_brightness_for_platform_change` passes
            // the current brightness through unchanged in that case.
            WindowEvent::ThemeChanged(winit_theme) => {
                self.theme.brightness = effective_brightness_for_platform_change(
                    self.theme_override_active,
                    self.theme.brightness,
                    brightness_from_winit(Some(winit_theme)),
                );
                self.apply_theme(&window);
            }

            // Track the cursor in logical space and dispatch a `Move`. We
            // dispatch on every move (not just while a button is down) so hover
            // handling can land later without a shell change; a widget that only
            // cares about drags simply ignores moves with no capture.
            WindowEvent::CursorMoved { position, .. } => {
                let scale = window.scale_factor();
                self.cursor = physical_to_logical(position.x, position.y, scale);
                self.dispatch(
                    &window,
                    InputEvent::Pointer(PointerEvent {
                        phase: PointerPhase::Move,
                        position: self.cursor,
                        button: PointerButton::Primary,
                    }),
                );
            }

            // Primary (left) button only in v1; other buttons are ignored until
            // secondary/middle gestures are specced. The press/release position
            // is the last `CursorMoved` position (winit carries none on the event).
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => {
                let phase = match state {
                    ElementState::Pressed => PointerPhase::Down,
                    ElementState::Released => PointerPhase::Up,
                };
                self.dispatch(
                    &window,
                    InputEvent::Pointer(PointerEvent {
                        phase,
                        position: self.cursor,
                        button: PointerButton::Primary,
                    }),
                );
            }

            // Wheel notches stay `Lines` (the ScrollView widget converts to px at
            // 40 px/line); precision-trackpad `PixelDelta` is physical, so divide
            // by the scale factor into logical pixels like every other coordinate.
            WindowEvent::MouseWheel { delta, .. } => {
                let scroll = match delta {
                    MouseScrollDelta::LineDelta(x, y) => ScrollDelta::Lines(x as f64, y as f64),
                    MouseScrollDelta::PixelDelta(px) => {
                        let scale = window.scale_factor();
                        ScrollDelta::Pixels(px.x / scale, px.y / scale)
                    }
                };
                self.dispatch(
                    &window,
                    InputEvent::Scroll {
                        position: self.cursor,
                        delta: scroll,
                    },
                );
            }

            // winit delivers `ModifiersChanged` *before* the `KeyboardInput`
            // that relies on it, so tracking it here keeps `self.modifiers`
            // current by the time a key event is mapped.
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = map_modifiers(modifiers.state());
            }

            // Not repeat-filtered — `repeat` passes through to the framework
            // event (task 54) so a widget can decide whether to honor
            // auto-repeat (e.g. held Backspace). Dropped entirely (`None`) on
            // key-up, an unmapped named key, or a composing-suppressed
            // character (see `map_key_event`'s dedupe rule).
            WindowEvent::KeyboardInput { event, .. } => {
                if let Some(key_event) = map_key_event(
                    &event.logical_key,
                    event.text.as_deref(),
                    event.state,
                    event.repeat,
                    self.modifiers,
                    self.compose.is_composing(),
                ) {
                    self.dispatch(&window, InputEvent::Key(key_event));
                }
            }

            // The compose latch both maps the winit variant and updates its own
            // dedupe state in one pass (`ComposeLatch::observe`).
            WindowEvent::Ime(ime) => {
                let mapped = self.compose.observe(&ime);
                self.dispatch(&window, InputEvent::Ime(mapped));
            }

            WindowEvent::RedrawRequested => {
                // Drain any UI-thread local tasks queued since the last turn
                // before rebuilding, so their signal writes are visible to this
                // frame. Cheap no-op when the queue is empty.
                self.runtime.pump_local();

                // Poll the app-facing theme override slot (task 6c-04) once
                // per frame, before rebuild — mirrors the mobile shells'
                // frame-callback poll. `Some(Some(theme))` is a new forced
                // theme; `Some(None)` is a `clear_app_theme` reverting to the
                // platform-derived default; `None` means nothing changed.
                match self.theme_override.poll() {
                    Some(Some(theme)) => {
                        self.theme = theme;
                        self.theme_override_active = true;
                        self.apply_theme(&window);
                    }
                    Some(None) => {
                        self.theme = Theme::m3_baseline();
                        self.theme.brightness = brightness_from_winit(window.theme());
                        self.theme_override_active = false;
                        self.apply_theme(&window);
                    }
                    None => {}
                }

                // Rebuild the view tree every frame (app_logic is cheap by
                // construction, spec §5). A real dirty-tracking loop would skip
                // this when state is unchanged; the on-demand `Wait` control
                // flow already keeps us from free-running.
                //
                // The rebuild runs under the reactive runtime's root `Owner`
                // (so signals created during it are root-owned) and inside the
                // `TrackedScope` (so every signal read subscribes this frame —
                // a later write dirties the scope and wakes the loop). Fields
                // are borrowed disjointly so the tracking closure captures only
                // what the rebuild needs, not all of `self`.
                let rebuild_start = Instant::now();
                let runtime = self.runtime;
                let scope = &self.scope;
                let root = &mut self.root;
                let app_logic = &mut self.app_logic;
                let state = &mut self.state;
                let _flags = runtime.with_owner(|| scope.track(|| root.rebuild(app_logic, state)));
                let rebuild_dur = rebuild_start.elapsed();
                // Perf instrumentation (task 10): the app's first-ever rebuild,
                // recorded once.
                if !self.first_rebuild_recorded {
                    self.startup_spans.record(SPAN_FIRST_REBUILD_DONE);
                    self.first_rebuild_recorded = true;
                }
                // A rebuild can change which widget is focused / what it
                // publishes without an intervening event (e.g. state-driven
                // focus), so re-sync the platform IME here too (task 54).
                self.sync_ime(&window);

                // HiDPI (spec task 08): lay out in logical pixels, then scale
                // the whole scene by the device pixel ratio so glyph outlines
                // are re-rasterised sharp at physical resolution.
                let physical = window.inner_size();
                let scale = window.scale_factor();
                let logical = Size::new(
                    physical.width as f64 / scale,
                    physical.height as f64 / scale,
                );
                let text_ctx: &mut dyn Any = &mut self.text_ctx;
                let layout_start = Instant::now();
                self.root.layout_with_text(logical, text_ctx);
                let layout_dur = layout_start.elapsed();

                // Push a fresh semantics tree to the accesskit adapter if it may
                // have changed since the last push (phase 6d D3's dirty gate —
                // `RenderRoot::semantics_if_changed`); post-layout so bounds are
                // valid. A no-op (`update_if_active` never runs the closure)
                // while no AT client has activated the adapter.
                if let Some(adapter) = self.adapter.as_mut()
                    && let Some(update) = self.root.semantics_if_changed(self.semantics_seen)
                {
                    self.semantics_seen = self.root.semantics_generation();
                    adapter.update_if_active(|| build_tree_update(&update));
                }

                self.scene.reset();
                // Sample the shell-owned monotonic clock once per frame and hand
                // it to paint; every animating widget differences it against its
                // own stored time (task 06 swaps this single expression for a
                // platform vsync timestamp on mobile).
                let frame_time = FrameTime::from_nanos(self.epoch.elapsed().as_nanos() as u64);
                let paint_start = Instant::now();
                let paint_outcome = {
                    let mut builder = SceneBuilder::new(&mut self.scene);
                    builder.push_transform(Affine::scale(scale));
                    let outcome = self.root.paint(&mut builder, frame_time);
                    builder.pop_transform();
                    outcome
                };
                let paint_dur = paint_start.elapsed();

                // Animation driver (spec v1 seam): if paint advanced animation
                // state (e.g. a scroll fling) it asks for another frame here.
                // `ControlFlow::Wait` would otherwise idle with no pending input,
                // so we keep frames coming with an explicit redraw request until
                // the animation reaches rest and stops signalling.
                if paint_outcome.needs_frame {
                    window.request_redraw();
                }

                // A tracked signal written *during* this frame (e.g. a local
                // task pumped above, or a write racing in from a background
                // thread) already re-dirtied the scope after `track` cleared it.
                // The waker's clean→dirty edge fired inside `track`, so no user
                // event will arrive for it — request the follow-up frame here,
                // mirroring the `needs_frame` animation path above.
                if self.scope.is_dirty() {
                    window.request_redraw();
                }

                window.pre_present_notify();
                // Deliberately log-and-continue rather than fatal: a single
                // frame's render failure is most often a transient GPU/surface
                // hiccup, and killing the whole app on one bad frame would be
                // worse than skipping it. Turning *persistent* per-frame
                // failures into a fatal error is future work — see `fatal`.
                // Clear to the live theme's surface color rather than a
                // hardcoded white, so a dark-scheme app doesn't render its
                // dark-themed widgets over a white canvas (6e Finding 6).
                // Two-phase render seam (Phase 10.A): time the GPU/CPU encode
                // and the swapchain-acquire (vsync) present separately so the
                // render-thread-split decision has an encode-only number. Each
                // span is its own `Instant` read; desktop's frame budget is
                // generous enough not to gate them behind `perf::enabled()` (see
                // docs/CODE_STANDARDS.md Instrumentation conventions).
                let encode_start = Instant::now();
                let encode_outcome =
                    self.renderer
                        .encode(&self.render_cx, &self.scene, self.theme.scheme().surface);
                let encode_dur = encode_start.elapsed();

                // First-frame decomposition (task 10.A): stamp the first frame's
                // encode-complete boundary once (only when something was actually
                // encoded), so the startup line splits the first frame into
                // paint/encode vs present. Its own latch (not `first_frame_recorded`)
                // because a first encode can precede the first present across an
                // intervening `Redraw` reconfigure frame.
                if matches!(encode_outcome, Ok(EncodeOutcome::Encoded)) && !self.first_encode_recorded {
                    self.startup_spans.record(SPAN_FIRST_ENCODE_DONE);
                    self.first_encode_recorded = true;
                }

                let present_start = Instant::now();
                let render_outcome = match encode_outcome {
                    Ok(EncodeOutcome::Encoded) => self.renderer.present(&self.render_cx),
                    // Nothing was encoded (no renderable surface): don't present,
                    // and surface the same `Skipped` the old combined `render`
                    // returned so the match below is unchanged.
                    Ok(EncodeOutcome::Skipped) => Ok(FrameOutcome::Skipped),
                    Err(err) => Err(err),
                };
                let present_dur = present_start.elapsed();

                // Perf instrumentation (task 10): one FramePasses record per
                // produced frame, piggybacking on this event-driven redraw path
                // rather than a timer (desktop is dirty-driven, spec §8) — see
                // `frame_stats`'s docs. `should_emit`/`emit_log` rate-limit the
                // periodic summary line to roughly once per 2s of frame time.
                self.frame_stats.record(FramePasses {
                    rebuild: rebuild_dur,
                    layout: layout_dur,
                    paint: paint_dur,
                    encode: encode_dur,
                    present: present_dur,
                    skipped: false,
                });
                if self.frame_stats.should_emit() {
                    self.frame_stats.emit_log();
                }

                match render_outcome {
                    // Stale swapchain (e.g. mid-resize): reconfigured internally,
                    // so ask for another frame against the fresh configuration.
                    Ok(FrameOutcome::Redraw) => window.request_redraw(),
                    // Surface lost (rare on desktop): recreate it from the same
                    // window and redraw. On failure, log and wait for the next
                    // event rather than killing the app.
                    Ok(FrameOutcome::SurfaceLost) => {
                        let size = window.inner_size();
                        let (width, height) = (size.width.max(1), size.height.max(1));
                        match pollster::block_on(self.renderer.on_surface_created(
                            &mut self.render_cx,
                            window.clone(),
                            width,
                            height,
                        )) {
                            Ok(()) => window.request_redraw(),
                            Err(err) => {
                                eprintln!("frust: failed to recreate surface: {err}");
                            }
                        }
                    }
                    Ok(FrameOutcome::Rendered) => {
                        // Perf instrumentation (task 10): the app's first-ever
                        // presented frame — record once, then emit the whole
                        // startup-span summary line.
                        if !self.first_frame_recorded {
                            self.startup_spans.record(SPAN_FIRST_FRAME_PRESENTED);
                            self.startup_spans.emit_log();
                            self.first_frame_recorded = true;
                        }
                    }
                    Ok(FrameOutcome::Skipped) => {}
                    Err(err) => eprintln!("frust: render error: {err}"),
                }
            }

            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ComposeLatch, ElementState, Ime, Tree, TreeId, WinitKey, WinitNamedKey, WinitTheme,
        brightness_from_winit, build_tree_update, finish, map_key_event, map_modifiers,
        map_named_key, physical_to_logical,
    };
    use frust_core::SemanticsUpdate;
    use frust_core::accesskit::{Node, NodeId, Role};
    use frust_core::event::{ImeEvent, Key, KeyEvent, Modifiers, NamedKey};
    use frust_theme::{Brightness, Theme};
    use kurbo::Point;
    use winit::keyboard::ModifiersState;

    // --- brightness_from_winit ---

    #[test]
    fn brightness_from_winit_maps_dark_light_and_defaults() {
        assert_eq!(
            brightness_from_winit(Some(WinitTheme::Dark)),
            Brightness::Dark
        );
        assert_eq!(
            brightness_from_winit(Some(WinitTheme::Light)),
            Brightness::Light
        );
        // No reported preference falls back to Light.
        assert_eq!(brightness_from_winit(None), Brightness::Light);
    }

    // --- provide_context replacement (the app-code theme delivery path) ---

    #[test]
    fn re_providing_theme_context_lets_a_live_flip_win() {
        // The desktop shell re-`provide_context`s the theme under the root owner
        // on every `ThemeChanged`. This proves reactive_graph 0.2's replacement
        // semantics: a second provide of the same type under one owner wins for
        // subsequent `use_context` reads — so a live dark-mode flip is observed
        // (no `RwSignal<Theme>` fallback needed; see the task's note).
        use frust_reactive::{Owner, provide_context, use_context};

        let owner = Owner::new();
        let resolved = owner.with(|| {
            let mut light = Theme::m3_baseline();
            light.brightness = Brightness::Light;
            provide_context(light);

            // A live flip: re-provide a dark theme under the same owner.
            let mut dark = Theme::m3_baseline();
            dark.brightness = Brightness::Dark;
            provide_context(dark);

            use_context::<Theme>()
        });
        assert_eq!(resolved.map(|t| t.brightness), Some(Brightness::Dark));
    }

    #[test]
    fn physical_to_logical_divides_by_scale() {
        // A 2× HiDPI display: a physical (200, 100) cursor is logical (100, 50).
        assert_eq!(
            physical_to_logical(200.0, 100.0, 2.0),
            Point::new(100.0, 50.0)
        );
    }

    #[test]
    fn physical_to_logical_is_identity_at_unit_scale() {
        // A non-HiDPI display: physical and logical coincide.
        assert_eq!(physical_to_logical(37.0, 12.0, 1.0), Point::new(37.0, 12.0));
    }

    #[test]
    fn finish_propagates_fatal_error_with_context() {
        let err = anyhow::anyhow!("boom").context("frust: failed to create window");
        let result = finish(Some(err));

        let err = result.expect_err("a stashed fatal error must surface as Err");
        assert_eq!(
            err.to_string(),
            "frust: failed to create window",
            "the outermost context string must be preserved"
        );
        assert_eq!(
            err.chain().last().unwrap().to_string(),
            "boom",
            "the underlying cause must still be reachable via the error chain"
        );
    }

    #[test]
    fn finish_is_ok_on_the_happy_path() {
        assert!(finish(None).is_ok());
    }

    // --- map_named_key ---

    #[test]
    fn map_named_key_covers_every_editing_named_key() {
        assert_eq!(map_named_key(WinitNamedKey::Enter), Some(NamedKey::Enter));
        assert_eq!(
            map_named_key(WinitNamedKey::Backspace),
            Some(NamedKey::Backspace)
        );
        assert_eq!(map_named_key(WinitNamedKey::Delete), Some(NamedKey::Delete));
        assert_eq!(
            map_named_key(WinitNamedKey::ArrowLeft),
            Some(NamedKey::ArrowLeft)
        );
        assert_eq!(
            map_named_key(WinitNamedKey::ArrowRight),
            Some(NamedKey::ArrowRight)
        );
        assert_eq!(
            map_named_key(WinitNamedKey::ArrowUp),
            Some(NamedKey::ArrowUp)
        );
        assert_eq!(
            map_named_key(WinitNamedKey::ArrowDown),
            Some(NamedKey::ArrowDown)
        );
        assert_eq!(map_named_key(WinitNamedKey::Home), Some(NamedKey::Home));
        assert_eq!(map_named_key(WinitNamedKey::End), Some(NamedKey::End));
        assert_eq!(map_named_key(WinitNamedKey::Escape), Some(NamedKey::Escape));
        assert_eq!(map_named_key(WinitNamedKey::Tab), Some(NamedKey::Tab));
    }

    #[test]
    fn map_named_key_is_none_for_unmapped_named_keys() {
        // No editing semantics for these — they must not be misreported.
        assert_eq!(map_named_key(WinitNamedKey::F1), None);
        assert_eq!(map_named_key(WinitNamedKey::Alt), None);
        assert_eq!(map_named_key(WinitNamedKey::CapsLock), None);
    }

    // --- map_modifiers ---

    #[test]
    fn map_modifiers_translates_each_flag_independently() {
        assert_eq!(
            map_modifiers(ModifiersState::SHIFT),
            Modifiers {
                shift: true,
                ctrl: false,
                alt: false,
                meta: false,
            }
        );
        assert_eq!(
            map_modifiers(ModifiersState::CONTROL | ModifiersState::SUPER),
            Modifiers {
                shift: false,
                ctrl: true,
                alt: false,
                meta: true,
            }
        );
        assert_eq!(map_modifiers(ModifiersState::empty()), Modifiers::default());
    }

    // --- map_key_event ---

    fn mods() -> Modifiers {
        Modifiers {
            shift: true,
            ..Modifiers::default()
        }
    }

    #[test]
    fn map_key_event_maps_a_named_key() {
        let event = map_key_event(
            &WinitKey::Named(WinitNamedKey::Backspace),
            None,
            ElementState::Pressed,
            false,
            mods(),
            false,
        );
        assert_eq!(
            event,
            Some(KeyEvent {
                key: Key::Named(NamedKey::Backspace),
                modifiers: mods(),
                repeat: false,
            })
        );
    }

    #[test]
    fn map_key_event_maps_resolved_text_as_character() {
        let event = map_key_event(
            &WinitKey::Character("a".into()),
            Some("a"),
            ElementState::Pressed,
            false,
            Modifiers::default(),
            false,
        );
        assert_eq!(
            event,
            Some(KeyEvent {
                key: Key::Character("a".to_string()),
                modifiers: Modifiers::default(),
                repeat: false,
            })
        );
    }

    #[test]
    fn map_key_event_passes_repeat_through_unfiltered() {
        let event = map_key_event(
            &WinitKey::Named(WinitNamedKey::ArrowLeft),
            None,
            ElementState::Pressed,
            true,
            Modifiers::default(),
            false,
        );
        assert!(event.expect("named key maps").repeat);
    }

    #[test]
    fn map_key_event_is_none_on_release() {
        let event = map_key_event(
            &WinitKey::Named(WinitNamedKey::Enter),
            None,
            ElementState::Released,
            false,
            Modifiers::default(),
            false,
        );
        assert_eq!(event, None);
    }

    #[test]
    fn map_key_event_drops_character_while_composing() {
        // The dedupe rule: a Preedit-active composition suppresses
        // KeyboardInput-derived Character events (Ime::Commit is authoritative).
        let event = map_key_event(
            &WinitKey::Character("a".into()),
            Some("a"),
            ElementState::Pressed,
            false,
            Modifiers::default(),
            true, // composing
        );
        assert_eq!(event, None);
    }

    #[test]
    fn map_key_event_still_maps_named_keys_while_composing() {
        // Only Character events are deduped; named editing keys pass through.
        let event = map_key_event(
            &WinitKey::Named(WinitNamedKey::Escape),
            None,
            ElementState::Pressed,
            false,
            Modifiers::default(),
            true, // composing
        );
        assert_eq!(
            event,
            Some(KeyEvent {
                key: Key::Named(NamedKey::Escape),
                modifiers: Modifiers::default(),
                repeat: false,
            })
        );
    }

    #[test]
    fn map_key_event_is_none_for_unmapped_named_key_with_no_text() {
        let event = map_key_event(
            &WinitKey::Named(WinitNamedKey::F1),
            None,
            ElementState::Pressed,
            false,
            Modifiers::default(),
            false,
        );
        assert_eq!(event, None);
    }

    // --- ComposeLatch / Ime mapping ---

    #[test]
    fn compose_latch_opens_on_nonempty_preedit() {
        let mut latch = ComposeLatch::default();
        assert!(!latch.is_composing());
        let mapped = latch.observe(&Ime::Preedit("n".to_string(), Some((0, 1))));
        assert!(latch.is_composing());
        assert_eq!(
            mapped,
            ImeEvent::Compose {
                text: "n".to_string(),
                cursor: Some((0, 1)),
            }
        );
    }

    #[test]
    fn compose_latch_closes_on_empty_preedit_preceding_commit() {
        let mut latch = ComposeLatch::default();
        latch.observe(&Ime::Preedit("n".to_string(), Some((0, 1))));
        assert!(latch.is_composing());

        let mapped = latch.observe(&Ime::Preedit(String::new(), None));
        assert!(!latch.is_composing());
        assert_eq!(
            mapped,
            ImeEvent::Compose {
                text: String::new(),
                cursor: None,
            }
        );
    }

    #[test]
    fn compose_latch_closes_on_commit() {
        let mut latch = ComposeLatch::default();
        latch.observe(&Ime::Preedit("n".to_string(), Some((0, 1))));
        let mapped = latch.observe(&Ime::Commit("\u{306a}".to_string()));
        assert!(!latch.is_composing());
        assert_eq!(mapped, ImeEvent::Commit("\u{306a}".to_string()));
    }

    #[test]
    fn compose_latch_closes_on_enabled_and_disabled() {
        let mut latch = ComposeLatch::default();
        latch.observe(&Ime::Preedit("n".to_string(), Some((0, 1))));

        let mapped = latch.observe(&Ime::Disabled);
        assert!(!latch.is_composing());
        assert_eq!(mapped, ImeEvent::Disabled);

        latch.observe(&Ime::Preedit("n".to_string(), Some((0, 1))));
        let mapped = latch.observe(&Ime::Enabled);
        assert!(!latch.is_composing());
        assert_eq!(mapped, ImeEvent::Enabled);
    }

    // --- build_tree_update (phase 6d D3) ---
    //
    // Pure-function coverage of the `SemanticsUpdate` -> `accesskit::TreeUpdate`
    // assembly; no winit event loop or live adapter needed.

    #[test]
    fn build_tree_update_assembles_nodes_tree_and_focus() {
        let root_id = NodeId(1);
        let child_id = NodeId(2);
        let mut root_node = Node::new(Role::Window);
        root_node.set_children(vec![child_id]);
        let child_node = Node::new(Role::Button);

        let update = SemanticsUpdate {
            nodes: vec![(root_id, root_node), (child_id, child_node)],
            root: root_id,
            focus: Some(child_id),
        };

        let tree_update = build_tree_update(&update);
        assert_eq!(tree_update.nodes.len(), 2);
        assert_eq!(tree_update.tree, Some(Tree::new(root_id)));
        assert_eq!(tree_update.tree_id, TreeId::ROOT);
        assert_eq!(tree_update.focus, child_id);
    }

    #[test]
    fn build_tree_update_defaults_focus_to_root_when_nothing_focused() {
        let root_id = NodeId(1);
        let update = SemanticsUpdate {
            nodes: vec![(root_id, Node::new(Role::Window))],
            root: root_id,
            focus: None,
        };

        let tree_update = build_tree_update(&update);
        // SemanticsUpdate::focus_id() defaults to root — accesskit's `focus`
        // field is non-optional and must always name some target.
        assert_eq!(tree_update.focus, root_id);
    }
}
