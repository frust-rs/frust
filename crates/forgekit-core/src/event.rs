//! Layer 2 input: pointer/scroll events and the [`EventCtx`] a widget mutates
//! while handling them (spec §9).
//!
//! The pipeline mirrors Masonry's corrected pointer model: an [`InputEvent`]
//! enters the tree at the root ([`crate::app::RenderRoot::event`]) and is routed
//! down through container [`ChildPod`](crate::widget::ChildPod)s, each of which
//! translates the event into its child's local coordinate space before
//! forwarding. A widget reports what it did through [`EventResult`] and can, via
//! [`EventCtx`], mutate application state, request a redraw, or *capture* the
//! pointer so subsequent moves/releases route straight back to it.
//!
//! Capture here is **by recorded path**, not a global registry: on
//! [`PointerPhase::Down`] a widget calls [`EventCtx::capture_pointer`]; the
//! enclosing container reads the flag ([`EventCtx::is_pointer_captured`]) and
//! records which child was active so it can route later moves/releases directly.
//! Capture auto-releases on [`PointerPhase::Up`]/[`PointerPhase::Cancel`] (never
//! on window-leave). See `research/RESEARCH.md` for the upstream rationale.

use std::any::Any;

use kurbo::{Point, Rect, Size, Vec2};

/// Which physical (or synthetic) button a pointer event carries.
///
/// Touch and pen contacts report [`PointerButton::Primary`]; the secondary /
/// middle variants exist for mouse input (right/middle click).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerButton {
    /// The primary button (left mouse, or any touch/pen contact).
    Primary,
    /// The secondary button (right mouse).
    Secondary,
    /// The middle button (mouse wheel click).
    Middle,
}

/// The lifecycle phase of a pointer gesture.
///
/// A gesture is a `Down`, zero or more `Move`s, and a terminating `Up` or
/// `Cancel`. `Cancel` fires when the platform steals the gesture (e.g. a system
/// gesture recognizer wins) and, like `Up`, releases any capture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerPhase {
    /// A contact began (mouse-down / finger-down).
    Down,
    /// A contact moved while down.
    Move,
    /// A contact ended normally (mouse-up / finger-up). Releases capture.
    Up,
    /// The gesture was cancelled by the platform. Releases capture.
    Cancel,
}

/// A single pointer event in the coordinate space of the widget receiving it.
///
/// `position` is **logical** (density-independent) pixels, already translated
/// into the receiving widget's local space by the container chain that routed it
/// (see [`crate::widget::ChildPod::event_child`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointerEvent {
    /// The gesture phase.
    pub phase: PointerPhase,
    /// The pointer location, in the receiving widget's local logical space.
    pub position: Point,
    /// Which button the event carries (`Primary` for touch/pen).
    pub button: PointerButton,
}

/// A scroll amount, in either discrete lines or continuous pixels.
///
/// Line deltas come from mouse wheels (winit `LineDelta`); pixel deltas from
/// precision trackpads/touch (winit `PixelDelta`). The `(x, y)` order is
/// horizontal then vertical.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ScrollDelta {
    /// A wheel-notch delta measured in lines `(x, y)`.
    Lines(f64, f64),
    /// A precision delta measured in logical pixels `(x, y)`.
    Pixels(f64, f64),
}

/// A named (non-character) key: the control keys an editable widget reacts to.
///
/// Character-producing keys arrive as [`Key::Character`] (already resolved to the
/// typed text, so dead keys / smart quotes / IME are handled upstream); only the
/// keys with editing *semantics* are enumerated here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NamedKey {
    /// Return / Enter — submit or newline.
    Enter,
    /// Backspace — delete the grapheme before the caret.
    Backspace,
    /// Forward delete — delete the grapheme after the caret.
    Delete,
    /// Move / extend the caret left.
    ArrowLeft,
    /// Move / extend the caret right.
    ArrowRight,
    /// Move / extend the caret up.
    ArrowUp,
    /// Move / extend the caret down.
    ArrowDown,
    /// Move to line / document start.
    Home,
    /// Move to line / document end.
    End,
    /// Cancel / dismiss (blur, drop composition).
    Escape,
    /// Tab — focus traversal or literal tab (widget's choice).
    Tab,
}

/// A logical key press: either a semantic [`NamedKey`] or a run of typed text.
///
/// [`Key::Character`] carries the *resolved* text a key produced (winit's
/// `KeyEvent.text` / a platform character), so widgets insert it verbatim without
/// re-deriving it from a keycode + modifiers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Key {
    /// A control / navigation key with editing semantics.
    Named(NamedKey),
    /// Typed text to insert as-is (usually a single grapheme).
    Character(String),
}

