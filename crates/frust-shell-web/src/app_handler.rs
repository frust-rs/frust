//! The winit-generic host-signal translation this shell is built from: input
//! mapping, the reactive-owner event wrap, theme delivery, and the
//! change-guarded window-metrics publish — plus the three seams a browser has
//! no counterpart for, kept as documented no-ops.
//!
//! # Why these are ports, not a dependency
//!
//! Every function below has a twin in `frust-shell-desktop`'s own
//! `app_handler`, and the twin is winit-generic: it reads a winit event or a
//! winit `Window` accessor and produces framework vocabulary, touching nothing
//! platform-specific. Depending on that crate to reach them is not an option —
//! it also pulls `accesskit_winit`, `pollster`, a render thread and
//! `frust-paths` cache I/O, none of which build for or make sense on
//! `wasm32-unknown-unknown`. So the generic halves are copied and the two
//! copies must not diverge in behaviour, the same contract the macOS and
//! Windows menu implementations already carry (see
//! `docs/SHELLS_ARCHITECTURE.md`).
//!
//! # Web-backend facts these ports rest on
//!
//! Each was read out of the pinned `winit 0.30.13`'s own web backend rather
//! than assumed:
//!
//! * `Window::scale_factor()` reports `window.devicePixelRatio`, so the
//!   physical→logical conversion and the metrics publish are the identical
//!   arithmetic desktop does.
//! * `Window::theme()` answers the `prefers-color-scheme: dark` media query and
//!   `WindowEvent::ThemeChanged` is emitted on a live change, so
//!   brightness-follow ports unchanged.
//! * `Window::set_cursor()` writes the canvas's CSS `cursor` property — a real
//!   implementation, unlike the three IME setters below.
//! * `WindowEvent::Ime` is **never** emitted by the web backend, so nothing
//!   currently feeds [`ComposeLatch`]; it ships anyway because it is the seam a
//!   real web IME bridge (a hidden editable element) would push into, and
//!   [`map_key_event`]'s dedupe rule is meaningless without it.
//! * `WindowEvent::Touch` *is* emitted, and is deliberately unmapped here —
//!   pointer/touch unification is a behaviour decision, not a translation, and
//!   inventing one would put this shell out of step with the mobile tier.

use std::sync::atomic::{AtomicBool, Ordering};

use frust_core::RenderRoot;
use frust_core::SemanticsUpdate;
use frust_core::event::{
    CursorIcon, EventOutcome, ImeEvent, InputEvent, Key, KeyEvent, Modifiers, NamedKey,
    PointerButton, PointerEvent, PointerPhase, ScrollDelta,
};
use frust_core::insets::WindowInsets;
use frust_core::view::View;
use frust_reactive::{ReactiveRuntime, provide_context};
use frust_shell_common::{WindowMetricsPublisher, effective_brightness_for_platform_change};
use frust_theme::{Brightness, Theme};
use kurbo::Point;
use winit::dpi::PhysicalPosition;
use winit::event::{ElementState, Ime, MouseButton, MouseScrollDelta};
use winit::keyboard::{Key as WinitKey, ModifiersState, NamedKey as WinitNamedKey};
use winit::window::{CursorIcon as WinitCursorIcon, Theme as WinitTheme, Window};

// --- input mapping -------------------------------------------------------

/// Convert a winit physical-pixel position into logical (density-independent)
/// pixels by the window's `scale_factor`.
///
/// winit reports pointer positions and `PixelDelta` scrolls in **physical**
/// pixels on every backend, the web included, where the scale factor is
/// `window.devicePixelRatio`. The widget tree lays out and hit-tests in the
/// logical space the layout pass uses, so every pointer coordinate is divided
/// by the scale factor at this boundary.
pub fn physical_to_logical(x: f64, y: f64, scale: f64) -> Point {
    Point::new(x / scale, y / scale)
}

/// Map a winit [`MouseScrollDelta`] to our [`ScrollDelta`], bridging both the
/// unit and the sign convention.
///
/// Units: wheel notches stay [`ScrollDelta::Lines`] (the scrolling widget
/// converts to px itself); a `PixelDelta` is physical, so it is divided by the
/// scale factor into logical pixels like every other coordinate. A browser
/// wheel event carries `deltaMode` line *or* pixel, and winit's web backend
/// maps each onto the matching variant, so both arms are live here — unlike on
/// a platform where only one ever occurs.
///
/// Sign: winit reports the direction the **content** moves — a positive `y`
/// moves the content down, revealing what sits above it. Frust's scrollable
/// widgets accumulate `offset + dy`, where a growing offset reveals *later*
/// content — the opposite sense, on both axes. Both are therefore negated
/// here, at the one boundary that knows winit's convention. The negation is
/// load-bearing: dropping it inverts scrolling in the whole app.
pub fn map_scroll_delta(delta: MouseScrollDelta, scale: f64) -> ScrollDelta {
    match delta {
        MouseScrollDelta::LineDelta(x, y) => ScrollDelta::Lines(-(x as f64), -(y as f64)),
        MouseScrollDelta::PixelDelta(px) => ScrollDelta::Pixels(-px.x / scale, -px.y / scale),
    }
}

/// Map a winit [`MouseButton`] to our [`PointerButton`] vocabulary, or `None`
/// for a button this shell forwards nothing for yet (Middle/Back/Forward —
/// dropped rather than misreported, matching the desktop core exactly).
pub fn map_mouse_button(button: MouseButton) -> Option<PointerButton> {
    match button {
        MouseButton::Left => Some(PointerButton::Primary),
        MouseButton::Right => Some(PointerButton::Secondary),
        _ => None,
    }
}

/// Whether a mapped button's press/release should reach the tree at all, given
/// the phase, whether a pointer gesture is currently captured
/// ([`RenderRoot::is_pointer_captured`]), and the delivery latch this gate owns
/// (threaded in by `&mut`).
///
/// Primary is never gated: it is the button a capture belongs to, and its whole
/// gesture must reach the tree.
///
/// Secondary is gated to keep two invariants at once:
///
/// - **A secondary press never disturbs a live capture.** `RenderRoot` tracks
///   capture as a single root-level flag, not one per button, and releases it
///   on phase alone. A secondary `Down` routed to a mid-drag capturer would
///   hand it a transition it never asked for, and a secondary `Up` would clear
///   the flag out from under a still-live primary gesture.
/// - **Pairing: the tree never sees an unpaired secondary event.** A widget's
///   press contract is a `Down` followed by its own `Up`, so dropping a release
///   whose press *was* delivered is as wrong as delivering a release whose
///   press was not — a stateless "drop while captured" rule does exactly that
///   when a capture opens between the two. The latch records what happened to
///   the `Down`, and the `Up` follows it regardless of the capture flag by then.
pub fn mouse_button_should_dispatch(
    button: PointerButton,
    phase: PointerPhase,
    pointer_captured: bool,
    secondary_down_delivered: &mut bool,
) -> bool {
    if button != PointerButton::Secondary {
        return true;
    }
    match phase {
        PointerPhase::Down => {
            let deliver = !pointer_captured;
            *secondary_down_delivered = deliver;
            deliver
        }
        PointerPhase::Up => {
            let deliver = *secondary_down_delivered;
            *secondary_down_delivered = false;
            deliver
        }
        // A mouse-button event produces only the two phases above; a pointer
        // `Move`/`Cancel` is built elsewhere and never reaches this gate.
        PointerPhase::Move | PointerPhase::Cancel => true,
    }
}

