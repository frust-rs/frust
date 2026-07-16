//! The winit 0.30 [`ApplicationHandler`] that drives a ForgeKit app in a desktop
//! preview window (spec §12.9).
//!
//! Ownership mirrors the shared platform bootstrap (spec §10.3): the shell owns
//! the [`RenderContext`], per-window [`SurfaceRenderer`], the shared
//! [`TextContext`], the [`RenderRoot`], and the application `State`/`app_logic`.
//! Each on-demand frame runs the framework passes in Masonry order (the v0
//! subset): rebuild → layout → paint → render.

use std::any::Any;
use std::sync::Arc;

use anyhow::{Context, Result};
use forgekit_core::RenderRoot;
use forgekit_core::event::{
    ImeEvent, InputEvent, Key, KeyEvent, Modifiers, NamedKey, PointerButton, PointerEvent,
    PointerPhase, ScrollDelta,
};
use forgekit_core::view::View;
use forgekit_render::{FrameOutcome, RenderContext, SurfacePhase, SurfaceRenderer};
use forgekit_scene::{Scene, SceneBuilder};
use forgekit_text::TextContext;
use kurbo::{Affine, Point, Size};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalPosition, LogicalSize};
use winit::event::{ElementState, Ime, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key as WinitKey, ModifiersState, NamedKey as WinitNamedKey};
use winit::window::{Window, WindowAttributes, WindowId};

/// Initial preview-window size, in logical pixels.
const INITIAL_SIZE: LogicalSize<u32> = LogicalSize::new(800, 600);
const WINDOW_TITLE: &str = "ForgeKit";

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
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Wait);

    let mut handler = ShellHandler {
        state,
        app_logic,
        root: RenderRoot::new(),
        text_ctx: TextContext::new(),
        render_cx: RenderContext::new(),
        scene: Scene::new(),
        cursor: Point::ZERO,
        modifiers: Modifiers::default(),
        compose: ComposeLatch::default(),
        ime_sync: ImeSync::default(),
        window: None,
        // Starts in `NoSurface`; the surface is created in `resumed` once the
        // window exists, and driven through the §8.1 lifecycle from there.
        renderer: SurfaceRenderer::new(),
        fatal: None,
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
    /// The §8.1 surface lifecycle machine; empty (`NoSurface`) until `resumed()`
    /// creates the surface, and torn down on `suspended()`.
    renderer: SurfaceRenderer,
    /// Set when `resumed` hits an unrecoverable init error; `event_loop.exit()`
    /// only stops the loop (it can't return an `Err`), so the error is stashed
    /// here and re-raised by `run_desktop` once `run_app` returns.
    fatal: Option<anyhow::Error>,
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
}

impl<State, Logic, V> ApplicationHandler for ShellHandler<State, Logic, V>
where
    State: 'static,
    V: View<State>,
    Logic: FnMut(&mut State) -> V + 'static,
{
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        // The window is created once and reused; the surface, by contrast, is
        // (re)created here every time we resume — `suspended()` tears it down,
        // mirroring Android's surfaceDestroyed/surfaceCreated so the §8.1
        // machine stays exercised on desktop too.
        if self.window.is_none() {
            let attrs = WindowAttributes::default()
                .with_title(WINDOW_TITLE)
                .with_inner_size(INITIAL_SIZE);
            match event_loop.create_window(attrs) {
                Ok(window) => self.window = Some(Arc::new(window)),
                Err(err) => {
                    self.fatal =
                        Some(anyhow::Error::from(err).context("forgekit: failed to create window"));
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
            // spec §11: single-threaded here — the CPU/GPU render-thread split
            // lands in a later phase; for the preview shell one thread suffices.
            if let Err(err) = pollster::block_on(self.renderer.on_surface_created(
                &mut self.render_cx,
                window.clone(),
                width,
                height,
            ))
            .context("forgekit: failed to create render surface")
            {
                self.fatal = Some(err);
                event_loop.exit();
                return;
            }
        }

        window.request_redraw();
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        // Drop the surface on suspend (spec §8.1): rare on macOS, but keeps the
        // NoSurface path exercised on desktop and matches Android's lifecycle.
        self.renderer.on_surface_destroyed();
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

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),

            WindowEvent::Resized(size) => {
                // A resize with no live surface is dropped by the machine.
                self.renderer
                    .on_surface_changed(&self.render_cx, size.width, size.height);
                window.request_redraw();
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
                // Rebuild the view tree every frame (app_logic is cheap by
                // construction, spec §5). A real dirty-tracking loop would skip
                // this when state is unchanged; the on-demand `Wait` control
                // flow already keeps us from free-running.
                let _flags = self.root.rebuild(&mut self.app_logic, &mut self.state);
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
                self.root.layout_with_text(logical, text_ctx);

                self.scene.reset();
                let paint_outcome = {
                    let mut builder = SceneBuilder::new(&mut self.scene);
                    builder.push_transform(Affine::scale(scale));
                    let outcome = self.root.paint(&mut builder);
                    builder.pop_transform();
                    outcome
                };

                // Animation driver (spec v1 seam): if paint advanced animation
                // state (e.g. a scroll fling) it asks for another frame here.
                // `ControlFlow::Wait` would otherwise idle with no pending input,
                // so we keep frames coming with an explicit redraw request until
                // the animation reaches rest and stops signalling.
                if paint_outcome.needs_frame {
                    window.request_redraw();
                }

                window.pre_present_notify();
                // Deliberately log-and-continue rather than fatal: a single
                // frame's render failure is most often a transient GPU/surface
                // hiccup, and killing the whole app on one bad frame would be
                // worse than skipping it. Turning *persistent* per-frame
                // failures into a fatal error is future work — see `fatal`.
                match self
                    .renderer
                    .render(&self.render_cx, &self.scene, peniko::Color::WHITE)
                {
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
                                eprintln!("forgekit: failed to recreate surface: {err}");
                            }
                        }
                    }
                    Ok(FrameOutcome::Rendered | FrameOutcome::Skipped) => {}
                    Err(err) => eprintln!("forgekit: render error: {err}"),
                }
            }

            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ComposeLatch, ElementState, Ime, WinitKey, WinitNamedKey, finish, map_key_event,
        map_modifiers, map_named_key, physical_to_logical,
    };
    use forgekit_core::event::{ImeEvent, Key, KeyEvent, Modifiers, NamedKey};
    use kurbo::Point;
    use winit::keyboard::ModifiersState;

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
        let err = anyhow::anyhow!("boom").context("forgekit: failed to create window");
        let result = finish(Some(err));

        let err = result.expect_err("a stashed fatal error must surface as Err");
        assert_eq!(
            err.to_string(),
            "forgekit: failed to create window",
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
}