/// The chord of modifier keys held when a [`KeyEvent`] fired.
///
/// `meta` is Command on macOS and the Windows/Super key elsewhere; widgets use
/// it (with `ctrl`) for shortcuts like select-all.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    /// Shift held (extends selection on arrow keys).
    pub shift: bool,
    /// Control held.
    pub ctrl: bool,
    /// Alt / Option held.
    pub alt: bool,
    /// Meta held (Command on macOS, Super/Windows elsewhere).
    pub meta: bool,
}

/// A keyboard key event delivered down the focus path (never hit-tested).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyEvent {
    /// The logical key (a [`NamedKey`] or typed [`Key::Character`] text).
    pub key: Key,
    /// The modifier chord held when the key fired.
    pub modifiers: Modifiers,
    /// Whether this is an auto-repeat (key held down), not a fresh press.
    pub repeat: bool,
}

/// The full editing state of a text field, the one struct every IME bridge syncs.
///
/// This mirrors Flutter's canonical editing-state shape (`−1` = "none" for the
/// selection/composing anchors). It is the value pushed across the framework↔
/// platform seam in both directions.
///
/// # Index boundary rule
///
/// **An `EditingState` crossing the `AppTree`/shell seam is UTF-16 code-unit
/// indexed** (`selection_*`/`composing_*` count UTF-16 units, the platform-native
/// unit for both Android `Editable` and iOS `NSMutableString`). Widgets and
/// `forgekit-text` convert to/from Rust byte offsets at their own boundary
/// (task 52 owns the conversion helpers). Core carries the value opaquely and
/// makes no index interpretation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditingState {
    /// The full text content.
    pub text: String,
    /// Selection anchor (UTF-16 unit index at the shell seam; `−1` = none).
    pub selection_base: i32,
    /// Selection focus (UTF-16 unit index at the shell seam; `−1` = none).
    pub selection_extent: i32,
    /// Composing-region start (UTF-16 unit index; `−1` = not composing).
    pub composing_base: i32,
    /// Composing-region end (UTF-16 unit index; `−1` = not composing).
    pub composing_extent: i32,
}

/// An input-method (IME) event delivered down the focus path (never hit-tested).
///
/// Desktop drives [`ImeEvent::Compose`]/[`ImeEvent::Commit`] from winit's
/// `Ime::Preedit`/`Ime::Commit`; the mobile bridges push whole values via
/// [`ImeEvent::ApplyEditingState`] (state-sync, not op-forwarding — see
/// `research/RESEARCH.md`). [`ImeEvent::Enabled`]/[`ImeEvent::Disabled`] bracket a
/// composition session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImeEvent {
    /// Preedit / marked text: `text` is the composing string, `cursor` its
    /// optional `(start, end)` selection within that string (byte indices, as
    /// winit reports).
    Compose {
        /// The composing (marked) text.
        text: String,
        /// Optional caret/selection `(start, end)` inside `text`.
        cursor: Option<(usize, usize)>,
    },
    /// Commit finished composition: insert `text` and clear the composing region.
    Commit(String),
    /// Replace the whole editing state (mobile state-sync path).
    ApplyEditingState(EditingState),
    /// The platform enabled IME on the focused field (composition may begin).
    Enabled,
    /// The platform disabled IME (composition ended / focus left).
    Disabled,
}

/// An input event delivered to the widget tree.
///
/// Pointer gestures and scroll are **hit-tested** (routed by position); keyboard
/// and IME events are **focus-routed** — delivered straight down the recorded
/// focus chain with no hit test and no meaningful position (see
/// [`crate::widget::ChildPod`]'s focus bookkeeping and `forgekit-widgets`'
/// `route_event`).
#[derive(Clone, Debug, PartialEq)]
pub enum InputEvent {
    /// A pointer (mouse/touch/pen) gesture event.
    Pointer(PointerEvent),
    /// A scroll event at `position` (local logical space) carrying `delta`.
    Scroll {
        /// Where the scroll occurred, in the receiving widget's local space.
        position: Point,
        /// How much to scroll.
        delta: ScrollDelta,
    },
    /// A keyboard key event, routed down the focus path (no hit test).
    Key(KeyEvent),
    /// An IME event, routed down the focus path (no hit test).
    Ime(ImeEvent),
}