/// Map a winit [`WinitNamedKey`] to our editing-semantics [`NamedKey`] set, or
/// `None` for a named key this shell carries no editing semantics for (function
/// keys, media keys, browser keys) — those are dropped rather than misreported
/// as text.
pub fn map_named_key(key: WinitNamedKey) -> Option<NamedKey> {
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
pub fn map_modifiers(state: ModifiersState) -> Modifiers {
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
/// Takes the individual fields off `winit::event::KeyEvent` rather than the
/// struct itself, whose `platform_specific` field is private to winit and so
/// cannot be constructed outside it — which is what keeps this a pure,
/// directly unit-testable function on the build host.
///
/// Named keys map through [`map_named_key`]; anything else falls back to the
/// key's resolved `text` (already dead-key resolved by the platform) as
/// [`Key::Character`]. The spacebar is the one named key that resolves to a
/// character instead: winit models it as [`WinitNamedKey::Space`] (never
/// `Character(" ")`), but it types a space rather than carrying editing
/// semantics, so it takes the character path with its `text` payload — falling
/// back to `" "` where none is sent. Either character path is dropped **while**
/// `composing` is set: an active preedit makes the commit, not the key event,
/// the authoritative source for composed text. `repeat` passes through
/// unfiltered — callers decide whether to honour auto-repeat. Only key-down
/// (`Pressed`) events map; releases produce `None`, since [`KeyEvent`] has no
/// up/down phase.
pub fn map_key_event(
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
        // Space is a named key that types text, so it resolves to a character
        // ahead of the editing-semantics mapping below — `map_named_key`
        // carries no `NamedKey` for it, and letting it fall through there would
        // drop the event and leave every text widget unable to receive a space.
        WinitKey::Named(WinitNamedKey::Space) => {
            if composing {
                return None;
            }
            Key::Character(text.unwrap_or(" ").to_string())
        }
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

/// Tracks whether an IME preedit (marked-text) composition is in progress,
/// purely from a stream of winit [`Ime`] variants — the dedupe latch
/// [`map_key_event`] consults so keyboard-derived `Character` events are
/// suppressed while a composition owns the text.
///
/// **Nothing feeds this on the web today.** winit's web backend emits no
/// `WindowEvent::Ime` at all (verified against the pinned 0.30.13), so a
/// browser build currently sees `is_composing() == false` forever and no key
/// event is ever suppressed. It is here because [`InputState::ime`] is the
/// documented push point for a real web IME bridge — a hidden contenteditable
/// or `<input>` driven from `compositionstart`/`compositionupdate`/
/// `compositionend` — and that bridge needs exactly this latch to avoid
/// double-inserting composed text.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ComposeLatch {
    active: bool,
}

impl ComposeLatch {
    /// Observe one [`Ime`] variant, updating the latch and mapping it to our
    /// [`ImeEvent`] in the same pass — the mapping and the dedupe-state
    /// transition are the same decision, so they live together.
    ///
    /// An empty-text `Preedit` closes the latch (per winit's contract it
    /// precedes a `Commit`); `Commit`/`Enabled`/`Disabled` also close it, each
    /// ending or restarting a composition session.
    pub fn observe(&mut self, ime: &Ime) -> ImeEvent {
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

    /// Whether a keyboard-derived `Character` event should be dropped because a
    /// composition is currently in progress.
    pub fn is_composing(&self) -> bool {
        self.active
    }
}

/// The per-window input state a browser shell carries between events, and the
/// one place a winit `WindowEvent` becomes an [`InputEvent`].
///
/// The desktop core keeps these four values as loose fields on its event-loop
/// handler; grouped here instead because this crate has no handler yet — the
/// event loop is a separate concern from the translation, and keeping the
/// translation in a plain struct is what lets every arm below be exercised on
/// the build host with no browser and no live window.
///
/// The methods intentionally *return* events rather than dispatching them: what
/// to do with an event (dispatch under the root owner, request a redraw,
/// re-sync the cursor) is the frame loop's decision, not the mapper's.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct InputState {
    cursor: Point,
    modifiers: Modifiers,
    secondary_down_delivered: bool,
    compose: ComposeLatch,
}

impl InputState {
    /// A fresh state: pointer at the origin, no modifiers held, no secondary
    /// press outstanding, not composing.
    pub fn new() -> Self {
        Self::default()
    }

    /// The last pointer position seen, in logical pixels. A button press or
    /// release carries no position of its own, so it reuses this — mirroring
    /// how winit models the two events.
    pub fn cursor_position(&self) -> Point {
        self.cursor
    }

    /// The modifier chord currently held.
    pub fn modifiers(&self) -> Modifiers {
        self.modifiers
    }

    /// Whether an IME composition is in progress — see [`ComposeLatch`] for why
    /// this is always `false` on the web until a real IME bridge lands.
    pub fn is_composing(&self) -> bool {
        self.compose.is_composing()
    }

    /// A `CursorMoved`: record the new logical position and produce the `Move`.
    ///
    /// A move is produced unconditionally, not only while a button is down, so
    /// hover and cursor-shape resolution work; a widget that only cares about
    /// drags ignores moves with no capture.
    pub fn pointer_moved(&mut self, position: PhysicalPosition<f64>, scale: f64) -> InputEvent {
        self.cursor = physical_to_logical(position.x, position.y, scale);
        InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Move,
            position: self.cursor,
            button: PointerButton::Primary,
        })
    }

    /// A `MouseInput`: map the button and phase, apply the secondary-delivery
    /// gate, and produce the event at the last known pointer position.
    ///
    /// `None` means the event is deliberately not forwarded — an unmapped
    /// button ([`map_mouse_button`]) or a secondary press/release the gate
    /// dropped ([`mouse_button_should_dispatch`]).
    pub fn mouse_input(
        &mut self,
        state: ElementState,
        button: MouseButton,
        pointer_captured: bool,
    ) -> Option<InputEvent> {
        let phase = match state {
            ElementState::Pressed => PointerPhase::Down,
            ElementState::Released => PointerPhase::Up,
        };
        let mapped = map_mouse_button(button)?;
        if !mouse_button_should_dispatch(
            mapped,
            phase,
            pointer_captured,
            &mut self.secondary_down_delivered,
        ) {
            return None;
        }
        Some(InputEvent::Pointer(PointerEvent {
            phase,
            position: self.cursor,
            button: mapped,
        }))
    }

    /// A `MouseWheel`: both the unit and the sign conversion live in
    /// [`map_scroll_delta`]; the scroll is anchored at the last pointer
    /// position, as winit carries none on the event.
    pub fn mouse_wheel(&self, delta: MouseScrollDelta, scale: f64) -> InputEvent {
        InputEvent::Scroll {
            position: self.cursor,
            delta: map_scroll_delta(delta, scale),
        }
    }

    /// A `ModifiersChanged`: winit delivers it *before* the `KeyboardInput`
    /// that relies on it, so tracking it here keeps the chord current by the
    /// time [`InputState::keyboard_input`] maps a key.
    pub fn modifiers_changed(&mut self, state: ModifiersState) {
        self.modifiers = map_modifiers(state);
    }

    /// A `KeyboardInput`, taken as winit's own field set (see
    /// [`map_key_event`] for why the struct itself cannot be passed). `None`
    /// on key-up, on an unmapped named key, or on a character suppressed by an
    /// active composition.
    pub fn keyboard_input(
        &self,
        logical_key: &WinitKey,
        text: Option<&str>,
        state: ElementState,
        repeat: bool,
    ) -> Option<InputEvent> {
        map_key_event(
            logical_key,
            text,
            state,
            repeat,
            self.modifiers,
            self.compose.is_composing(),
        )
        .map(InputEvent::Key)
    }

    /// One IME transition: the latch both maps the variant and updates its own
    /// dedupe state in a single pass. See [`ComposeLatch`] — winit's web
    /// backend never calls this today, and it exists for the IME bridge that
    /// will.
    pub fn ime(&mut self, ime: &Ime) -> InputEvent {
        InputEvent::Ime(self.compose.observe(ime))
    }
}

// --- the reactive-owner event wrap ---------------------------------------

/// Deliver one input event to the widget tree **under the reactive runtime's
/// root `Owner`**, so context lookups work from inside an event handler.
///
/// # Why the wrap is needed
///
/// `reactive_graph`'s `use_context` resolves by walking up from the current
/// owner, and `Owner::with` *restores* the previous owner when it returns — so
/// outside the per-frame rebuild wrap there is no current owner at all. An
/// unwrapped event pass therefore gives every handler `use_context::<T>() ==
/// None`, even for a context the shell itself provided (`Theme`,
/// `WindowMetrics`): the whole app looks unthemed from a press handler.
///
/// # Owner identity: the root owner, not a fresh child
///
/// The pass shares the rebuild's owner rather than opening a child scope. A
/// child scope would have to be created and disposed per input event —
/// including per pointer `Move` — and a handler's `provide_context` would
/// silently evaporate on dispose instead of being visible to the next rebuild.
///
/// # No tracked scope: deliberately untracked
///
/// The rebuild wrap pairs `with_owner` with a tracked scope; this one must
/// **not**. Tracking clears the scope's recorded sources and its dirty flag on
/// entry, so tracking an event pass would throw away the dependency set the
/// last rebuild recorded — silently unsubscribing the frame loop from every
/// signal the view reads — and swallow a wake that arrived since. A handler
/// that *writes* a signal still wakes the shell: the write notifies the rebuild
/// scope, which is subscribed from its own tracking pass.
pub fn event_under_owner<State: 'static, V: View<State>>(
    runtime: &ReactiveRuntime,
    root: &mut RenderRoot<State, V>,
    state: &mut State,
    event: &InputEvent,
) -> EventOutcome {
    runtime.with_owner(|| root.event(state, event))
}

// --- window metrics ------------------------------------------------------

/// `provide_context` the window's `WindowMetrics` under the reactive root owner
/// — **only when it actually changed** ([`WindowMetricsPublisher::poll`]
/// returns `None` otherwise) — so a `Component::build`'s
/// `use_context::<WindowMetrics>()` resolves the current shape on the next
/// rebuild. Returns whether anything was published.
///
/// **The guard is load-bearing, not an optimization.** `provide_context` is a
/// plain map insert that notifies nothing on its own, so re-providing once per
/// frame would pay a lock write plus an allocation for no observable benefit —
/// the value only becomes visible on the next rebuild, which a genuine input
/// change already drives. Call this only where an input genuinely moves: the
/// canvas's first sizing, and every `Resized`. A `ScaleFactorChanged` needs no
/// separate call — winit guarantees a following `Resized`, on the web backend
/// as everywhere else, which is what makes a browser zoom or a monitor change
/// republish through the same one path.
///
/// `physical` is winit's device-pixel `inner_size`, divided by `scale` (the
/// browser's `devicePixelRatio`) into the logical space the layout pass uses,
/// so the published size is logical exactly as on every other shell.
///
/// Insets are the default — zero occlusion. The browser does expose safe-area
/// values, but only to CSS (`env(safe-area-inset-*)`) and only inside a
/// display-mode that has them; nothing in winit surfaces them, so this shell is
/// in the same position the desktop core is, and reading them would mean the
/// host page pushing them in through a seam that does not exist yet.
///
/// Unlike [`apply_theme`] this requests no redraw of its own: the call sites
/// that move the window shape already drive one, and a metrics publish that
/// forced a frame would defeat the guard's purpose.
pub fn publish_window_metrics(
    runtime: &ReactiveRuntime,
    publisher: &mut WindowMetricsPublisher,
    physical: (u32, u32),
    scale: f64,
) -> bool {
    let Some(metrics) = publisher.poll(physical, scale, WindowInsets::default()) else {
        return false; // unchanged — no re-provide, no app-wide rebuild
    };
    runtime.with_owner(move || provide_context(metrics));
    true
}

