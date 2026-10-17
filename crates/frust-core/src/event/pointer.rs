use kurbo::Point;

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

/// The kind of device a pointer contact comes from — one half of a
/// [`PointerId`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PointerSource {
    /// The mouse (or any single-cursor host pointer a shell reports as one).
    Mouse,
    /// A touch contact — one finger on a touchscreen.
    Touch,
}

/// The identity of one pointer contact: which device it comes from and which
/// slot on that device.
///
/// It rides **beside** a [`PointerEvent`], never inside it: a shell hands a
/// touch contact to the root as [`InputEvent::PointerContact`](crate::event::InputEvent::PointerContact), the root
/// unwraps it, and the widget receiving the plain [`InputEvent::Pointer`](crate::event::InputEvent::Pointer) reads
/// the identity from [`EventCtx::pointer_id`](crate::event::EventCtx::pointer_id). A bare `InputEvent::Pointer`
/// from a shell means [`PointerId::MOUSE`].
///
/// A slot is the shell's own numbering of simultaneous contacts on one source:
/// slot `0` is the gesture's first contact, and a slot is free again once its
/// contact ended. The mouse only ever has slot `0`.
///
/// # Multi-contact contract
///
/// Applied at the root ([`crate::app::RenderRoot::event`]):
///
/// * **(a)** With no live pointer capture, a slot-`0` contact is hit-tested
///   exactly like a plain [`InputEvent::Pointer`](crate::event::InputEvent::Pointer) — the same overlay, hover,
///   focus and blur bookkeeping — and a capture taken on its `Down` latches
///   *this* id as the gesture's **claimant**.
/// * **(b)** With no live capture, a contact on slot `1` or above is **dropped**
///   at the root. Additional contacts exist only inside a captured gesture.
/// * **(c)** While a capture is live, the claimant's own events take the
///   captured path as usual. An event with any other id reaches the captor —
///   and only the captor: the containers above it on that path forward it
///   without running their own handling — only if the captor opted in with
///   [`EventCtx::capture_contacts`](crate::event::EventCtx::capture_contacts) on its capturing `Down`; otherwise the root
///   drops it. Only the claimant's `Up`/`Cancel` ends the capture: another
///   contact's `Up`/`Cancel` never releases it, whether it came from a touch
///   while a mouse holds the capture or the other way round. A container that
///   takes the gesture over from the captor ([`EventCtx::release_captured_child`](crate::event::EventCtx::release_captured_child))
///   ends the opt-in, and the other contacts are dropped from then on.
///
/// See [`InputEvent::PointerContact`](crate::event::InputEvent::PointerContact) for the full contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PointerId {
    /// The device the contact comes from.
    pub source: PointerSource,
    /// The contact's slot on that device (`0` for the first contact).
    pub slot: u32,
}

impl PointerId {
    /// The mouse pointer: the identity a bare [`InputEvent::Pointer`](crate::event::InputEvent::Pointer) carries,
    /// and what [`EventCtx::pointer_id`](crate::event::EventCtx::pointer_id) reports when nothing else is known.
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

/// The pointer cursor a widget asks the host to display.
///
/// A **request vocabulary**, not a rendering one: platform-neutral names a
/// widget states its intent in ([`EventCtx::set_cursor`](crate::event::EventCtx::set_cursor)), which a desktop shell
/// maps onto its own host API — `frust-shell-desktop` onto winit's own cursor
/// icons, the one place any of these names touches a platform. Deliberately
/// tiny: the shapes a desktop-class design system actually needs, not a full CSS
/// cursor set.
///
/// `#[non_exhaustive]` from birth, so widening it later cannot break an
/// out-of-tree `match` (a shell or design system must carry a wildcard arm and
/// degrade an unknown request to [`CursorIcon::Default`] rather than fail to
/// compile).
///
/// **Nothing below a desktop shell honours a request.** The mobile shells never
/// read the resolved value — a touch host has no pointer to shape — so a widget
/// may set a cursor unconditionally and get the desktop behaviour where it
/// exists and no behaviour at all where it does not.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CursorIcon {
    /// The host's ordinary arrow. The resolved value of any pass in which no
    /// widget asked for anything else, so a widget never has to ask for it to
    /// "give the cursor back" (see [`EventCtx::set_cursor`](crate::event::EventCtx::set_cursor)).
    #[default]
    Default,
    /// The clickable hand: buttons, links, and anything else a press activates.
    Pointer,
    /// The text I-beam: editable or selectable text.
    Text,
    /// An open hand: this is draggable, and no drag has started yet.
    Grab,
    /// A closed hand: a drag is in progress.
    Grabbing,
    /// A column-resize handle — a divider the pointer moves horizontally.
    ColResize,
    /// A row-resize handle — a divider the pointer moves vertically.
    RowResize,
    /// The action under the pointer is refused: a disabled control, or a drop
    /// target rejecting what is being dragged.
    NotAllowed,
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

/// The lifecycle phase of a scale (pinch/zoom) gesture.
///
/// No `Cancel`: every shipped source — the desktop ctrl/⌘+wheel mapping and
/// macOS's `PinchGesture` — reports a clean bracket (or, for an ordinary
/// notch wheel, a lone [`Update`](ScalePhase::Update) with no bracket at
/// all), so there is nothing yet for a cancelled variant to mean. Widened the
/// day a source needs one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScalePhase {
    /// The gesture began.
    Begin,
    /// The gesture continued; the event's `scale_delta`/`focal`/`velocity`
    /// describe this increment.
    Update,
    /// The gesture ended normally.
    End,
}

/// A scale (pinch/zoom) gesture event — [`InputEvent::Scroll`](crate::event::InputEvent::Scroll)'s hit-tested
/// sibling, carrying a *multiplicative* delta and a focal point rather than
/// an additive one.
///
/// Any source that reports a scale-factor change rather than individual
/// contact moves reduces to this one event — a desktop shell's ctrl/⌘+wheel
/// mapping, macOS's `PinchGesture`, and eventually a touch two-finger pinch
/// recognizer — so a widget reacts to pinch-to-zoom the same way regardless
/// of input device.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScaleEvent {
    /// The gesture phase.
    pub phase: ScalePhase,
    /// The multiplicative scale change this event represents — not a running
    /// total. A consumer multiplies its own accumulated scale by this value
    /// each time an event arrives: `1.0` is a no-op, `>1.0` zooms in, `<1.0`
    /// zooms out.
    pub scale_delta: f64,
    /// Where the gesture is centered, in the receiving widget's local
    /// logical space — the point that must stay visually fixed while scale
    /// changes. Translated like [`InputEvent::Pointer`](crate::event::InputEvent::Pointer)'s position by the
    /// container chain that routes it (see [`InputEvent::translated`](crate::event::InputEvent::translated)).
    pub focal: Point,
    /// The gesture's current rate of scale change, per second. `0.0` when the
    /// source reports none — every shipped desktop source today, since
    /// neither a wheel notch nor winit's `PinchGesture` carries a velocity —
    /// reserved for a recognizer that tracks contact velocity directly.
    pub velocity: f64,
}
