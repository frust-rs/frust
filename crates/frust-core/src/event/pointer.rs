//! Pointer, cursor, scroll and scale vocabulary: what a hit-tested event carries.

use kurbo::Point;

/// Which button a pointer event carries; touch and pen contacts report `Primary`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerButton {
    Primary,
    Secondary,
    Middle,
}

/// A gesture's phase: `Down`, zero or more `Move`s, then `Up` or `Cancel` (the platform stole
/// the gesture). Both terminal phases release capture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerPhase {
    Down,
    Move,
    Up,
    Cancel,
}

/// One pointer event: three fields — phase, position, button — and no pointer-kind axis.
/// `position` is logical pixels in the receiving widget's local space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointerEvent {
    pub phase: PointerPhase,
    pub position: Point,
    pub button: PointerButton,
}

/// The kind of device a pointer contact comes from — one half of a [`PointerId`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PointerSource {
    Mouse,
    Touch,
}

/// A contact's identity: its source plus the shell's slot (`0` is the gesture's first contact;
/// the mouse only has `0`). It rides beside a [`PointerEvent`], never inside it; see
/// [`InputEvent::PointerContact`](crate::event::InputEvent::PointerContact) for the contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PointerId {
    pub source: PointerSource,
    pub slot: u32,
}

impl PointerId {
    /// The mouse: the identity a bare `InputEvent::Pointer` carries.
    pub const MOUSE: PointerId = PointerId {
        source: PointerSource::Mouse,
        slot: 0,
    };

    /// The touch contact in `slot` (`0` for the gesture's first finger).
    pub const fn touch(slot: u32) -> PointerId {
        PointerId {
            source: PointerSource::Touch,
            slot,
        }
    }
}

/// The platform-neutral cursor a widget requests through `EventCtx::set_cursor`; only a desktop
/// shell honours it. `#[non_exhaustive]`: an out-of-tree `match` degrades an unknown variant to
/// [`CursorIcon::Default`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CursorIcon {
    /// The host arrow; what a pass in which no widget asked resolves to.
    #[default]
    Default,
    /// The clickable hand.
    Pointer,
    Text,
    Grab,
    Grabbing,
    ColResize,
    RowResize,
    /// A refused action: a disabled control, or a drop target rejecting the drag.
    NotAllowed,
}

/// A scroll amount `(x, y)`: `Lines` from a mouse wheel, `Pixels` (logical) from a precision
/// trackpad or touch.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ScrollDelta {
    Lines(f64, f64),
    Pixels(f64, f64),
}

/// A scale gesture's phase. No `Cancel`: every shipped source reports a clean bracket, or a lone
/// `Update` for a notch wheel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScalePhase {
    Begin,
    Update,
    End,
}

/// A pinch/zoom event — [`InputEvent::Scroll`](crate::event::InputEvent::Scroll)'s hit-tested
/// sibling, carrying a multiplicative delta and a focal point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScaleEvent {
    pub phase: ScalePhase,
    /// This event's multiplicative change, not a running total: `1.0` is a no-op.
    pub scale_delta: f64,
    /// The fixed point of the zoom, in local logical space; translated like a pointer position.
    pub focal: Point,
    /// Rate of scale change per second; `0.0` from every shipped source, which reports none.
    pub velocity: f64,
}