// --- theme ---------------------------------------------------------------

/// Map winit's window [`WinitTheme`] (`None` when the platform reports no
/// preference) to a [`Brightness`], defaulting to [`Brightness::Light`].
///
/// On the web the source is the `prefers-color-scheme: dark` media query, which
/// winit answers `Window::theme()` from; a browser that matches neither state,
/// or one where the query is unavailable, lands on the same `Light` default
/// every other host uses.
pub fn brightness_from_winit(theme: Option<WinitTheme>) -> Brightness {
    match theme {
        Some(WinitTheme::Dark) => Brightness::Dark,
        _ => Brightness::Light,
    }
}

/// The base theme this shell seeds from: a design system's own
/// `set_default_theme` value when one was installed, else the built-in neutral
/// fallback.
///
/// Takes the slot's value as an argument rather than reading the process-global
/// itself, so the fallback ladder is testable without touching a slot that has
/// no reset; call sites pass `frust_shell_common::default_theme()`.
pub fn base_theme(seeded: Option<Theme>) -> Theme {
    seeded.unwrap_or_else(Theme::neutral)
}

/// The theme a cleared app-theme override reverts to: the seeded base
/// ([`base_theme`]) at the platform's *current* brightness, never the cleared
/// override's own pinned one.
pub fn reverted_theme(seeded: Option<Theme>, platform: Brightness) -> Theme {
    let mut theme = base_theme(seeded);
    theme.brightness = platform;
    theme
}

/// The active-theme decision for one theme-override poll — the precedence
/// ladder's top two rungs as one pure function.
///
/// Returns `None` when the poll reported no change (the shell leaves its theme
/// alone), else the new active theme paired with whether an app-forced override
/// is now pinning it.
///
/// `seeded`/`platform` are suppliers rather than values because only the
/// cleared-override arm needs them: reading the process-global default slot (a
/// lock plus a whole-`Theme` clone) and querying the window's reported
/// appearance would otherwise become per-frame cost for a poll that reports
/// "nothing changed" on all but a handful of frames.
pub fn theme_after_override_poll(
    polled: Option<Option<Theme>>,
    seeded: impl FnOnce() -> Option<Theme>,
    platform: impl FnOnce() -> Brightness,
) -> Option<(Theme, bool)> {
    match polled {
        // A forced theme wins wholesale — neither the seeded default nor the
        // platform's brightness is consulted.
        Some(Some(theme)) => Some((theme, true)),
        // Cleared: back to the seeded base at the platform's own brightness.
        Some(None) => Some((reverted_theme(seeded(), platform()), false)),
        None => None,
    }
}

/// Re-derive `theme`'s brightness from a platform appearance report, honouring
/// the override-wins rule: an app-forced override pins brightness, a
/// design-system-seeded default does not — `theme` still *is* that base, so
/// flipping it in place re-derives light/dark against the design system's own
/// tokens.
///
/// This is the whole of what a `WindowEvent::ThemeChanged` needs on the web:
/// the browser's `prefers-color-scheme` flip arrives as the same winit event
/// desktop gets, so the ladder ports without a web-specific arm.
pub fn follow_platform_brightness(theme: &mut Theme, override_active: bool, platform: Brightness) {
    theme.brightness =
        effective_brightness_for_platform_change(override_active, theme.brightness, platform);
}

/// Push a resolved [`Theme`] down both delivery paths at once: into
/// [`RenderRoot`]'s type-erased slot, so widgets read it through their paint and
/// layout contexts, and as a `provide_context` clone under the reactive root
/// owner, so app code reads it through `use_context::<Theme>()`.
///
/// Two paths rather than one because they serve different readers, and both
/// must move together or a live brightness flip repaints against one theme
/// while `use_context` still answers the other. Re-providing the same type
/// under one owner replaces it, which is what makes the flip observable rather
/// than needing a signal.
///
/// Requests no redraw: this crate does not own the frame loop, and the caller
/// that resolved the new theme is the one that knows whether a frame is already
/// coming.
pub fn apply_theme<State, V>(
    runtime: &ReactiveRuntime,
    root: &mut RenderRoot<State, V>,
    theme: &Theme,
) where
    State: 'static,
    V: View<State>,
{
    root.set_theme(Box::new(theme.clone()));
    let published = theme.clone();
    runtime.with_owner(move || provide_context(published));
}

// --- cursor --------------------------------------------------------------

/// Map a resolved framework [`CursorIcon`] onto winit's own cursor vocabulary.
///
/// The one place in this shell where a cursor name touches a platform. On the
/// web, winit writes the mapped icon into the canvas's CSS `cursor` property,
/// and winit's icon names come from `cursor-icon`, whose names follow CSS — so
/// every variant round-trips exactly and nothing here approximates.
///
/// The wildcard arm is not dead code: [`CursorIcon`] is `#[non_exhaustive]`, so
/// a variant added later must compile here and *degrade* to the platform arrow
/// rather than break the build or invent a shape.
pub fn winit_cursor_for(icon: CursorIcon) -> WinitCursorIcon {
    match icon {
        CursorIcon::Default => WinitCursorIcon::Default,
        CursorIcon::Pointer => WinitCursorIcon::Pointer,
        CursorIcon::Text => WinitCursorIcon::Text,
        CursorIcon::Grab => WinitCursorIcon::Grab,
        CursorIcon::Grabbing => WinitCursorIcon::Grabbing,
        CursorIcon::ColResize => WinitCursorIcon::ColResize,
        CursorIcon::RowResize => WinitCursorIcon::RowResize,
        CursorIcon::NotAllowed => WinitCursorIcon::NotAllowed,
        _ => WinitCursorIcon::Default,
    }
}

/// The winit cursor (if any) to push, given the shape last pushed and the one
/// [`RenderRoot::cursor`] now resolves to.
///
/// `None` means "say nothing to winit": the resolved cursor re-resolves on every
/// pointer `Move`, so an unguarded push would write the canvas's CSS `cursor`
/// property on every single mouse motion for a value that had not moved.
pub fn cursor_change_to_apply(last: CursorIcon, current: CursorIcon) -> Option<WinitCursorIcon> {
    (last != current).then(|| winit_cursor_for(current))
}

/// Push the cursor shape the tree resolved to, but only when it differs from
/// the last one pushed; `last` is updated in place when it does.
///
/// Unlike the three seams below this is a **real** implementation, not a
/// no-op: winit's web backend implements `Window::set_cursor` by writing the
/// canvas element's CSS `cursor` property, so the desktop core's
/// request/resolve contract carries over to the browser unchanged.
///
/// Deliberately not tied to a frame — a cursor is a window property, not
/// something painted — so a hover that changes nothing but the shape costs one
/// DOM write and no repaint.
pub fn sync_cursor(window: &Window, last: &mut CursorIcon, resolved: CursorIcon) {
    if let Some(winit_icon) = cursor_change_to_apply(*last, resolved) {
        window.set_cursor(winit_icon);
        *last = resolved;
    }
}

// --- host signals a browser does not have --------------------------------

/// Report an unsupported host signal exactly once per process.
///
/// Once, not per call: every seam below sits on a per-event or per-frame path,
/// and a browser console that repeats the same line sixty times a second buries
/// the diagnostics that matter. Once is enough to answer "why does my text
/// field get no IME?" and cheap enough to leave in a release build.
fn report_gap_once(reported: &AtomicBool, gap: &str) {
    if !reported.swap(true, Ordering::Relaxed) {
        log::debug!("frust-shell-web: {gap}");
    }
}

static IME_GAP_REPORTED: AtomicBool = AtomicBool::new(false);
static SEMANTICS_GAP_REPORTED: AtomicBool = AtomicBool::new(false);
static DEVTOOLS_GAP_REPORTED: AtomicBool = AtomicBool::new(false);

/// Push the focused widget's IME editing state to the platform input method —
/// a documented **no-op** in a browser.
///
/// The desktop core drives `set_ime_allowed`, `set_ime_cursor_area` and
/// `set_ime_purpose` here. All three are no-ops in the pinned winit's web
/// backend (its own source says so at each one), and the backend emits no
/// `WindowEvent::Ime` for them to pair with, so calling them would be theatre:
/// the framework would believe it had armed an input method that does not
/// exist.
///
/// A real web IME is a hidden editable element the shell focuses and positions,
/// bridging `compositionstart`/`compositionupdate`/`compositionend` into
/// [`InputState::ime`]. That is a browser-DOM job with its own consent and
/// focus-stealing questions, not a translation, and it is deliberately not
/// invented here — this seam exists so the eventual bridge has a call site
/// already in the loop, and so the gap is *stated* rather than looking like an
/// oversight.
pub fn sync_ime(_window: &Window) {
    report_gap_once(
        &IME_GAP_REPORTED,
        "platform IME is unavailable in a browser (winit's web backend implements no IME \
         setters and emits no IME events); text input works, composed input does not",
    );
}