impl InputEvent {
    /// The event's location, in the receiving widget's local coordinate space.
    ///
    /// Focus-routed events ([`InputEvent::Key`]/[`InputEvent::Ime`]) have no
    /// spatial position — they are delivered down the focus chain, not hit-tested
    /// — so this reports [`Point::ZERO`] for them; callers must never hit-test on
    /// it (routing helpers early-return the focus-routed variants).
    pub fn position(&self) -> Point {
        match self {
            InputEvent::Pointer(p) => p.position,
            InputEvent::Scroll { position, .. } => *position,
            InputEvent::Key(_) | InputEvent::Ime(_) => Point::ZERO,
        }
    }

    /// Return a copy of this event with its position shifted by `offset`.
    ///
    /// Containers use this (with `offset = -child_origin`) to translate an event
    /// from their own coordinate space into a child's local space before
    /// forwarding it — see [`crate::widget::ChildPod::event_child`]. Focus-routed
    /// events ([`InputEvent::Key`]/[`InputEvent::Ime`]) carry no position, so they
    /// are returned unchanged (cloned).
    pub fn translated(&self, offset: Vec2) -> InputEvent {
        match self {
            InputEvent::Pointer(p) => InputEvent::Pointer(PointerEvent {
                position: p.position + offset,
                ..*p
            }),
            InputEvent::Scroll { position, delta } => InputEvent::Scroll {
                position: *position + offset,
                delta: *delta,
            },
            InputEvent::Key(_) | InputEvent::Ime(_) => self.clone(),
        }
    }

    /// Whether this event is focus-routed (delivered down the focus chain with no
    /// hit test) rather than hit-tested by position.
    pub fn is_focus_routed(&self) -> bool {
        matches!(self, InputEvent::Key(_) | InputEvent::Ime(_))
    }
}

/// The IME-relevant surface a focused editable widget publishes for the shell.
///
/// Written by the focused widget through [`EventCtx::publish_ime_state`], it
/// bubbles up the focus chain and is stored on [`crate::app::RenderRoot`], where
/// the shell reads it via [`crate::app::RenderRoot::ime_state`] to drive the
/// platform IME (winit `set_ime_cursor_area`, Android `updateSelection`, iOS
/// `inputDelegate`). See the module docs for the index boundary rule.
#[derive(Clone, Debug, PartialEq)]
pub struct ImeState {
    /// Whether the focused widget currently wants IME active.
    pub active: bool,
    /// The current editing state (UTF-16 indexed at this shell-facing surface).
    pub editing: EditingState,
    /// The caret rectangle in logical coordinates, for IME candidate placement.
    pub caret: Option<Rect>,
}

/// What a widget did with an event.
///
/// `Handled` stops the enclosing container from offering the event to further
/// siblings and marks the frame dirty; `Ignored` lets routing continue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventResult {
    /// The widget did not consume the event.
    Ignored,
    /// The widget consumed the event.
    Handled,
}

/// The result of a whole [`crate::app::RenderRoot::event`] pass.
///
/// `handled` is whether any widget consumed the event; `needs_redraw` is whether
/// the shell should schedule a repaint (a handled event or an explicit
/// [`EventCtx::request_redraw`]). The shell turns `needs_redraw` into a
/// `window.request_redraw()` — the event pass itself never rebuilds or repaints.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EventOutcome {
    /// Whether the event was consumed by the tree.
    pub handled: bool,
    /// Whether the shell should schedule a redraw as a result.
    pub needs_redraw: bool,
}

/// Context threaded into [`crate::widget::Widget::event`].
///
/// Gives an event handler three capabilities: mutate the (type-erased)
/// application state, request a redraw, and capture the pointer. It also carries
/// the receiving widget's own geometry ([`EventCtx::origin`]/[`EventCtx::size`])
/// so handlers can do local-coordinate math (the event `position` is already in
/// the widget's local space; origin/size describe where that widget sits in and
/// how big it is within its parent).
///
/// `State` is erased as `&mut dyn Any` — the same pattern
/// [`crate::widget::LayoutCtx`] uses for the text context — so `forgekit-core`
/// carries no knowledge of the concrete app state type; a handler recovers it
/// with [`EventCtx::state_mut`].
pub struct EventCtx<'a> {
    state: &'a mut dyn Any,
    needs_redraw: bool,
    capture_requested: bool,
    /// Set by [`EventCtx::request_focus`]; read by the enclosing container to
    /// record which child holds the focus path (mirrors `capture_requested`).
    focus_requested: bool,
    /// Set by [`EventCtx::release_focus`]; drops the recorded focus path.
    focus_released: bool,
    /// Whether the receiving widget currently holds focus (threaded down from its
    /// pod's recorded focus flag; seeded from the root focus state at the root).
    has_focus: bool,
    /// The IME surface the focused widget published this dispatch, if any; bubbles
    /// up the focus chain to [`crate::app::RenderRoot`].
    ime_state: Option<ImeState>,
    origin: Point,
    size: Size,
}

