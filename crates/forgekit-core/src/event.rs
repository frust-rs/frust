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

use kurbo::{Point, Size, Vec2};

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

/// An input event delivered to the widget tree.
///
/// The two families widgets handle in Phase 4A: pointer gestures and scroll.
/// Keyboard/IME arrive with TextInput (Phase 4B) and are intentionally absent.
#[derive(Clone, Copy, Debug, PartialEq)]
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
}

impl InputEvent {
    /// The event's location, in the receiving widget's local coordinate space.
    pub fn position(&self) -> Point {
        match self {
            InputEvent::Pointer(p) => p.position,
            InputEvent::Scroll { position, .. } => *position,
        }
    }

    /// Return a copy of this event with its position shifted by `offset`.
    ///
    /// Containers use this (with `offset = -child_origin`) to translate an event
    /// from their own coordinate space into a child's local space before
    /// forwarding it — see [`crate::widget::ChildPod::event_child`].
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
        }
    }
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

    /// The receiving widget's origin in its parent's coordinate space.
    pub fn origin(&self) -> Point {
        self.origin
    }

    /// The receiving widget's resolved size.
    pub fn size(&self) -> Size {
        self.size
    }

    /// Create a fresh sub-context for a child at `origin`/`size`, reborrowing the
    /// same erased state. The child's `needs_redraw`/`capture_requested` start
    /// clear; the parent folds them back in with [`EventCtx::absorb_child`].
    pub(crate) fn child_ctx(&mut self, origin: Point, size: Size) -> EventCtx<'_> {
        EventCtx {
            state: &mut *self.state,
            needs_redraw: false,
            capture_requested: false,
            origin,
            size,
        }
    }

    /// Fold a child dispatch's redraw/capture flags back into this context.
    pub(crate) fn absorb_child(&mut self, child_needs_redraw: bool, child_captured: bool) {
        self.needs_redraw |= child_needs_redraw;
        self.capture_requested |= child_captured;
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
            let mut child = ctx.child_ctx(Point::new(1.0, 2.0), Size::new(3.0, 4.0));
            child.request_redraw();
            child.capture_pointer();
            let (redraw, cap) = (child.needs_redraw(), child.is_pointer_captured());
            ctx.absorb_child(redraw, cap);
        }
        assert!(ctx.needs_redraw());
        assert!(ctx.is_pointer_captured());
    }
}