/// Publish the semantics tree to an assistive-technology client — a documented
/// **no-op** in a browser.
///
/// The desktop core hands each pass to an `accesskit_winit` adapter. AccessKit
/// ships no web adapter, and it could not be a drop-in one if it did: the
/// browser's accessibility tree is the DOM, so a canvas-rendered app is opaque
/// to a screen reader until the shell mirrors its semantics into real ARIA
/// elements. That mirror is a whole subsystem — element lifecycle, focus
/// ownership, hit-test correspondence — and stubbing it here keeps the gap
/// visible instead of shipping a shell that silently reports nothing.
///
/// The `scale` argument is carried even though nothing is published, because it
/// is the conversion the desktop adapter needs (semantics report logical
/// pixels, accessibility APIs want physical) and a real implementation will
/// need it at the same call site.
pub fn push_semantics(_update: &SemanticsUpdate, _scale: f64) {
    report_gap_once(
        &SEMANTICS_GAP_REPORTED,
        "accessibility is not published in a browser (no AccessKit web adapter; a canvas app \
         needs its semantics mirrored into real DOM/ARIA elements)",
    );
}

/// Drain the in-app devtools service's UI-thread hop — a documented **no-op**
/// in a browser.
///
/// The devtools service is a loopback TCP listener that `frust-drive` and
/// `frust-tui` connect to; a `wasm32-unknown-unknown` build has no sockets and
/// no listener to drain. A web equivalent would have to be a different
/// transport entirely (the page's own WebSocket back to the dev server), which
/// is a protocol decision rather than a port, so nothing is faked here.
///
/// This shell therefore forwards no `devtools` cargo feature: a feature that
/// compiled the service in but could never accept a connection would be worse
/// than its absence.
pub fn pump_devtools() {
    report_gap_once(
        &DEVTOOLS_GAP_REPORTED,
        "the in-app devtools service is unavailable in a browser (its loopback listener needs \
         sockets a wasm32 build does not have)",
    );
}

// --- the browser frame loop ----------------------------------------------

/// The browser's frame loop: window creation, the asynchronous surface
/// bring-up, the reactive wake path, and the `rebuild → layout → paint →
/// encode → present` turn, all of it `wasm32`-only.
///
/// Everything above this point in the file is winit-generic translation that
/// compiles, runs and is unit-tested on the build host. Everything in here
/// owns a browser resource — a canvas, `requestAnimationFrame`, the browser's
/// task scheduler — and there is no non-browser host of this crate for it to
/// serve, so it is gated rather than stubbed. The decisions it makes are not
/// gated: they live in [`crate::pacing`] and [`crate::render`] as pure
/// functions with host-side tests, and this module only applies them.
#[cfg(target_arch = "wasm32")]
mod browser_loop {
    use std::any::Any;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    use std::sync::Arc;

    use frust_core::view::View;
    use frust_core::{FrameTime, RenderRoot};
    use frust_reactive::{FrameWaker, ReactiveRuntime, TrackedScope};
    use frust_scene::{Scene, SceneBuilder};
    use frust_shell_common::font_registry::FontRegistryWatcher;
    use frust_shell_common::{
        ThemeOverrideWatcher, WindowMetricsPublisher, anim_pacing_kill_switch_engaged,
        default_theme,
    };
    use frust_text::TextContext;
    use frust_theme::Theme;
    use kurbo::{Affine, Size};
    use web_time::Instant;
    use winit::application::ApplicationHandler;
    use winit::dpi::PhysicalSize;
    use winit::error::EventLoopError;
    use winit::event::WindowEvent;
    use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
    use winit::platform::web::{EventLoopExtWebSys, WindowAttributesExtWebSys};
    use winit::window::{Window, WindowId};

    use crate::pacing::{
        ControlFlowIntent, OVERSHOOT_LOG_THRESHOLD, next_paced_wake, overshoot, paced_wake_action,
    };
    use crate::render::{FrameFollowUp, WebFrameExecutor};

    /// The canvas this shell asks winit for when nothing else sized it, in
    /// physical pixels.
    ///
    /// A placeholder, not a policy: winit creates its own canvas here and
    /// appends it to the page body, and an un-sized `<canvas>` element falls
    /// back to HTML's own 300x150 default, which is too small to see an app
    /// in. Adopting a host page's canvas at its real device-pixel ratio is
    /// the canvas-binding work that replaces this.
    const DEFAULT_CANVAS_SIZE: PhysicalSize<u32> = PhysicalSize::new(800, 600);