impl<'a> EventCtx<'a> {
    /// Build a root event context over the erased application `state` for a
    /// widget placed at `origin` with `size`.
    pub fn new(state: &'a mut dyn Any, origin: Point, size: Size) -> Self {
        Self {
            state,
            needs_redraw: false,
            capture_requested: false,
            focus_requested: false,
            focus_released: false,
            has_focus: false,
            ime_state: None,
            origin,
            size,
        }
    }

    /// Recover the application state as `&mut T`.
    ///
    /// Panics if `T` is not the concrete state type the render root erased — a
    /// shell/wiring bug, not a runtime-data condition (mirrors
    /// [`crate::widget::LayoutCtx::text_context`]).
    pub fn state_mut<T: Any>(&mut self) -> &mut T {
        self.state
            .downcast_mut::<T>()
            .expect("event state is not the expected application-state type")
    }

    /// Request that the shell schedule a repaint after this event pass.
    pub fn request_redraw(&mut self) {
        self.needs_redraw = true;
    }

    /// Whether a redraw was requested during this (sub)dispatch.
    pub fn needs_redraw(&self) -> bool {
        self.needs_redraw
    }

    /// Capture the pointer: subsequent moves/releases should route back to this
    /// widget. The enclosing container reads [`EventCtx::is_pointer_captured`]
    /// after the dispatch returns to record the active child.
    pub fn capture_pointer(&mut self) {
        self.capture_requested = true;
    }

    /// Whether the widget requested pointer capture during this (sub)dispatch.
    pub fn is_pointer_captured(&self) -> bool {
        self.capture_requested
    }

    /// Request focus: subsequent keyboard/IME events should route to this widget.
    ///
    /// The enclosing container reads the flag after the dispatch returns and
    /// records this child as the focused path (the focus mirror of
    /// [`EventCtx::capture_pointer`]). Focus is delivered down the recorded chain
    /// with no hit test.
    pub fn request_focus(&mut self) {
        self.focus_requested = true;
    }

    /// Release focus: drop the recorded focus path (e.g. Escape / blur).
    pub fn release_focus(&mut self) {
        self.focus_released = true;
    }

    /// Whether the receiving widget currently holds the focus path.
    ///
    /// Threaded down from the widget's pod ([`crate::widget::ChildPod::is_focused`]);
    /// a keyboard/IME event only reaches a widget along this chain, so a widget
    /// handling such an event is by construction focused.
    pub fn has_focus(&self) -> bool {
        self.has_focus
    }

    /// Publish this widget's IME surface (editing state + caret) for the shell.
    ///
    /// The value bubbles up the focus chain to [`crate::app::RenderRoot`], where
    /// the shell reads it via [`crate::app::RenderRoot::ime_state`]. Called by the
    /// focused editable widget after any state change so the platform IME stays in
    /// sync.
    pub fn publish_ime_state(&mut self, state: ImeState) {
        self.ime_state = Some(state);
    }

    /// Whether this widget requested focus during this (sub)dispatch (container-side).
    pub(crate) fn is_focus_requested(&self) -> bool {
        self.focus_requested
    }

    /// Whether this widget released focus during this (sub)dispatch (container-side).
    pub(crate) fn is_focus_released(&self) -> bool {
        self.focus_released
    }

    /// Take the IME surface published during this (sub)dispatch, leaving `None`.
    pub(crate) fn take_ime_state(&mut self) -> Option<ImeState> {
        self.ime_state.take()
    }

    /// Seed whether the receiving (root) widget holds focus — used by
    /// [`crate::app::RenderRoot::event`] when it dispatches straight to the root.
    pub(crate) fn set_has_focus(&mut self, has_focus: bool) {
        self.has_focus = has_focus;
    }

    /// The receiving widget's origin in its parent's coordinate space.
    pub fn origin(&self) -> Point {
        self.origin
    }

    /// The receiving widget's resolved size.
    pub fn size(&self) -> Size {
        self.size
    }