    /// The one thing a browser event loop is ever woken *out of band* for: a
    /// tracked signal was written, so the view must be rebuilt.
    ///
    /// A single variant where the desktop core has five. There is no render
    /// thread to route a surface re-creation back from, no `accesskit` adapter
    /// to service, and no devtools transport, so the four other desktop
    /// user-events have no counterpart here. It stays an `enum` rather than
    /// `()` so the loop's wake vocabulary is nameable and extending it later
    /// is not a breaking change to the event type.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum ShellUserEvent {
        /// A tracked signal was written; rebuild and repaint.
        SignalsDirty,
    }

    thread_local! {
        /// The live event loop's proxy, reachable from a waker closure that
        /// captures nothing.
        ///
        /// `frust_reactive::FrameWaker` is `Arc<dyn Fn() + Send + Sync>`, and
        /// winit's *web* `EventLoopProxy` is neither `Send` nor `Sync` (it
        /// holds an `Rc`-backed runner handle and an `mpsc::Sender`), so a
        /// closure that captured one directly would not satisfy that bound.
        /// The desktop core has no such problem — its proxies really do cross
        /// real OS threads.
        ///
        /// Parking the proxy in a thread-local and capturing *nothing* closes
        /// the gap without `unsafe`: the alternative, an `unsafe impl
        /// Send + Sync` wrapper asserting "wasm has no threads", would make
        /// this the only shell crate outside the sanctioned platform-FFI zones
        /// listed in `docs/CODE_STANDARDS.md` to carry `unsafe` at all. This
        /// crate stays `unsafe`-free instead, and the single-thread argument
        /// becomes something the type system enforces rather than something a
        /// comment promises: a `wasm32-unknown-unknown` page runs entirely on
        /// the one JS thread that installed this slot, so the waker can only
        /// ever run where the proxy already is, and any other thread would
        /// simply find its own slot empty rather than touching this one.
        static WAKE_PROXY: RefCell<Option<EventLoopProxy<ShellUserEvent>>> =
            const { RefCell::new(None) };
    }

    /// Park `proxy` in this thread's [`WAKE_PROXY`] slot and answer the
    /// [`FrameWaker`] `frust-reactive` fires on a tracked-signal write.
    ///
    /// The returned closure captures nothing, which is what makes it
    /// `Send + Sync` — see [`WAKE_PROXY`]. `send_event` fails only once the
    /// loop has closed (a shutdown race), which is benign and ignored, the
    /// same as every other shell's waker.
    fn install_wake_proxy(proxy: EventLoopProxy<ShellUserEvent>) -> FrameWaker {
        WAKE_PROXY.with(|slot| *slot.borrow_mut() = Some(proxy));
        Arc::new(|| {
            WAKE_PROXY.with(|slot| {
                if let Some(proxy) = slot.borrow().as_ref() {
                    let _ = proxy.send_event(ShellUserEvent::SignalsDirty);
                }
            });
        })
    }

    /// The shared home of the frame executor, which does not exist until an
    /// `async` bring-up completes.
    ///
    /// Shaped as `Rc<SurfaceSlot>` because the bring-up future must own the
    /// executor across its `await` and hand it back afterwards, while the
    /// event loop keeps a handle to draw through. `pending` is the guard that
    /// keeps a second bring-up from being spawned (and a second `wgpu` device
    /// from being created) while the first is still in flight — a `resumed`
    /// re-entry or a redraw arriving mid-bring-up would otherwise do exactly
    /// that, since the executor slot reads as empty the whole time.
    #[derive(Default)]
    struct SurfaceSlot {
        executor: RefCell<Option<WebFrameExecutor>>,
        pending: Cell<bool>,
    }

    /// Start (or restart) the asynchronous surface bring-up for `window`,
    /// unless one is already in flight.
    ///
    /// One path serves both the cold start and `FrameOutcome::SurfaceLost`
    /// recovery: an executor already in the slot is *taken* and re-used, so a
    /// recovery keeps the live `wgpu` device and only replaces the surface,
    /// while a cold start builds a fresh one. Frames drawn while the slot is
    /// empty are simply skipped, which is the same thing the renderer would do
    /// internally in `SurfacePhase::NoSurface`.
    fn spawn_surface_bringup(slot: &Rc<SurfaceSlot>, window: &Arc<Window>) {
        if slot.pending.get() {
            return;
        }
        slot.pending.set(true);
        let mut executor = slot.executor.borrow_mut().take().unwrap_or_default();
        let slot = Rc::clone(slot);
        let window = Arc::clone(window);
        let size = window.inner_size();
        wasm_bindgen_futures::spawn_local(async move {
            let outcome = executor
                .ensure_surface_with_retry(Arc::clone(&window), size.width, size.height)
                .await;
            slot.pending.set(false);
            match outcome {
                Ok(attempts) => {
                    log::info!(
                        "frust: render surface online at {}x{} after {attempts} attempt(s)",
                        size.width,
                        size.height
                    );
                    *slot.executor.borrow_mut() = Some(executor);
                    // Drive the first frame explicitly rather than waiting for
                    // one to be asked for: every redraw requested before this
                    // point was serviced against an empty slot and drew
                    // nothing.
                    window.request_redraw();
                }
                // Terminal: the retry already logged every attempt verbatim.
                // The executor is dropped with its context, so a later
                // lifecycle event starts a genuinely fresh bring-up.
                Err(err) => log::error!("{err}"),
            }
        });
    }

    /// Apply a [`ControlFlowIntent`] to the live event loop.
    fn apply_control_flow(event_loop: &ActiveEventLoop, intent: ControlFlowIntent) {
        match intent {
            ControlFlowIntent::Wait => event_loop.set_control_flow(ControlFlow::Wait),
            ControlFlowIntent::WaitUntil(deadline) => {
                event_loop.set_control_flow(ControlFlow::WaitUntil(deadline))
            }
            ControlFlowIntent::Unchanged => {}
        }
    }

    /// The browser shell's winit `ApplicationHandler` — the retained tree, the
    /// reactive plumbing, the shell-owned appearance state, and the wake
    /// bookkeeping the frame loop runs on.
    struct WebShellHandler<State: 'static, Logic, V: View<State>> {
        state: State,
        app_logic: Logic,
        /// The process-wide reactive runtime, initialized on this (the page's
        /// only) thread. Per-frame rebuilds run under its root `Owner`, and
        /// `pump_local` drains the local task queue on each wake and frame.
        runtime: &'static ReactiveRuntime,
        /// Records which signals the last rebuild read, so a later write
        /// dirties the scope and (via the frame waker) wakes the loop for one
        /// more frame. Re-tracked from scratch every rebuild.
        scope: TrackedScope,
        root: RenderRoot<State, V>,
        text_ctx: TextContext,
        /// The frame executor, absent until its `async` bring-up lands — see
        /// [`SurfaceSlot`].
        surface: Rc<SurfaceSlot>,
        /// Reused across frames; `reset()` each frame rather than
        /// reallocated. There is no scene hand-off on this shell, so unlike
        /// the desktop split the same scene is drawn from in place.
        scene: Scene,
        /// Created in `resumed`, which winit calls once the page is ready for
        /// one.
        window: Option<Arc<Window>>,
        /// The app's active theme — the seeded base (a design system's
        /// `set_default_theme`, else the built-in fallback) until an app-forced
        /// override replaces it. Its `brightness` is seeded from the browser's
        /// `prefers-color-scheme` answer in `resumed` and flipped live on
        /// `WindowEvent::ThemeChanged`.
        theme: Theme,
        /// Whether the initial theme has been pushed to the render root and
        /// the reactive context yet.
        theme_seeded: bool,
        /// Polls the process-wide app-facing theme override slot once per
        /// frame, before rebuild.
        theme_override: ThemeOverrideWatcher,
        /// Whether an app-forced theme override is active. While `true`, a
        /// `prefers-color-scheme` flip must not move `theme.brightness`.
        theme_override_active: bool,
        /// Polls the process-wide pending-font registry once per frame,
        /// draining any late registration into `text_ctx`.
        font_registry: FontRegistryWatcher,
        /// Change detector for the app-facing `WindowMetrics` context: seeded
        /// in `resumed` and re-polled on `WindowEvent::Resized`, never per
        /// frame.
        window_metrics: WindowMetricsPublisher,
        /// The pending paced redraw deadline, or `None` when nothing paced is
        /// owed — see [`crate::pacing`].
        paced_wake: Option<Instant>,
        /// Whether animation pacing is enabled (the pacing kill switch,
        /// resolved once at construction). A browser has no environment to
        /// read, so this is always `true` there; it is resolved through the
        /// shared helper anyway so the two shell tiers cannot disagree about
        /// what the switch means.
        anim_pacing: bool,
        /// The shell-owned monotonic epoch every per-frame `FrameTime` is
        /// measured from. `frust-core` never reads a clock itself — time
        /// enters from the shell — and on this target the clock is
        /// `performance.now()` behind `web_time`.
        epoch: Instant,
    }

    impl<State, Logic, V> WebShellHandler<State, Logic, V>
    where
        State: 'static,
        V: View<State>,
        Logic: FnMut(&mut State) -> V + 'static,
    {
        /// The whole `rebuild → layout → paint → encode → present` turn, run
        /// inside the `requestAnimationFrame` callback winit services
        /// `RedrawRequested` from.
        fn frame(&mut self, event_loop: &ActiveEventLoop, window: &Arc<Window>) {
            // Drain any local tasks queued since the last turn before
            // rebuilding, so their signal writes are visible to this frame.
            self.runtime.pump_local();

            // Poll the app-facing theme override slot once per frame, before
            // rebuild. Reverting an override lands on the base this shell
            // seeded itself from, with brightness re-derived from the
            // browser's current `prefers-color-scheme` answer rather than
            // inherited from the cleared override.
            if let Some((theme, override_active)) =
                super::theme_after_override_poll(self.theme_override.poll(), default_theme, || {
                    super::brightness_from_winit(window.theme())
                })
            {
                self.theme = theme;
                self.theme_override_active = override_active;
                super::apply_theme(self.runtime, &mut self.root, &self.theme);
            }

            // Drain any late-registered fonts, and on a real drain force the
            // relayout `register_fonts` documents by re-pushing the active
            // theme (the same LAYOUT|PAINT contract a theme swap uses).
            if self.font_registry.drain_into(&mut self.text_ctx) {
                self.root.set_theme(Box::new(self.theme.clone()));
            }

            // The rebuild runs under the reactive runtime's root `Owner` (so
            // signals created during it are root-owned) and inside the
            // `TrackedScope` (so every signal read subscribes this frame — a
            // later write dirties the scope and fires the waker). Fields are
            // borrowed disjointly so the tracking closure captures only what
            // the rebuild needs.
            let rebuild_start = Instant::now();
            let runtime = self.runtime;
            let scope = &self.scope;
            let root = &mut self.root;
            let app_logic = &mut self.app_logic;
            let state = &mut self.state;
            let _flags = runtime.with_owner(|| scope.track(|| root.rebuild(app_logic, state)));
            let rebuild = rebuild_start.elapsed();

            // HiDPI: lay out in logical pixels, then scale the whole scene by
            // the browser's `devicePixelRatio` so glyph outlines are
            // re-rasterised sharp at the canvas's real backing resolution.
            let physical = window.inner_size();
            let scale = window.scale_factor();
            let logical = Size::new(
                f64::from(physical.width) / scale,
                f64::from(physical.height) / scale,
            );
            let text_ctx: &mut dyn Any = &mut self.text_ctx;
            let layout_start = Instant::now();
            self.root.layout_with_text(logical, text_ctx);
            let layout = layout_start.elapsed();

            self.scene.reset();
            // One clock read per frame, handed to paint; every animating
            // widget differences it against its own stored time.
            let frame_time = FrameTime::from_nanos(self.epoch.elapsed().as_nanos() as u64);
            // Push the presented-frame count so a widget measuring FPS reports
            // the presented rate, not its paint cadence. A pure observation —
            // it dirties nothing.
            if let Ok(slot) = self.surface.executor.try_borrow()
                && let Some(executor) = slot.as_ref()
            {
                self.root.set_presented_frames(executor.presented_frames());
            }
            let paint_start = Instant::now();
            let paint_outcome = {
                let mut builder = SceneBuilder::new(&mut self.scene);
                builder.push_transform(Affine::scale(scale));
                let outcome = self.root.paint(&mut builder, frame_time);
                builder.pop_transform();
                outcome
            };
            let paint = paint_start.elapsed();

            // Animation driver: if paint advanced animation state it asks for
            // another frame here. A paced-only decorative loop (a caret blink,
            // a shimmer) schedules a *delayed* wake instead of an immediate
            // redraw, so it runs at the theme's cosmetic-loop cadence rather
            // than at every display refresh; a real transition keeps the
            // immediate every-frame path. A settled loop clears any stale
            // deadline AND returns the control flow to `Wait`. The decision
            // bundles all three effects into one value so the field can never
            // be updated without also deciding the control flow — see
            // `crate::pacing`.
            let decision = next_paced_wake(
                paint_outcome.needs_frame,
                paint_outcome.needs_frame_paced_only,
                self.anim_pacing,
                Instant::now(),
                self.theme.motion.cosmetic_loop_rate.hz(),
                paint_outcome.paced_interval,
            );
            self.paced_wake = decision.paced_wake;
            if decision.request_redraw {
                window.request_redraw();
            }
            apply_control_flow(event_loop, decision.control_flow);

            // A tracked signal written *during* this frame already re-dirtied
            // the scope after `track` cleared it. The waker's clean-to-dirty
            // edge fired inside `track`, so no user event will arrive for it —
            // request the follow-up frame here.
            if self.scope.is_dirty() {
                window.request_redraw();
            }

            // Hand the finished frame to the executor, if one exists yet: a
            // redraw can be serviced while the bring-up future is still in
            // flight, and skipping is correct — that future drives its own
            // first frame when it lands.
            let ui_spans = frust_shell_common::perf::UiSpans {
                rebuild,
                layout,
                paint,
                skipped: false,
            };
            let base_color = self.theme.scheme().surface;
            let follow_up = match self.surface.executor.try_borrow_mut() {
                Ok(mut slot) => slot.as_mut().map(|executor| {
                    // Reconcile the swapchain against the canvas's size before
                    // drawing into it: on this host the first size the surface
                    // was configured at is necessarily stale, because winit
                    // reports 0x0 until its `ResizeObserver` has run. See
                    // `WebFrameExecutor::ensure_size`.
                    executor.ensure_size(physical.width, physical.height);
                    executor.submit_frame(&self.scene, base_color, ui_spans)
                }),
                Err(_) => None,
            };
            match follow_up {
                Some(FrameFollowUp::Redraw) => window.request_redraw(),
                Some(FrameFollowUp::RecreateSurface) => {
                    spawn_surface_bringup(&self.surface, window)
                }
                Some(FrameFollowUp::Idle) | None => {}
            }
        }
    }

    impl<State, Logic, V> ApplicationHandler<ShellUserEvent> for WebShellHandler<State, Logic, V>
    where
        State: 'static,
        V: View<State>,
        Logic: FnMut(&mut State) -> V + 'static,
    {
        /// A tracked-signal write routes here through the frame waker and the
        /// event-loop proxy. Pump the local task queue first (a completing
        /// local task may have driven the write), then ask for the one frame
        /// that write owes.
        ///
        /// This is the entire background-wake path: nothing about it involves
        /// an input event, which is what lets a signal written by a timer or a
        /// completing task repaint a page nobody is touching.
        fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: ShellUserEvent) {
            self.runtime.pump_local();
            match event {
                ShellUserEvent::SignalsDirty => {
                    if let Some(window) = self.window.as_ref() {
                        window.request_redraw();
                    }
                }
            }
        }

        fn resumed(&mut self, event_loop: &ActiveEventLoop) {
            if self.window.is_none() {
                // `with_append`: winit creates the canvas but never inserts it
                // into the page on its own, so an un-appended canvas renders
                // to nothing visible. Adopting a host page's own canvas
                // instead is the canvas-binding work this default stands in
                // for.
                let attributes = Window::default_attributes()
                    .with_inner_size(DEFAULT_CANVAS_SIZE)
                    .with_append(true);
                match event_loop.create_window(attributes) {
                    Ok(window) => self.window = Some(Arc::new(window)),
                    Err(err) => {
                        // A page has no exit path and no error to return to —
                        // report and leave the loop idle rather than
                        // pretending a window exists.
                        log::error!("frust: failed to create the browser window: {err}");
                        return;
                    }
                }
            }
            let window = self
                .window
                .clone()
                .expect("window was just created or already present");

            // Seed the theme once, before the first rebuild: read the
            // browser's `prefers-color-scheme` answer into the baseline, then
            // push it to the render root and the reactive context. Live
            // changes arrive later via `WindowEvent::ThemeChanged`.
            if !self.theme_seeded {
                self.theme.brightness = super::brightness_from_winit(window.theme());
                self.theme_seeded = true;
                super::apply_theme(self.runtime, &mut self.root, &self.theme);
            }

            // Seed the app-facing window-shape context before the first frame,
            // so a `Component::build` calling `use_context::<WindowMetrics>()`
            // in the very first rebuild resolves a real value rather than
            // `None`. Self-guarded, so a redundant `resumed` publishes nothing.
            let physical = window.inner_size();
            super::publish_window_metrics(
                self.runtime,
                &mut self.window_metrics,
                (physical.width, physical.height),
                window.scale_factor(),
            );

            // Bring the surface online. `resumed` can fire more than once and
            // the bring-up is asynchronous, so both the "already live" and the
            // "already starting" cases are guarded.
            if self.surface.executor.borrow().is_none() {
                spawn_surface_bringup(&self.surface, &window);
            }
            window.request_redraw();
        }

        fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
            // Drain any local tasks that became runnable while dispatching
            // this batch of events, before the loop parks. A task that writes
            // a signal here re-dirties a scope and fires the waker, which
            // re-arms the loop rather than parking it.
            self.runtime.pump_local();

            let now = Instant::now();
            // A wake delivered far past its deadline means the browser paused
            // this page — a hidden tab, a battery-saver throttle, a long
            // main-thread task. Reported rather than absorbed, because from
            // inside the loop it is otherwise indistinguishable from a wedge.
            // Exactly one frame is owed however long the gap was; see
            // `crate::pacing`'s long-gap section.
            if let Some(late) = overshoot(self.paced_wake, now)
                && late >= OVERSHOOT_LOG_THRESHOLD
            {
                log::debug!(
                    "frust: paced wake delivered {}ms late (the page was throttled or hidden); \
                     one frame is owed, not a backlog",
                    late.as_millis()
                );
            }

            let decision = paced_wake_action(self.paced_wake, now);
            self.paced_wake = decision.paced_wake;
            if decision.request_redraw
                && let Some(window) = self.window.as_ref()
            {
                window.request_redraw();
            }
            apply_control_flow(event_loop, decision.control_flow);
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
                WindowEvent::RedrawRequested => self.frame(event_loop, &window),

                // Minimal, deliberately: reconfigure the swapchain, republish
                // the app-facing window shape, and draw. Everything that makes
                // a browser resize interesting — observing the host element,
                // reconciling a `devicePixelRatio` change against the canvas's
                // backing store — belongs with the canvas binding, not with
                // the frame loop.
                WindowEvent::Resized(size) => {
                    if let Ok(mut slot) = self.surface.executor.try_borrow_mut()
                        && let Some(executor) = slot.as_mut()
                    {
                        executor.resize_surface(size.width, size.height);
                    }
                    super::publish_window_metrics(
                        self.runtime,
                        &mut self.window_metrics,
                        (size.width, size.height),
                        window.scale_factor(),
                    );
                    window.request_redraw();
                }

                // A browser zoom or a move between displays changes
                // `devicePixelRatio`; winit guarantees a following `Resized`,
                // which is the one place the change is acted on, so there is
                // nothing to do here.
                WindowEvent::ScaleFactorChanged { .. } => {}

                // The `prefers-color-scheme` media query flipped. An
                // app-forced override wins entirely until it is cleared.
                WindowEvent::ThemeChanged(theme) => {
                    super::follow_platform_brightness(
                        &mut self.theme,
                        self.theme_override_active,
                        super::brightness_from_winit(Some(theme)),
                    );
                    super::apply_theme(self.runtime, &mut self.root, &self.theme);
                    window.request_redraw();
                }

                // A page cannot close itself, and winit's web backend never
                // emits this today; answering it by tearing the app down would
                // be inventing a lifecycle the host does not have.
                WindowEvent::CloseRequested => {}

                // Input events fall through unhandled: mapping them onto the
                // framework's own vocabulary is what this module's
                // `InputState`/`map_*` half exists for, and wiring that half
                // into the loop is its own piece of work.
                _ => {}
            }
        }
    }

    /// Start a Frust app in the browser: create the event loop, initialize the
    /// reactive runtime against it, and hand the loop to the browser's own
    /// task queue.
    ///
    /// **Returns immediately on success.** winit's web backend implements
    /// `run_app` by throwing a JS exception to unwind out of the caller's
    /// stack, which surfaces as an uncaught error out of whatever `init()`
    /// glue called it; `spawn_app` instead hands control back normally and
    /// drives the loop from the browser's task queue. So this returning `Ok`
    /// means the app is *running*, not that it finished — there is no "after"
    /// for a page, and the app lives until the document goes away.
    ///
    /// The reactive runtime is initialized here, on the page's only thread,
    /// because that thread must be the one that owns the local task queue
    /// `pump_local` drains.
    pub fn spawn_app<State, Logic, V>(state: State, app_logic: Logic) -> Result<(), EventLoopError>
    where
        State: 'static,
        V: View<State>,
        Logic: FnMut(&mut State) -> V + 'static,
    {
        // Captured before anything else, so the frame clock's origin is the
        // earliest moment this shell exists.
        let epoch = Instant::now();

        let event_loop = EventLoop::<ShellUserEvent>::with_user_event().build()?;
        // Dirty-driven, exactly like the desktop core: a frame runs when
        // something asks for one. `ControlFlow::Poll` would schedule a task
        // per turn whether or not a frame was owed.
        event_loop.set_control_flow(ControlFlow::Wait);

        let waker = install_wake_proxy(event_loop.create_proxy());
        let runtime = ReactiveRuntime::init(waker);

        let mut handler = WebShellHandler {
            state,
            app_logic,
            runtime,
            scope: TrackedScope::new(),
            root: RenderRoot::new(),
            text_ctx: TextContext::new(),
            surface: Rc::new(SurfaceSlot::default()),
            scene: Scene::new(),
            window: None,
            // The seeded base theme, carrying that base's own brightness only
            // until `resumed` seeds the browser's real preference over it.
            theme: super::base_theme(default_theme()),
            theme_seeded: false,
            theme_override: ThemeOverrideWatcher::new(),
            theme_override_active: false,
            font_registry: FontRegistryWatcher::new(),
            window_metrics: WindowMetricsPublisher::new(),
            paced_wake: None,
            anim_pacing: !anim_pacing_kill_switch_engaged(),
            epoch,
        };

        // Apply any fonts registered before the app started, before the first
        // layout — pre-first-layout, so no invalidation is needed; the
        // per-frame poll picks up any later registration.
        handler.font_registry.drain_into(&mut handler.text_ctx);

        event_loop.spawn_app(handler);
        Ok(())
    }

    /// The facade's entry point: the shape `frust::web_app!` hands its
    /// initialised state and app-logic closure to (`run_app(state, logic)`,
    /// mirroring `frust_shell_desktop::run_desktop_with`'s `(state, logic, ..)`
    /// convention). A thin wrapper over [`spawn_app`]: the generated
    /// `#[wasm_bindgen(start)]` shim has nowhere to return an error to, so an
    /// event-loop construction failure is reported through the `log` facade
    /// (which the facade routes to the browser console before calling this)
    /// instead of being propagated. The facade initialises the reactive runtime
    /// with a no-op waker before building the state; `spawn_app`'s own
    /// `ReactiveRuntime::init` call then replaces that waker with the
    /// event-loop proxy — the documented idempotent re-init path.
    pub fn run_app<State, Logic, V>(state: State, app_logic: Logic)
    where
        State: 'static,
        V: View<State>,
        Logic: FnMut(&mut State) -> V + 'static,
    {
        if let Err(err) = spawn_app(state, app_logic) {
            log::error!("frust-shell-web: could not start the browser event loop: {err}");
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub use browser_loop::{ShellUserEvent, run_app, spawn_app};

#[cfg(test)]
mod tests {
    use super::{
        ComposeLatch, ElementState, Ime, InputState, MouseScrollDelta, WinitCursorIcon, WinitKey,
        WinitNamedKey, WinitTheme, base_theme, brightness_from_winit, cursor_change_to_apply,
        follow_platform_brightness, map_key_event, map_modifiers, map_mouse_button, map_named_key,
        map_scroll_delta, mouse_button_should_dispatch, physical_to_logical, reverted_theme,
        theme_after_override_poll, winit_cursor_for,
    };
    use frust_core::event::{
        CursorIcon, ImeEvent, InputEvent, Key, Modifiers, NamedKey, PointerButton, PointerPhase,
        ScrollDelta,
    };
    use frust_theme::{Brightness, Theme};
    use kurbo::Point;
    use winit::dpi::{PhysicalPosition, PhysicalSize};
    use winit::event::MouseButton;
    use winit::keyboard::ModifiersState;

    // --- physical_to_logical ---

    #[test]
    fn physical_to_logical_divides_by_scale() {
        // A 2× device-pixel-ratio page: a physical (200, 100) pointer is
        // logical (100, 50).
        assert_eq!(
            physical_to_logical(200.0, 100.0, 2.0),
            Point::new(100.0, 50.0)
        );
    }

    #[test]
    fn physical_to_logical_is_identity_at_unit_scale() {
        assert_eq!(physical_to_logical(37.0, 12.0, 1.0), Point::new(37.0, 12.0));
    }

    #[test]
    fn physical_to_logical_handles_a_fractional_device_pixel_ratio() {
        // Browsers report fractional ratios routinely (a 125%-zoomed page),
        // unlike the integral factors most desktop compositors hand out.
        assert_eq!(
            physical_to_logical(250.0, 125.0, 1.25),
            Point::new(200.0, 100.0)
        );
    }

    // --- map_scroll_delta ---

    #[test]
    fn map_scroll_delta_negates_line_deltas() {
        assert_eq!(
            map_scroll_delta(MouseScrollDelta::LineDelta(1.0, -3.0), 1.0),
            ScrollDelta::Lines(-1.0, 3.0)
        );
    }

    #[test]
    fn map_scroll_delta_negates_and_descales_pixel_deltas() {
        // A pixel-mode wheel event on a 2× page: physical pixels in, negated
        // logical pixels out.
        assert_eq!(
            map_scroll_delta(
                MouseScrollDelta::PixelDelta(PhysicalPosition::new(20.0, -40.0)),
                2.0
            ),
            ScrollDelta::Pixels(-10.0, 20.0)
        );
    }

    // --- map_mouse_button ---

    #[test]
    fn map_mouse_button_maps_left_and_right_only() {
        assert_eq!(
            map_mouse_button(MouseButton::Left),
            Some(PointerButton::Primary)
        );
        assert_eq!(
            map_mouse_button(MouseButton::Right),
            Some(PointerButton::Secondary)
        );
        assert_eq!(map_mouse_button(MouseButton::Middle), None);
        assert_eq!(map_mouse_button(MouseButton::Back), None);
    }

    // --- mouse_button_should_dispatch ---

    #[test]
    fn primary_is_never_gated() {
        let mut latch = false;
        for phase in [PointerPhase::Down, PointerPhase::Up] {
            assert!(mouse_button_should_dispatch(
                PointerButton::Primary,
                phase,
                true,
                &mut latch
            ));
        }
        // The latch belongs to the secondary button alone and must be untouched.
        assert!(!latch);
    }

    #[test]
    fn secondary_down_is_dropped_while_captured_and_its_up_follows_it() {
        let mut latch = false;
        assert!(!mouse_button_should_dispatch(
            PointerButton::Secondary,
            PointerPhase::Down,
            true,
            &mut latch
        ));
        // The release follows the press even though the capture has since
        // ended — the tree must never see an unpaired secondary event.
        assert!(!mouse_button_should_dispatch(
            PointerButton::Secondary,
            PointerPhase::Up,
            false,
            &mut latch
        ));
    }

    #[test]
    fn secondary_up_is_delivered_when_a_capture_opened_after_its_down() {
        let mut latch = false;
        assert!(mouse_button_should_dispatch(
            PointerButton::Secondary,
            PointerPhase::Down,
            false,
            &mut latch
        ));
        // A capture opened between the two: the release still has to land,
        // or the widget whose press was delivered stays armed forever.
        assert!(mouse_button_should_dispatch(
            PointerButton::Secondary,
            PointerPhase::Up,
            true,
            &mut latch
        ));
        assert!(!latch);
    }

    // --- map_named_key / map_modifiers ---

    #[test]
    fn map_named_key_maps_editing_keys_and_drops_the_rest() {
        assert_eq!(map_named_key(WinitNamedKey::Enter), Some(NamedKey::Enter));
        assert_eq!(map_named_key(WinitNamedKey::Escape), Some(NamedKey::Escape));
        // A browser key with no editing semantics is dropped, not misreported.
        assert_eq!(map_named_key(WinitNamedKey::BrowserBack), None);
        // Space is deliberately absent: it types a character, and
        // `map_key_event` routes it through the character path instead.
        assert_eq!(map_named_key(WinitNamedKey::Space), None);
    }

    #[test]
    fn map_modifiers_maps_every_bit() {
        let all = map_modifiers(
            ModifiersState::SHIFT
                | ModifiersState::CONTROL
                | ModifiersState::ALT
                | ModifiersState::SUPER,
        );
        assert_eq!(
            all,
            Modifiers {
                shift: true,
                ctrl: true,
                alt: true,
                meta: true,
            }
        );
        assert_eq!(map_modifiers(ModifiersState::empty()), Modifiers::default());
    }

    // --- map_key_event ---

    #[test]
    fn map_key_event_drops_key_releases() {
        assert!(
            map_key_event(
                &WinitKey::Character("a".into()),
                Some("a"),
                ElementState::Released,
                false,
                Modifiers::default(),
                false,
            )
            .is_none()
        );
    }

    #[test]
    fn map_key_event_maps_a_character_and_passes_repeat_through() {
        let event = map_key_event(
            &WinitKey::Character("a".into()),
            Some("a"),
            ElementState::Pressed,
            true,
            Modifiers::default(),
            false,
        )
        .expect("a pressed character key maps");
        assert_eq!(event.key, Key::Character("a".to_string()));
        assert!(event.repeat);
    }

    #[test]
    fn map_key_event_routes_space_through_the_character_path() {
        let event = map_key_event(
            &WinitKey::Named(WinitNamedKey::Space),
            Some(" "),
            ElementState::Pressed,
            false,
            Modifiers::default(),
            false,
        )
        .expect("space maps");
        assert_eq!(event.key, Key::Character(" ".to_string()));
    }

    #[test]
    fn map_key_event_falls_back_to_a_literal_space_when_no_text_is_sent() {
        let event = map_key_event(
            &WinitKey::Named(WinitNamedKey::Space),
            None,
            ElementState::Pressed,
            false,
            Modifiers::default(),
            false,
        )
        .expect("space maps without text");
        assert_eq!(event.key, Key::Character(" ".to_string()));
    }

    #[test]
    fn map_key_event_suppresses_characters_while_composing_but_not_named_keys() {
        assert!(
            map_key_event(
                &WinitKey::Character("a".into()),
                Some("a"),
                ElementState::Pressed,
                false,
                Modifiers::default(),
                true,
            )
            .is_none()
        );
        assert!(
            map_key_event(
                &WinitKey::Named(WinitNamedKey::Space),
                Some(" "),
                ElementState::Pressed,
                false,
                Modifiers::default(),
                true,
            )
            .is_none()
        );
        // An editing key still reaches the tree mid-composition: an IME's own
        // Enter/Escape handling is the widget's business, not a duplicate of
        // the composed text.
        let event = map_key_event(
            &WinitKey::Named(WinitNamedKey::Enter),
            None,
            ElementState::Pressed,
            false,
            Modifiers::default(),
            true,
        )
        .expect("a named key survives composition");
        assert_eq!(event.key, Key::Named(NamedKey::Enter));
    }

    // --- ComposeLatch ---

    #[test]
    fn compose_latch_opens_on_a_preedit_and_closes_on_commit() {
        let mut latch = ComposeLatch::default();
        assert!(!latch.is_composing());

        let mapped = latch.observe(&Ime::Preedit("ni".to_string(), Some((2, 2))));
        assert_eq!(
            mapped,
            ImeEvent::Compose {
                text: "ni".to_string(),
                cursor: Some((2, 2)),
            }
        );
        assert!(latch.is_composing());

        let mapped = latch.observe(&Ime::Commit("你".to_string()));
        assert_eq!(mapped, ImeEvent::Commit("你".to_string()));
        assert!(!latch.is_composing());
    }

    #[test]
    fn compose_latch_closes_on_an_empty_preedit() {
        let mut latch = ComposeLatch::default();
        latch.observe(&Ime::Preedit("ni".to_string(), None));
        assert!(latch.is_composing());
        // winit's contract: an empty preedit precedes a commit.
        latch.observe(&Ime::Preedit(String::new(), None));
        assert!(!latch.is_composing());
    }

    // --- InputState ---

    #[test]
    fn pointer_move_records_the_logical_position_a_click_then_reuses() {
        let mut input = InputState::new();
        let moved = input.pointer_moved(PhysicalPosition::new(200.0, 100.0), 2.0);
        assert_eq!(
            moved,
            InputEvent::Pointer(frust_core::event::PointerEvent {
                phase: PointerPhase::Move,
                position: Point::new(100.0, 50.0),
                button: PointerButton::Primary,
            })
        );
        assert_eq!(input.cursor_position(), Point::new(100.0, 50.0));

        // A press carries no position of its own and must reuse the move's.
        let pressed = input
            .mouse_input(ElementState::Pressed, MouseButton::Left, false)
            .expect("a left press maps");
        let InputEvent::Pointer(pointer) = pressed else {
            panic!("expected a pointer event");
        };
        assert_eq!(pointer.position, Point::new(100.0, 50.0));
        assert_eq!(pointer.phase, PointerPhase::Down);
    }

    #[test]
    fn input_state_drops_an_unmapped_button() {
        let mut input = InputState::new();
        assert!(
            input
                .mouse_input(ElementState::Pressed, MouseButton::Middle, false)
                .is_none()
        );
    }

    #[test]
    fn input_state_pairs_a_secondary_press_and_release_through_its_own_latch() {
        let mut input = InputState::new();
        // Captured at press time: the press is dropped, and so is its release.
        assert!(
            input
                .mouse_input(ElementState::Pressed, MouseButton::Right, true)
                .is_none()
        );
        assert!(
            input
                .mouse_input(ElementState::Released, MouseButton::Right, false)
                .is_none()
        );
    }

    #[test]
    fn modifiers_changed_is_visible_to_the_next_key_event() {
        let mut input = InputState::new();
        input.modifiers_changed(ModifiersState::CONTROL);
        assert_eq!(
            input.modifiers(),
            Modifiers {
                shift: false,
                ctrl: true,
                alt: false,
                meta: false,
            }
        );

        let event = input
            .keyboard_input(
                &WinitKey::Character("c".into()),
                Some("c"),
                ElementState::Pressed,
                false,
            )
            .expect("a pressed character maps");
        let InputEvent::Key(key) = event else {
            panic!("expected a key event");
        };
        assert!(key.modifiers.ctrl);
    }

    #[test]
    fn an_ime_composition_suppresses_the_key_events_that_would_duplicate_it() {
        let mut input = InputState::new();
        assert!(!input.is_composing());
        input.ime(&Ime::Preedit("ni".to_string(), None));
        assert!(input.is_composing());
        assert!(
            input
                .keyboard_input(
                    &WinitKey::Character("i".into()),
                    Some("i"),
                    ElementState::Pressed,
                    false,
                )
                .is_none()
        );
    }

    #[test]
    fn mouse_wheel_is_anchored_at_the_last_pointer_position() {
        let mut input = InputState::new();
        input.pointer_moved(PhysicalPosition::new(40.0, 60.0), 2.0);
        let event = input.mouse_wheel(MouseScrollDelta::LineDelta(0.0, 1.0), 2.0);
        assert_eq!(
            event,
            InputEvent::Scroll {
                position: Point::new(20.0, 30.0),
                delta: ScrollDelta::Lines(0.0, -1.0),
            }
        );
    }

    // --- theme ---

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
        // No reported preference — a browser without the media query — falls
        // back to Light.
        assert_eq!(brightness_from_winit(None), Brightness::Light);
    }

    #[test]
    fn base_theme_prefers_the_seeded_design_system_over_the_fallback() {
        let mut seeded = Theme::neutral();
        seeded.brightness = Brightness::Dark;
        assert_eq!(base_theme(Some(seeded)).brightness, Brightness::Dark);
        assert_eq!(base_theme(None).brightness, Theme::neutral().brightness);
    }

    #[test]
    fn reverted_theme_takes_the_platform_brightness_not_the_seed_s() {
        let mut seeded = Theme::neutral();
        seeded.brightness = Brightness::Dark;
        let reverted = reverted_theme(Some(seeded), Brightness::Light);
        assert_eq!(reverted.brightness, Brightness::Light);
    }

    #[test]
    fn theme_after_override_poll_reports_nothing_when_nothing_changed() {
        let decided = theme_after_override_poll(
            None,
            || panic!("the seeded default must not be read on an unchanged poll"),
            || panic!("the platform brightness must not be read on an unchanged poll"),
        );
        assert!(decided.is_none());
    }

    #[test]
    fn theme_after_override_poll_lets_a_forced_theme_win_wholesale() {
        let mut forced = Theme::neutral();
        forced.brightness = Brightness::Dark;
        let (theme, pinned) = theme_after_override_poll(
            Some(Some(forced)),
            || panic!("a forced theme must not consult the seeded default"),
            || panic!("a forced theme must not consult the platform brightness"),
        )
        .expect("a forced theme is a change");
        assert_eq!(theme.brightness, Brightness::Dark);
        assert!(pinned);
    }

    #[test]
    fn clearing_an_override_reverts_to_the_seed_at_the_platform_brightness() {
        let mut seeded = Theme::neutral();
        seeded.brightness = Brightness::Dark;
        let (theme, pinned) =
            theme_after_override_poll(Some(None), || Some(seeded), || Brightness::Light)
                .expect("clearing an override is a change");
        assert_eq!(theme.brightness, Brightness::Light);
        assert!(!pinned);
    }

    #[test]
    fn follow_platform_brightness_flips_a_seeded_default_but_not_a_forced_theme() {
        let mut theme = Theme::neutral();
        theme.brightness = Brightness::Light;
        follow_platform_brightness(&mut theme, false, Brightness::Dark);
        assert_eq!(theme.brightness, Brightness::Dark);

        // An app-forced override pins brightness: a `prefers-color-scheme`
        // flip must not move it.
        let mut pinned = Theme::neutral();
        pinned.brightness = Brightness::Dark;
        follow_platform_brightness(&mut pinned, true, Brightness::Light);
        assert_eq!(pinned.brightness, Brightness::Dark);
    }

    // --- cursor ---

    #[test]
    fn winit_cursor_for_maps_every_named_shape() {
        assert_eq!(winit_cursor_for(CursorIcon::Text), WinitCursorIcon::Text);
        assert_eq!(
            winit_cursor_for(CursorIcon::Grabbing),
            WinitCursorIcon::Grabbing
        );
        assert_eq!(
            winit_cursor_for(CursorIcon::NotAllowed),
            WinitCursorIcon::NotAllowed
        );
        assert_eq!(
            winit_cursor_for(CursorIcon::Default),
            WinitCursorIcon::Default
        );
    }

    #[test]
    fn cursor_change_is_reported_only_when_the_shape_actually_moves() {
        assert_eq!(
            cursor_change_to_apply(CursorIcon::Default, CursorIcon::Pointer),
            Some(WinitCursorIcon::Pointer)
        );
        assert_eq!(
            cursor_change_to_apply(CursorIcon::Pointer, CursorIcon::Pointer),
            None
        );
    }

    // --- a guard against a silent winit-web assumption ---

    #[test]
    fn a_physical_size_round_trips_through_the_logical_conversion() {
        // The metrics publish divides winit's physical `inner_size` by the
        // device-pixel ratio; this pins the arithmetic the browser's
        // fractional ratios make easy to get wrong.
        let physical = PhysicalSize::new(1000u32, 500u32);
        let logical =
            physical_to_logical(f64::from(physical.width), f64::from(physical.height), 2.5);
        assert_eq!(logical, Point::new(400.0, 200.0));
    }
}