    /// Create a fresh sub-context for a child at `origin`/`size`, reborrowing the
    /// same erased state. The child's `needs_redraw`/`capture_requested`/focus
    /// flags start clear; `has_focus` reflects the child pod's recorded focus
    /// flag. The parent folds the results back in with [`EventCtx::absorb_child`].
    pub(crate) fn child_ctx(&mut self, origin: Point, size: Size, focused: bool) -> EventCtx<'_> {
        EventCtx {
            state: &mut *self.state,
            needs_redraw: false,
            capture_requested: false,
            focus_requested: false,
            focus_released: false,
            has_focus: focused,
            ime_state: None,
            origin,
            size,
        }
    }

    /// Fold a child dispatch's redraw/capture/focus flags (and any published IME
    /// surface) back into this context.
    pub(crate) fn absorb_child(
        &mut self,
        child_needs_redraw: bool,
        child_captured: bool,
        child_focus_requested: bool,
        child_focus_released: bool,
        child_ime_state: Option<ImeState>,
    ) {
        self.needs_redraw |= child_needs_redraw;
        self.capture_requested |= child_captured;
        self.focus_requested |= child_focus_requested;
        self.focus_released |= child_focus_released;
        if child_ime_state.is_some() {
            self.ime_state = child_ime_state;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn down(x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Down,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    #[test]
    fn state_mut_recovers_concrete_state() {
        let mut count = 3u32;
        let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::new(10.0, 10.0));
        *ctx.state_mut::<u32>() += 1;
        assert_eq!(count, 4);
    }

    #[test]
    fn request_redraw_and_capture_set_flags() {
        let mut count = 0u32;
        let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::ZERO);
        assert!(!ctx.needs_redraw());
        assert!(!ctx.is_pointer_captured());
        ctx.request_redraw();
        ctx.capture_pointer();
        assert!(ctx.needs_redraw());
        assert!(ctx.is_pointer_captured());
    }

    #[test]
    fn translated_shifts_pointer_position() {
        let e = down(20.0, 30.0);
        let local = e.translated(-Vec2::new(5.0, 7.0));
        assert_eq!(local.position(), Point::new(15.0, 23.0));
        // The original is untouched.
        assert_eq!(e.position(), Point::new(20.0, 30.0));
    }

    #[test]
    fn absorb_child_folds_flags_upward() {
        let mut count = 0u32;
        let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::ZERO);
        {
            let mut child = ctx.child_ctx(Point::new(1.0, 2.0), Size::new(3.0, 4.0), false);
            child.request_redraw();
            child.capture_pointer();
            let (redraw, cap) = (child.needs_redraw(), child.is_pointer_captured());
            ctx.absorb_child(redraw, cap, false, false, None);
        }
        assert!(ctx.needs_redraw());
        assert!(ctx.is_pointer_captured());
    }

    #[test]
    fn request_and_release_focus_set_flags() {
        let mut count = 0u32;
        let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::ZERO);
        assert!(!ctx.is_focus_requested());
        assert!(!ctx.is_focus_released());
        assert!(!ctx.has_focus());
        ctx.request_focus();
        ctx.release_focus();
        assert!(ctx.is_focus_requested());
        assert!(ctx.is_focus_released());
    }

    #[test]
    fn child_ctx_seeds_has_focus_from_pod_flag() {
        let mut count = 0u32;
        let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::ZERO);
        let focused_child = ctx.child_ctx(Point::ZERO, Size::ZERO, true);
        assert!(focused_child.has_focus());
        let unfocused_child = ctx.child_ctx(Point::ZERO, Size::ZERO, false);
        assert!(!unfocused_child.has_focus());
    }

    #[test]
    fn absorb_child_folds_focus_and_ime_upward() {
        let mut count = 0u32;
        let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::ZERO);
        let published = ImeState {
            active: true,
            editing: EditingState {
                text: "hi".to_string(),
                selection_base: 2,
                selection_extent: 2,
                composing_base: -1,
                composing_extent: -1,
            },
            caret: Some(Rect::new(0.0, 0.0, 1.0, 10.0)),
        };
        {
            let mut child = ctx.child_ctx(Point::ZERO, Size::ZERO, false);
            child.request_focus();
            child.publish_ime_state(published.clone());
            let (fr, frl, ime) = (
                child.is_focus_requested(),
                child.is_focus_released(),
                child.take_ime_state(),
            );
            ctx.absorb_child(false, false, fr, frl, ime);
        }
        assert!(ctx.is_focus_requested());
        assert!(!ctx.is_focus_released());
        assert_eq!(ctx.take_ime_state(), Some(published));
    }

    #[test]
    fn key_and_ime_events_are_focus_routed_with_zero_position() {
        let key = InputEvent::Key(KeyEvent {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers::default(),
            repeat: false,
        });
        assert!(key.is_focus_routed());
        assert_eq!(key.position(), Point::ZERO);
        // translated is identity for focus-routed events.
        assert_eq!(key.translated(Vec2::new(5.0, 5.0)), key);

        let ime = InputEvent::Ime(ImeEvent::Commit("x".to_string()));
        assert!(ime.is_focus_routed());
        assert!(!down(1.0, 1.0).is_focus_routed());
    }
}
