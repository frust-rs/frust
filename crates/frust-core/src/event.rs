//! Layer 2 input: pointer/scroll events and the [`EventCtx`] a widget mutates
//! while handling them.
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
//! on window-leave) — the claimant's own, when more than one contact is down:
//! [`InputEvent::PointerContact`] states the multi-contact contract, and
//! [`EventCtx::pointer_id`]/[`EventCtx::capture_contacts`] are a widget's side of it.
//!
//! # Hover is a claim, not a phase
//!
//! There is no Enter/Leave phase, and [`PointerPhase`] deliberately gains none:
//! hover is an **opt-in claim** a widget makes from its ordinary uncaptured
//! [`PointerPhase::Move`] arm ([`EventCtx::claim_hover`]), recorded as a path
//! through the pod chain the same way focus is. The claim's identity is an
//! *epoch*: [`crate::app::RenderRoot`] advances one hover epoch per hover pass
//! (an uncaptured `Move`, or the `Down`/`Up`/`Cancel` that ends a hover
//! outright), a claim stamps that epoch onto every
//! [`ChildPod`](crate::widget::ChildPod) from the claimant up to the root, and a
//! link only counts as hovered while its stamp still matches the live epoch. So
//! the previous claimant needs no explicit clearing — the pointer moving anywhere
//! else advances the epoch and its stamp goes stale by construction, which is why
//! a container that never hears about the move cannot leave a stale path standing.
//!
//! Stranding needs a hover pass, and there is exactly one way for a link to lose
//! its owner without one: a rebuild that *removes* the claimant, which will never
//! see another `Move`. A dropped [`ChildPod`](crate::widget::ChildPod) holding the
//! live link therefore reports itself, and
//! [`RenderRoot::rebuild`](crate::app::RenderRoot::rebuild) ends the hover before
//! the frame does — the hover twin of the focus-orphan release.
//!
//! The recorded thing is a **path**, exactly like focus, and both hover reads
//! report membership of it: the claimant *and* every ancestor enclosing it read
//! hovered, the way CSS `:hover` applies to an element while the pointer is over
//! one of its descendants. Nothing off the path does — a sibling, or a widget
//! whose descendant did not claim, reads `false`.
//!
//! Only an **uncaptured** `Move` may claim: the root marks a captured pass
//! ineligible outright, and [`ChildPod::event_child`](crate::widget::ChildPod::event_child)
//! additionally refuses a claim from inside a pod that itself holds the capture
//! path, so a drag can never paint hover under the finger. At most one claim per
//! pass is recorded — the **first one recorded wins**, and every later claim in
//! that pass is ineligible — so at most one path is hovered and two *stacked*
//! widgets cannot each hold their own link. First-recorded is the topmost
//! (deepest) claimant only while every container claims **after** routing the
//! move to its children, which is what [`EventCtx::claim_hover`]'s contract
//! requires of one: an ancestor that claims *before* it forwards is recorded
//! first instead, and starves its whole subtree for the pass.
//!
//! # The cursor is a per-pass request, on its own channel
//!
//! [`EventCtx::set_cursor`] is hover's sibling and deliberately **not** derived
//! from it: the root's hover mirror is identity-free (it knows *that* something
//! is hovered, not which widget or what shape that widget wants), so the cursor
//! gets its own channel — one slot per pass, last writer wins, resolved by
//! [`crate::app::RenderRoot::event`] into [`crate::app::RenderRoot::cursor`] for
//! a desktop shell to apply. Absence resolves to [`CursorIcon::Default`], so a
//! widget that stops asking needs no clearing, and only a pointer
//! [`PointerPhase::Move`] re-resolves — a captured `Move` included, which is what
//! lets a drag keep its own cursor outside its bounds.

use std::any::Any;
use std::cell::Cell;
use std::fmt;
use std::thread::LocalKey;

use kurbo::{Affine, Point, Rect, Size, Vec2};

use crate::overlay::OverlayKey;

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
/// touch contact to the root as [`InputEvent::PointerContact`], the root
/// unwraps it, and the widget receiving the plain [`InputEvent::Pointer`] reads
/// the identity from [`EventCtx::pointer_id`]. A bare `InputEvent::Pointer`
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
///   exactly like a plain [`InputEvent::Pointer`] — the same overlay, hover,
///   focus and blur bookkeeping — and a capture taken on its `Down` latches
///   *this* id as the gesture's **claimant**.
/// * **(b)** With no live capture, a contact on slot `1` or above is **dropped**
///   at the root. Additional contacts exist only inside a captured gesture.
/// * **(c)** While a capture is live, the claimant's own events take the
///   captured path as usual. An event with any other id reaches the captor —
///   and only the captor: the containers above it on that path forward it
///   without running their own handling — only if the captor opted in with
///   [`EventCtx::capture_contacts`] on its capturing `Down`; otherwise the root
///   drops it. Only the claimant's `Up`/`Cancel` ends the capture: another
///   contact's `Up`/`Cancel` never releases it, whether it came from a touch
///   while a mouse holds the capture or the other way round. A container that
///   takes the gesture over from the captor ([`EventCtx::release_captured_child`])
///   ends the opt-in, and the other contacts are dropped from then on.
///
/// See [`InputEvent::PointerContact`] for the full contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PointerId {
    /// The device the contact comes from.
    pub source: PointerSource,
    /// The contact's slot on that device (`0` for the first contact).
    pub slot: u32,
}

impl PointerId {
    /// The mouse pointer: the identity a bare [`InputEvent::Pointer`] carries,
    /// and what [`EventCtx::pointer_id`] reports when nothing else is known.
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
/// widget states its intent in ([`EventCtx::set_cursor`]), which a desktop shell
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
    /// "give the cursor back" (see [`EventCtx::set_cursor`]).
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

/// A scale (pinch/zoom) gesture event — [`InputEvent::Scroll`]'s hit-tested
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
    /// changes. Translated like [`InputEvent::Pointer`]'s position by the
    /// container chain that routes it (see [`InputEvent::translated`]).
    pub focal: Point,
    /// The gesture's current rate of scale change, per second. `0.0` when the
    /// source reports none — every shipped desktop source today, since
    /// neither a wheel notch nor winit's `PinchGesture` carries a velocity —
    /// reserved for a recognizer that tracks contact velocity directly.
    pub velocity: f64,
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
    /// The dedicated hardware **Copy** key (winit's `NamedKey::Copy`), present on
    /// full-size and multimedia keyboards. Semantically identical to the
    /// platform copy chord, but it arrives as a key rather than as a modifier
    /// combination, so a shell maps it straight onto
    /// [`EditCommand::Copy`] instead of asking a widget to decode a chord.
    Copy,
    /// The dedicated hardware **Cut** key (winit's `NamedKey::Cut`) — the
    /// [`Copy`](NamedKey::Copy) note applies verbatim, mapping onto
    /// [`EditCommand::Cut`].
    Cut,
    /// The dedicated hardware **Paste** key (winit's `NamedKey::Paste`) — the
    /// [`Copy`](NamedKey::Copy) note applies verbatim. A shell answers it the way
    /// it answers any paste: by reading the host clipboard and dispatching
    /// [`EditCommand::Paste`] with the text.
    Paste,
    /// **Insert** — carried for the legacy clipboard chords rather than for an
    /// overtype mode: `Shift+Insert` is paste and `Ctrl+Insert` is copy on
    /// Windows, Linux, and most X11 terminals, which is the only reason this key
    /// is enumerated here (nothing in this workspace toggles overtype).
    Insert,
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

/// A semantic clipboard / selection command delivered to the focused editable.
///
/// The **decoded** form of a platform gesture, not the gesture itself: a shell
/// resolves `Cmd+C` / `Ctrl+C` / [`NamedKey::Copy`] / `Ctrl+Insert` / an Android
/// `ACTION_PROCESS_TEXT` / an iOS edit-menu tap into one of these variants and
/// dispatches it as [`InputEvent::EditCommand`], so no widget has to know which
/// chord means copy on which OS. Every widget sees the same four verbs.
///
/// # Why paste carries its text and copy does not
///
/// The clipboard itself lives in the shell (only the shell has a host clipboard
/// to talk to), and the two directions are deliberately asymmetric:
///
/// * [`Copy`](EditCommand::Copy) / [`Cut`](EditCommand::Cut) carry nothing —
///   the widget owns the selection, so it answers by writing its own text into
///   the pass's clipboard slot ([`EventCtx::write_clipboard`]), which the shell
///   drains and hands to the host.
/// * [`Paste`](EditCommand::Paste) carries the text — the *shell* owns the
///   host clipboard, so by the time the command reaches the tree the read has
///   already happened. A widget that wants a paste it did not receive asks for
///   one ([`EventCtx::request_paste`]) and the shell answers with this variant.
///
/// # Refusal is the widget's call
///
/// Nothing here is a permission: a read-only or secret field is free to ignore
/// a [`Copy`](EditCommand::Copy)/[`Cut`](EditCommand::Cut) it does not want to
/// honour, and a widget with no selection simply reports
/// [`EventResult::Ignored`]. The vocabulary states what was *asked for*.
///
/// Exhaustive on purpose (no `#[non_exhaustive]`): these four verbs are the
/// whole clipboard contract, and a widget matching on them should be told by
/// the compiler if that ever stops being true.
#[derive(Clone, PartialEq, Eq)]
pub enum EditCommand {
    /// Copy the current selection to the host clipboard, leaving the document
    /// unchanged. A widget answers by calling [`EventCtx::write_clipboard`].
    Copy,
    /// Copy the current selection and delete it. A widget answers by calling
    /// [`EventCtx::write_clipboard`] *and* mutating its own text — the shell
    /// sees one clipboard write either way (see that method's last-writer rule).
    Cut,
    /// Replace the current selection with this text (insert it at the caret when
    /// there is no selection). Already read from the host clipboard by the shell.
    Paste(String),
    /// Select the widget's entire content — the selection half of this
    /// vocabulary, carried here because it arrives through the same platform
    /// chords and edit menus as the other three.
    SelectAll,
}

impl fmt::Debug for EditCommand {
    /// Hand-written so pasted text never reaches a log.
    ///
    /// [`ImeState`]'s reason, one step earlier in the pipeline (see its `Debug`):
    /// a paste payload is arbitrary host-clipboard content — a password manager's
    /// fill, a copied token, a recovery phrase — and unlike an IME surface there
    /// is no content-type hint to key the decision off, because the *clipboard*
    /// has no owner to state one. So the payload is unconditionally replaced by
    /// `<redacted>` (no length, which would itself leak), and the variant name
    /// still prints so a trace stays readable.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EditCommand::Copy => f.write_str("Copy"),
            EditCommand::Cut => f.write_str("Cut"),
            EditCommand::Paste(_) => f.debug_tuple("Paste").field(&"<redacted>").finish(),
            EditCommand::SelectAll => f.write_str("SelectAll"),
        }
    }
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
/// `frust-text` convert to/from Rust byte offsets at their own boundary.
/// Core carries the value opaquely and makes no index interpretation.
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

impl Default for EditingState {
    /// An empty field with no selection and no composing region.
    ///
    /// Note this is **not** the derived default: the anchors are the `−1`
    /// "none" sentinel, not `0` (which would mean a real caret at offset 0).
    fn default() -> Self {
        Self {
            text: String::new(),
            selection_base: -1,
            selection_extent: -1,
            composing_base: -1,
            composing_extent: -1,
        }
    }
}

/// An input-method (IME) event delivered down the focus path (never hit-tested).
///
/// Desktop drives [`ImeEvent::Compose`]/[`ImeEvent::Commit`] from winit's
/// `Ime::Preedit`/`Ime::Commit`; the mobile bridges push whole values via
/// [`ImeEvent::ApplyEditingState`] (state-sync, not op-forwarding).
/// [`ImeEvent::Enabled`]/[`ImeEvent::Disabled`] bracket a
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

/// What an [`InputEvent::Overlay`] carries into a floated surface.
///
/// The key names the surface's owner (see [`OverlayKey`]); the kind is the input
/// itself, always in **absolute window space** rather than in anyone's local
/// space — see [`OverlayEventKind`].
#[derive(Clone, Debug, PartialEq)]
pub struct OverlayEvent {
    /// The owner whose registered surface the root hit. Every other widget in
    /// the tree sees this broadcast and must ignore it.
    pub key: OverlayKey,
    /// What happened.
    pub kind: OverlayEventKind,
}

/// The input an [`OverlayEvent`] delivers.
///
/// # Window space, not local space
///
/// Every position here is absolute logical window space, deliberately: the
/// broadcast reaches the owner by travelling the *main* tree, so the translation
/// chain it passes through on the way (`ChildPod::event_child` subtracting each
/// container's origin) describes the owner's position, not the floated pod's.
/// Translating the payload would therefore corrupt it. The owner instead
/// subtracts its own registered
/// [`window_rect`](crate::overlay::OverlayEntry::window_rect) origin before
/// forwarding into the pod, which is the only offset that means anything — which
/// is also why [`InputEvent::translated`] returns an overlay event unchanged.
#[derive(Clone, Debug, PartialEq)]
pub enum OverlayEventKind {
    /// A pointer event inside the surface's rect, positioned in window space.
    Pointer(PointerEvent),
    /// A scroll inside the surface's rect, positioned in window space.
    Scroll {
        /// Where the scroll occurred, in absolute window space.
        position: Point,
        /// How much to scroll.
        delta: ScrollDelta,
    },
    /// A scale gesture inside the surface's rect, focal point in window
    /// space — the overlay mirror of [`InputEvent::Scale`], routed here on
    /// exactly the same hit-test terms as [`Scroll`](OverlayEventKind::Scroll).
    Scale {
        /// Where the gesture is centered, in absolute window space.
        focal: Point,
        /// The gesture phase.
        phase: ScalePhase,
        /// The multiplicative scale change this event represents.
        scale_delta: f64,
        /// The gesture's current rate of scale change, per second.
        velocity: f64,
    },
    /// A primary press landed outside **every** registered surface — the
    /// light-dismiss notification, delivered only to entries registered
    /// [`OutsideTap::Notify`](crate::overlay::OutsideTap::Notify). It carries no
    /// position: where the press landed is the main tree's business, and an
    /// owner that wants it can register `consume: false` and watch the press
    /// arrive there normally.
    OutsideDown,
}

/// An input event delivered to the widget tree.
///
/// Pointer gestures, scroll, and scale are **hit-tested** (routed by position);
/// keyboard, IME, and edit-command events are **focus-routed** — delivered
/// straight down the recorded focus chain with no hit test and no meaningful
/// position (see [`crate::widget::ChildPod`]'s focus bookkeeping and
/// `frust-widgets`' `route_event`). [`InputEvent::Housekeeping`] and
/// [`InputEvent::Overlay`] are neither: they are **broadcasts** that reach
/// every child unconditionally.
#[derive(Clone, Debug, PartialEq)]
pub enum InputEvent {
    /// A pointer (mouse/touch/pen) gesture event.
    ///
    /// From a shell it means the same as
    /// [`PointerContact`](InputEvent::PointerContact) with [`PointerId::MOUSE`].
    /// It is also the **only** pointer form a widget ever receives: the root
    /// unwraps a `PointerContact` into this variant and reports the contact's
    /// identity through [`EventCtx::pointer_id`].
    Pointer(PointerEvent),
    /// One identified pointer contact — the shell-facing carrier for a touch
    /// contact (or any pointer that is not [`PointerId::MOUSE`]).
    ///
    /// **Widgets never receive this variant.**
    /// [`RenderRoot::event`](crate::app::RenderRoot::event) unwraps it and
    /// dispatches [`InputEvent::Pointer`]`(event)` with
    /// [`EventCtx::pointer_id`] reporting `pointer_id`, so every existing
    /// `match` on `InputEvent::Pointer` keeps working unchanged and a widget
    /// that does not care which contact it is seeing never has to ask.
    ///
    /// # Multi-contact contract
    ///
    /// The root decides what a contact does from its id and the capture latch
    /// it holds (the latch records the **claimant**: the id whose `Down` took the
    /// capture).
    ///
    /// * **(a) Slot 0 with no live capture is hit-tested exactly like
    ///   [`InputEvent::Pointer`].** The overlay pre-pass, hover, focus and
    ///   blur-on-outside-tap bookkeeping are the same code path, so a
    ///   single-finger gesture behaves identically whichever carrier delivered
    ///   it. A capture taken on its `Down` latches `pointer_id` as the claimant.
    /// * **(b) Slot 1 and above with no live capture is dropped at the root.**
    ///   Additional contacts exist only inside a captured gesture; one that
    ///   arrives while nothing holds the pointer reaches no widget and moves no
    ///   root state.
    /// * **(c) While a capture is live, routing is keyed on the claimant.** The
    ///   claimant's own events take the captured path exactly as before. An
    ///   event from **any other id** is delivered down the same captured path
    ///   to the captor — as `InputEvent::Pointer`, with
    ///   [`EventCtx::pointer_id`] reporting that id — only if the captor called
    ///   [`EventCtx::capture_contacts`] on the `Down` it captured with;
    ///   otherwise it is dropped at the root. **Only the claimant's
    ///   `Up`/`Cancel` releases the capture.** Another contact's `Up`/`Cancel`
    ///   never does — at the root or in any container's recorded active path —
    ///   so a finger lifting elsewhere cannot break a mouse drag, nor a mouse
    ///   release a touch drag.
    ///
    ///   **The captor is the only widget that sees another contact.** The
    ///   delivery walks the recorded active path *forward-only*: every
    ///   container between the root and the captor hands it on without its own
    ///   pointer handling running (see
    ///   [`ChildPod::event_child`](crate::widget::ChildPod::event_child) for the
    ///   mechanism), so a scroll view or gesture detector enclosing a pinch
    ///   recognizer never sees the second finger as a `Down` of its own; the
    ///   captor's own handler, and whatever it routes below itself, run as
    ///   usual. If the walk cannot reach the captor through a container (an
    ///   overlay owner whose captured pod is a floated surface), that container
    ///   alone is handed the event the ordinary way.
    ///
    ///   **A takeover ends the opt-in.** A container that cancels the captor
    ///   and keeps the gesture for itself releases it with
    ///   [`EventCtx::release_captured_child`]; the root then stops routing the
    ///   other contacts (they fall under rule (c)'s drop branch), while the
    ///   claimant keeps the capture — now held by that container — until its own
    ///   `Up`/`Cancel`.
    ///
    /// A delivered non-claimant contact is not a gesture of its own: it opens or
    /// moves no capture, takes no hover pass, resolves no cursor, and blurs
    /// nothing (an explicit [`EventCtx::request_focus`]/[`EventCtx::release_focus`]
    /// from its handler is still honoured, as it is for a scroll). When the
    /// claimant's `Up`/`Cancel` ends the capture, the captor must treat every
    /// other contact it was tracking as ended too: their later events fall under
    /// rule (b) and never reach it.
    ///
    /// A bare [`InputEvent::Pointer`] from a shell is this variant with
    /// [`PointerId::MOUSE`], so a host that emits only `Pointer` (desktop, web)
    /// sees exactly the single-pointer behaviour it always had.
    PointerContact {
        /// Which contact this is.
        pointer_id: PointerId,
        /// The contact's event, positioned like any [`InputEvent::Pointer`].
        event: PointerEvent,
    },
    /// A scroll event at `position` (local logical space) carrying `delta`.
    Scroll {
        /// Where the scroll occurred, in the receiving widget's local space.
        position: Point,
        /// How much to scroll.
        delta: ScrollDelta,
    },
    /// A scale (pinch/zoom) gesture event — hit-tested exactly like
    /// [`Scroll`](InputEvent::Scroll), by its [`ScaleEvent::focal`] point, and
    /// bubbles up the tree until a widget reports [`EventResult::Handled`].
    /// See [`ScaleEvent`] for the field contract.
    Scale(ScaleEvent),
    /// A keyboard key event, routed down the focus path (no hit test).
    Key(KeyEvent),
    /// An IME event, routed down the focus path (no hit test).
    Ime(ImeEvent),
    /// A decoded clipboard / selection command, routed down the focus path (no
    /// hit test) exactly like [`Key`](InputEvent::Key) and [`Ime`](InputEvent::Ime).
    ///
    /// Focus-routed rather than hit-tested because a clipboard verb is *about
    /// the selection*, and the selection lives wherever focus is — a `Cmd+V`
    /// carries no pointer position, and an edit-menu tap's position is the
    /// menu's, not the field's. Focus routing is also what makes a paste with
    /// nothing focused a harmless no-op: the event reaches no widget and is
    /// dropped, so a shell may answer a stale paste request unconditionally
    /// (see [`EventCtx::request_paste`]).
    EditCommand(EditCommand),
    /// **Not user input**: a state-bearing housekeeping pass, broadcast to the
    /// whole tree so a widget that queued a callback needing `&mut State` during
    /// a state-free `BuildCtx` pass can run it.
    ///
    /// # Why it exists
    ///
    /// [`crate::app::RenderRoot::rebuild`] is the only unconditional per-frame
    /// pass holding `&mut State`, and it hands that state to `app_logic` alone —
    /// the view diff itself (and therefore every `View::rebuild`, where a
    /// navigator applies its queued push/pop ops) is state-free. A widget that
    /// needs to call back into app state from there had, before this variant, no
    /// pass to run in except the *next event*, which on a touch device may be
    /// seconds away or may never reach that widget at all (a pop-result
    /// callback measured 3.2s late on device, and was lost entirely when the
    /// next tap was consumed by chrome outside the navigator).
    /// `rebuild` now dispatches this variant instead, so the deferred callback
    /// runs on the very frame that queued it.
    ///
    /// # Routing contract
    ///
    /// **Broadcast, never consumed.** It carries no position, is not hit-tested,
    /// and is not focus-routed: a container forwards it to *every* child
    /// unconditionally (before any capture/focus/hit-test branch) and reports
    /// [`EventResult::Ignored`] regardless of what the children returned, so no
    /// "first handler wins" short-circuit can hide a subtree from it. A leaf
    /// widget with nothing deferred simply ignores it — the fall-through is
    /// harmless by construction. It never opens or releases a capture, never
    /// moves focus, and never blurs.
    ///
    /// # Naming
    ///
    /// Deliberately *not* `Tick`: `Tick` already means frame pacing in this
    /// codebase ([`crate::widget::TickClass`]), and this variant has nothing to
    /// do with the frame gate.
    Housekeeping,
    /// **Not user input either**: one floated overlay surface's own input,
    /// broadcast to the whole tree so it reaches the owner that registered the
    /// surface, wherever in the tree that owner sits.
    ///
    /// # Why a broadcast
    ///
    /// The owner of a floated surface is an ordinary widget somewhere in the
    /// tree, and the pointer that hit its surface is nowhere near its own bounds
    /// — that is the entire point of floating. Hit-testing the event would
    /// therefore deliver it to whatever the main tree has under the pointer, and
    /// focus-routing it would deliver it to a text field that has nothing to do
    /// with the surface. Broadcasting is the only route that reaches the owner
    /// without knowing where it is, so this is the **second** broadcast variant
    /// (see [`InputEvent::is_broadcast`]), and every routing helper's existing
    /// broadcast-first branch already forwards it correctly with no change.
    ///
    /// # Routing contract
    ///
    /// **Only the owner whose [`OverlayKey`] matches acts on it; every other
    /// widget ignores it.** A container forwards it to every child
    /// unconditionally — no hit test, no capture fast path, no focus gate — and
    /// reports [`EventResult::Ignored`] regardless, exactly like
    /// [`Housekeeping`](InputEvent::Housekeeping). A widget that is not an
    /// overlay owner, or whose key differs, must fall through: the key
    /// comparison is the whole addressing mechanism.
    ///
    /// At the root it is inert in the ways a broadcast must be — it advances no
    /// hover epoch and never blurs — but, unlike `Housekeeping`, it *is* a real
    /// user gesture underneath, so a focus request or a pointer capture bubbled
    /// from inside the surface is honoured (see
    /// [`crate::app::RenderRoot::event`]).
    Overlay(OverlayEvent),
}

impl InputEvent {
    /// The event's location, in the receiving widget's local coordinate space.
    ///
    /// [`InputEvent::Scale`] reports its [`ScaleEvent::focal`] point here, the
    /// same way [`InputEvent::Scroll`] reports `position`. Focus-routed events
    /// ([`InputEvent::Key`]/[`InputEvent::Ime`]/
    /// [`InputEvent::EditCommand`]) and the two broadcasts
    /// ([`Housekeeping`](InputEvent::Housekeeping) and
    /// [`Overlay`](InputEvent::Overlay)) have no spatial position — they are
    /// delivered down the focus chain, or to every child, not hit-tested — so
    /// this reports [`Point::ZERO`] for them; callers must never hit-test on it
    /// (routing helpers early-return both classes). An overlay event's *payload*
    /// does carry a position, but in window space rather than in the receiver's
    /// local space, which is precisely why it is not reported here (see
    /// [`OverlayEventKind`]).
    pub fn position(&self) -> Point {
        match self {
            InputEvent::Pointer(p) | InputEvent::PointerContact { event: p, .. } => p.position,
            InputEvent::Scroll { position, .. } => *position,
            InputEvent::Scale(scale) => scale.focal,
            InputEvent::Key(_)
            | InputEvent::Ime(_)
            | InputEvent::EditCommand(_)
            | InputEvent::Housekeeping
            | InputEvent::Overlay(_) => Point::ZERO,
        }
    }

    /// Return a copy of this event with its position shifted by `offset`.
    ///
    /// Containers use this (with `offset = -child_origin`) to translate an event
    /// from their own coordinate space into a child's local space before
    /// forwarding it — see [`crate::widget::ChildPod::event_child`].
    /// [`InputEvent::Scale`] shifts its [`ScaleEvent::focal`] point the same way
    /// [`InputEvent::Scroll`] shifts its `position`. Focus-routed
    /// events ([`InputEvent::Key`]/[`InputEvent::Ime`]/
    /// [`InputEvent::EditCommand`]) and the
    /// [`Housekeeping`](InputEvent::Housekeeping) broadcast carry no position, so
    /// they are returned unchanged (cloned). An
    /// [`Overlay`](InputEvent::Overlay) event is returned unchanged for the
    /// opposite reason — its payload carries a **window-space** position that the
    /// container chain between the root and the owner must not shift, since that
    /// chain describes where the *owner* sits and not where the floated surface
    /// does (see [`OverlayEventKind`]).
    pub fn translated(&self, offset: Vec2) -> InputEvent {
        match self {
            InputEvent::Pointer(p) => InputEvent::Pointer(PointerEvent {
                position: p.position + offset,
                ..*p
            }),
            InputEvent::PointerContact { pointer_id, event } => InputEvent::PointerContact {
                pointer_id: *pointer_id,
                event: PointerEvent {
                    position: event.position + offset,
                    ..*event
                },
            },
            InputEvent::Scroll { position, delta } => InputEvent::Scroll {
                position: *position + offset,
                delta: *delta,
            },
            InputEvent::Scale(scale) => InputEvent::Scale(ScaleEvent {
                focal: scale.focal + offset,
                ..*scale
            }),
            InputEvent::Key(_)
            | InputEvent::Ime(_)
            | InputEvent::EditCommand(_)
            | InputEvent::Housekeeping
            | InputEvent::Overlay(_) => self.clone(),
        }
    }

    /// Return a copy of this event with its position mapped through `affine` —
    /// the general form of [`InputEvent::translated`], for a container that
    /// places a child under an arbitrary transform
    /// ([`crate::widget::ChildPod::set_transform`]).
    ///
    /// Maps exactly the positions `translated` shifts, and leaves alone exactly
    /// what it leaves alone: [`InputEvent::Pointer`]'s position, the inner event
    /// of an [`InputEvent::PointerContact`], [`InputEvent::Scroll`]'s `position`
    /// and [`InputEvent::Scale`]'s [`ScaleEvent::focal`] are mapped; the
    /// focus-routed events, the [`Housekeeping`](InputEvent::Housekeeping)
    /// broadcast and the window-space [`Overlay`](InputEvent::Overlay) payload are
    /// returned unchanged (cloned), for the reasons `translated` gives.
    ///
    /// Only *positions* are mapped. A scroll `delta`, a scale's multiplicative
    /// `scale_delta` and its `velocity` are carried over as-is: they describe the
    /// gesture's magnitude in the input device's terms, not a point in the
    /// receiver's space.
    ///
    /// A container routing into a transformed child passes the **inverse** of the
    /// child's local→container mapping here; the caller owns checking that the
    /// inverse exists (see [`crate::hit::checked_inverse`]).
    pub fn transformed(&self, affine: &Affine) -> InputEvent {
        match self {
            InputEvent::Pointer(p) => InputEvent::Pointer(PointerEvent {
                position: *affine * p.position,
                ..*p
            }),
            InputEvent::PointerContact { pointer_id, event } => InputEvent::PointerContact {
                pointer_id: *pointer_id,
                event: PointerEvent {
                    position: *affine * event.position,
                    ..*event
                },
            },
            InputEvent::Scroll { position, delta } => InputEvent::Scroll {
                position: *affine * *position,
                delta: *delta,
            },
            InputEvent::Scale(scale) => InputEvent::Scale(ScaleEvent {
                focal: *affine * scale.focal,
                ..*scale
            }),
            InputEvent::Key(_)
            | InputEvent::Ime(_)
            | InputEvent::EditCommand(_)
            | InputEvent::Housekeeping
            | InputEvent::Overlay(_) => self.clone(),
        }
    }

    /// Whether this event is focus-routed (delivered down the focus chain with no
    /// hit test) rather than hit-tested by position.
    ///
    /// [`Housekeeping`](InputEvent::Housekeeping) is **not** focus-routed — it
    /// reaches every child, focused or not; see
    /// [`is_broadcast`](InputEvent::is_broadcast).
    pub fn is_focus_routed(&self) -> bool {
        matches!(
            self,
            InputEvent::Key(_) | InputEvent::Ime(_) | InputEvent::EditCommand(_)
        )
    }

    /// Whether this event is a broadcast: forwarded to **every** child
    /// unconditionally, with no hit test, no capture fast-path, and no focus
    /// routing — [`InputEvent::Housekeeping`] and [`InputEvent::Overlay`].
    ///
    /// Every routing helper branches on this **first**, before its capture,
    /// focus, and hit-test branches (`frust-widgets`'
    /// `route_event`/`route_event_single`, and this crate's own
    /// [`crate::component`] mirror), so a broadcast can never be swallowed by a
    /// captured child or a `contains()` miss. That existing branch is exactly
    /// what carries an overlay event to its owner with no router change: the two
    /// variants differ in what they *mean* (a deferred callback flush vs one
    /// floated surface's own input), not in how they travel.
    pub fn is_broadcast(&self) -> bool {
        matches!(self, InputEvent::Housekeeping | InputEvent::Overlay(_))
    }
}

thread_local! {
    /// The "a deferred state-bearing callback is queued somewhere in this
    /// thread's tree" flag, raised by [`mark_pending_result_flush`] and drained
    /// by [`take_pending_result_flush`].
    ///
    /// A side channel for the same reason [`crate::widget::report_retired_slot`]'s
    /// `RETIRED_SLOTS` list is one: the widget that queues the callback is deep
    /// inside a `View::rebuild` (a `BuildCtx` pass) with no
    /// [`crate::app::RenderRoot`] handle to reach, and — unlike a paint pass — no
    /// threaded per-frame sink.
    ///
    /// **Data-free on purpose.** Only the *fact* that a flush is owed rides here;
    /// the callbacks themselves stay in the widget that queued them. Those
    /// callbacks are `Rc<dyn Fn>` (`!Send`), so they can only ever be run on the
    /// thread that queued them — which is exactly why this is `thread_local`
    /// rather than a process-global `AtomicBool`. A global would let a
    /// [`RenderRoot`](crate::app::RenderRoot) on one thread *drain a mark raised
    /// on another*, broadcasting into a tree with nothing pending while the tree
    /// that actually owes the flush is left waiting — silently reintroducing the
    /// failure this broadcast exists to fix. UI-thread affinity is the same argument
    /// `frust-reactive`'s `CAN_POP_PROVIDER` and `frust-widgets`' `PAGE_REACH`
    /// make for their own `Rc`-backed state.
    static PENDING_RESULT_FLUSH: Cell<bool> = const { Cell::new(false) };
}

/// Record that a widget queued a callback needing `&mut State` during a
/// state-free pass, so [`crate::app::RenderRoot::rebuild`] dispatches an
/// [`InputEvent::Housekeeping`] broadcast before the frame ends.
///
/// Idempotent: marking twice in one pass owes exactly one broadcast, and the
/// broadcast reaches every widget that queued anything (see the variant's
/// routing contract).
///
/// Thread-affine: the mark is visible only to the thread that raised it, which
/// is also the only thread that can run the `!Send` callback it stands for.
pub fn mark_pending_result_flush() {
    PENDING_RESULT_FLUSH.with(|flag| flag.set(true));
}

/// Take (and clear) the [`mark_pending_result_flush`] flag.
///
/// Drained by [`crate::app::RenderRoot::rebuild`], which dispatches one
/// [`InputEvent::Housekeeping`] broadcast per `true` it takes. Destructive,
/// mirroring [`crate::app::RenderRoot::take_change_flags`]: a caller that drains
/// and drops the result loses that flush until something marks again.
pub fn take_pending_result_flush() -> bool {
    PENDING_RESULT_FLUSH.with(|flag| flag.replace(false))
}

/// Non-draining peek at the [`mark_pending_result_flush`] flag — whether a
/// deferred state-bearing callback is owed a [`InputEvent::Housekeeping`]
/// broadcast, without consuming the mark.
///
/// The [`take_change_flags`](crate::app::RenderRoot::take_change_flags) /
/// [`has_pending_change_flags`](crate::app::RenderRoot::has_pending_change_flags)
/// pairing, one layer down: the mobile shells read this while gathering their
/// frame-gate inputs (`FrameInputs::deferred_callbacks_pending`) *before*
/// deciding whether the frame runs at all, so a frame the gate would otherwise
/// skip still runs and reaches the [`crate::app::RenderRoot::rebuild`] that
/// drains the mark. Peeking must not consume it — draining stays that rebuild's
/// job.
///
/// Thread-affine like both of its neighbours: it reports only marks raised on
/// the calling thread (see the `PENDING_RESULT_FLUSH` doc for why the flag is
/// thread-local rather than a process-global `AtomicBool`).
pub fn has_pending_result_flush() -> bool {
    PENDING_RESULT_FLUSH.with(|flag| flag.get())
}

thread_local! {
    /// The "a focused child pod lost its identity during this thread's view
    /// diff" flag, raised by [`mark_focus_orphaned`] and drained by
    /// [`take_focus_orphaned`].
    ///
    /// A side channel for exactly the reason [`PENDING_RESULT_FLUSH`] above is
    /// one: the reconciler that tears a focused pod down runs deep inside a
    /// `View::rebuild` (a [`crate::view::BuildCtx`] pass) with no
    /// [`RenderRoot`](crate::app::RenderRoot) handle to reach, so it cannot
    /// clear the root's `focus_active`/`ime_state` mirror itself — the
    /// long-standing desync `frust-widgets`' `cancel_active_children` documents.
    /// *Which* pods may raise it is narrowed by the pass's own focus chain
    /// ([`crate::view::BuildCtx::has_focus`]) — see [`mark_focus_orphaned`].
    ///
    /// **Data-free on purpose, and idempotent.** Only the *fact* that some
    /// focused pod died rides here; there is nothing useful to carry (the root
    /// keeps no id of the focused widget, only the boolean mirror). Several pods
    /// cleared in one diff owe exactly one release.
    ///
    /// Thread-local rather than a process-global `AtomicBool` for the same
    /// UI-thread-affinity reason: the tree that lost the focus, and the
    /// `RenderRoot` that must release the session, live on one thread. A global
    /// would let a root on one thread release a session another thread's tree
    /// still holds.
    static FOCUS_ORPHANED: Cell<bool> = const { Cell::new(false) };
}

/// Record that a structural rebuild severed the recorded focus path — a focused
/// [`ChildPod`](crate::widget::ChildPod) was torn down, type-swapped, or had its
/// `focused` flag cleared by a reconciler — so
/// [`RenderRoot::rebuild`](crate::app::RenderRoot::rebuild) releases the whole
/// focus/IME session before the frame ends.
///
/// # The invariant: a mark means a LIVE session lost its owner
///
/// Raise this only when the severed link was on the **live focus chain** — the
/// pod's own `focused` flag AND
/// [`BuildCtx::has_focus`](crate::view::BuildCtx::has_focus), the composed chain
/// from the root down to it. A `focused` flag on its own is not evidence of a
/// session: a container-routed blur clears the focus link at the nearest common
/// ancestor only, so flags deeper in the blurred branch legitimately stay set,
/// and marking on one of those releases whatever field is *actually* focused
/// elsewhere in the tree — the keyboard dropping mid-typing because an unrelated
/// list recycled a row. Every drain here performs a real, user-visible release;
/// it must never fire on speculation.
///
/// Raised by `frust-widgets`' reconcilers (`teardown_child`,
/// `cancel_active_children`, and the type-swap arms of both the keyed reconciler
/// and the single-child `rebuild_child`), all four through one shared gate
/// (`mark_orphan_if_live`), and by
/// [`ComponentView::rebuild`](crate::component::ComponentView)'s own swap arm,
/// which spells the identical gate by hand because this crate sits below
/// `frust-widgets`. A hand-rolled container that clears a focused pod itself
/// should raise it under the same condition.
///
/// Idempotent and thread-affine, exactly like [`mark_pending_result_flush`].
pub fn mark_focus_orphaned() {
    FOCUS_ORPHANED.with(|flag| flag.set(true));
}

/// Take (and clear) the [`mark_focus_orphaned`] flag.
///
/// Drained by [`RenderRoot::rebuild`](crate::app::RenderRoot::rebuild), which
/// performs one full focus/IME session release per `true` it takes. Destructive,
/// mirroring [`take_pending_result_flush`]: a caller that drains and drops the
/// result loses that release until something marks again.
///
/// A mark can only be raised *during* a view diff, and the diff's own
/// `RenderRoot::rebuild` drains it before returning, so the flag never survives
/// a frame — there is no peeking counterpart (unlike
/// [`has_pending_result_flush`], which a frame gate must consult before deciding
/// whether to run the rebuild that drains it at all).
pub fn take_focus_orphaned() -> bool {
    FOCUS_ORPHANED.with(|flag| flag.replace(false))
}

thread_local! {
    /// Which root owes a hover end because a pod holding its LIVE hover link was
    /// dropped by this thread's view diff — raised by [`mark_hover_orphaned`] and
    /// drained by [`take_hover_orphaned`]. `None` when nothing is owed.
    ///
    /// Hover's analog of [`FOCUS_ORPHANED`], and a side channel for the same
    /// missing-handle reason: the reconciler that drops the claimant's
    /// [`ChildPod`](crate::widget::ChildPod) runs inside a
    /// [`View::rebuild`](crate::view::View::rebuild) with no
    /// [`RenderRoot`](crate::app::RenderRoot) to clear the root's hover mirror
    /// with. Idempotent for the same reason too: several pods severed in one diff
    /// owe exactly one hover end.
    ///
    /// **Root-qualified rather than data-free**, which is where it diverges from
    /// its focus neighbour. Focus is raised *and* drained inside one root's own
    /// `rebuild`, so a bare bool cannot reach a second root. A hover mark comes
    /// from a destructor, which fires whenever a pod happens to die — including
    /// while another root on the same thread is the one that rebuilds next — so
    /// the mark carries the identity of the root whose link died and only that
    /// root's drain consumes it. Two roots' epoch counters legitimately collide
    /// (each starts at `1` and advances per hover pass), so the identity, not the
    /// epoch, is what keeps them apart.
    ///
    /// One slot, so two roots severed between the same pair of rebuilds leave the
    /// later mark standing and the earlier root's mirror to lapse on its own next
    /// hover pass — the pre-existing degradation, never a release of a link that
    /// is still held.
    static HOVER_ORPHANED: Cell<Option<u64>> = const { Cell::new(None) };

    /// The hover link standing on this thread right now as `(root identity,
    /// epoch)`, or `(0, 0)` when nothing holds one — published by
    /// [`RenderRoot::event`](crate::app::RenderRoot::event) whenever it closes a
    /// hover pass, and read by a dropping pod to tell a live link from a stale
    /// stamp ([`live_hover_link_is`]).
    ///
    /// The root would otherwise be unreachable from a destructor, and the
    /// distinction is the whole invariant: pods carrying *stale* stamps are
    /// dropped constantly (any recycled list row that was hovered at some point),
    /// and ending the hover on one of those would drop the chrome of whatever is
    /// hovered now.
    ///
    /// Thread-local for its neighbours' UI-thread-affinity reason. It mirrors
    /// **one** root, so a second `RenderRoot` driving passes on the same thread
    /// overwrites it — costing the first root's hover the drop-time check (its
    /// link then lapses on the next `Move`, the pre-existing behaviour) rather
    /// than corrupting anything. The root identity in the pair is what makes that
    /// last clause true: a pod of the overwritten root can no longer match the
    /// published link by an epoch integer the two roots happen to share, so it
    /// marks nothing instead of ending the *other* root's live hover.
    static LIVE_HOVER_LINK: Cell<(u64, u64)> = const { Cell::new((0, 0)) };
}

/// Record that a [`ChildPod`](crate::widget::ChildPod) holding the **live** hover
/// link was dropped, so [`RenderRoot::rebuild`](crate::app::RenderRoot::rebuild)
/// ends the hover before the frame ends.
///
/// # Why a destructor, and not the reconcilers
///
/// [`mark_focus_orphaned`]'s callers are the reconcilers themselves, because a
/// focused pod's link is a flag they own and clear (`set_focused`). Hover has no
/// such flag and no setter: the link is an epoch stamp
/// ([`ChildPod::hover_epoch`](crate::widget::ChildPod::hover_epoch)) that only
/// [`ChildPod::event_child`](crate::widget::ChildPod::event_child) may write,
/// deliberately, so that no container can record or clear a hover by hand. A
/// container therefore *cannot* report its own severance, and hand-rolled
/// containers outside this workspace could never opt in. The pod reports instead,
/// from `Drop`, which covers every removal route — a truncated `Vec`, a
/// `None`-ed `Option`, a keyed reconciler's dropped entry, a whole subtree torn
/// down — with nothing to remember to call.
///
/// # The invariant: a mark means the LIVE link lost its owner
///
/// Exactly [`mark_focus_orphaned`]'s invariant, enforced by the stamp comparison
/// instead of a chain: a pod marks only when its own stamp is non-zero *and*
/// names the published link — same root identity, same epoch
/// ([`live_hover_link_is`]). A stale stamp — the far commoner case, since a
/// stamp is never cleared, only stranded by the next epoch advance — marks
/// nothing, and so does a stamp from a *different* root that happens to carry the
/// same epoch integer.
///
/// `root` is the identity the claim was stamped with (the root that ran the hover
/// pass), so only that root's [`take_hover_orphaned`] consumes the mark.
///
/// Idempotent and thread-affine, exactly like [`mark_focus_orphaned`].
pub(crate) fn mark_hover_orphaned(root: u64) {
    HOVER_ORPHANED.with(|slot| slot.set(Some(root)));
}

/// Take (and clear) a [`mark_hover_orphaned`] mark raised for `root`.
///
/// Drained by [`RenderRoot::rebuild`](crate::app::RenderRoot::rebuild), which
/// ends its own standing hover link per `true` it takes. Destructive for the
/// matching root only, mirroring [`take_focus_orphaned`]: a mark another root
/// raised is left standing rather than consumed, which is what keeps two roots on
/// one thread from ending each other's hover.
pub(crate) fn take_hover_orphaned(root: u64) -> bool {
    HOVER_ORPHANED.with(|slot| {
        if slot.get() == Some(root) {
            slot.set(None);
            true
        } else {
            false
        }
    })
}

/// Publish the hover link standing on this thread — `root`'s live epoch while one
/// of its widgets holds the link, epoch `0` while none does.
///
/// Called by [`RenderRoot::event`](crate::app::RenderRoot::event) as it closes a
/// hover pass, and by the rebuild-time end that [`take_hover_orphaned`] drives.
pub(crate) fn set_live_hover_link(root: u64, epoch: u64) {
    LIVE_HOVER_LINK.with(|slot| slot.set((root, epoch)));
}

/// Whether `(root, epoch)` is the hover link standing on this thread — what a
/// dropping [`ChildPod`](crate::widget::ChildPod) compares its own stamp against
/// (see [`mark_hover_orphaned`]). Epoch `0` is "no link" and never matches.
pub(crate) fn live_hover_link_is(root: u64, epoch: u64) -> bool {
    epoch != 0 && LIVE_HOVER_LINK.with(|slot| slot.get()) == (root, epoch)
}

thread_local! {
    /// The cursor a widget asked for during the request pass currently running on
    /// this thread — written by [`EventCtx::set_cursor`], bracketed by the
    /// [`RequestPass`] guard [`crate::app::RenderRoot::event`] holds for the
    /// length of its dispatch.
    ///
    /// A side channel for a *routing* reason rather than the missing-handle
    /// reason [`PENDING_RESULT_FLUSH`] and [`FOCUS_ORPHANED`] above have. Unlike
    /// capture, focus, and hover, a cursor request has nothing to record **per
    /// pod**: the root wants one value — whichever widget on the routed path
    /// spoke last — and no container between that widget and the root reads it or
    /// acts on it. Bubbling it pod by pod would mean widening every container's
    /// fold to carry a value no container uses.
    ///
    /// **Pass-scoped, not persistent.** The guard clears the slot before
    /// dispatching and drains it after, so a request never outlives its pass, and
    /// a [`EventCtx::set_cursor`] made from a dispatch no root drives (the
    /// `Cancel` a reconciler synthesizes during a rebuild, say) is dropped by the
    /// next pass's clear rather than leaking into it. Last write wins, which is
    /// what makes the innermost widget the routed path reaches the one that
    /// decides. A *nested* pass is scoped the same way and hands the slot back
    /// (see [`RequestPass`]).
    ///
    /// Thread-local rather than a process-global for the same UI-thread-affinity
    /// reason as its two neighbours: the tree that requests a cursor and the
    /// `RenderRoot` whose shell applies it live on one thread, and a global would
    /// let a hover on one thread reshape another window's pointer.
    static CURSOR_REQUEST: Cell<Option<CursorIcon>> = const { Cell::new(None) };

    /// The text a widget asked the shell to put on the host clipboard during the
    /// request pass currently running on this thread — written by
    /// [`EventCtx::write_clipboard`], bracketed by the same [`RequestPass`] guard,
    /// and resolved by [`crate::app::RenderRoot::event`] into the value a shell
    /// drains through
    /// [`RenderRoot::take_clipboard_write`](crate::app::RenderRoot::take_clipboard_write).
    ///
    /// **A slot rather than a bubbled field, for [`CURSOR_REQUEST`]'s routing
    /// reason verbatim** (above): the root wants one value — whichever widget on
    /// the routed path spoke last — and no container between the copying widget
    /// and the root reads it or acts on it, so recording it per pod would widen
    /// every container's fold ([`EventCtx::absorb_child`], and with it
    /// [`crate::widget::ChildPod::event_child`] and every hand-written router in
    /// `frust-widgets`) to carry a payload no container uses. `ImeState` is
    /// bubbled precisely because containers *do* re-publish it; a clipboard write
    /// is a one-way message to the shell.
    ///
    /// **Pass-scoped and last-writer-wins**, exactly like the cursor: a write
    /// made outside any pass (a reconciler's synthesized `Cancel`) is dropped
    /// rather than leaked into the next pass, and a `Cut` that writes from an
    /// inner widget after its container wrote something else sends the inner
    /// widget's text.
    ///
    /// Thread-local for its neighbours' UI-thread-affinity reason: the tree that
    /// copies and the `RenderRoot` whose shell owns the host clipboard live on
    /// one thread.
    static CLIPBOARD_WRITE: Cell<Option<String>> = const { Cell::new(None) };

    /// Whether a widget asked the shell to hand it the host clipboard's contents
    /// during the request pass currently running on this thread — raised by
    /// [`EventCtx::request_paste`], bracketed by the same [`RequestPass`] guard,
    /// and resolved by [`crate::app::RenderRoot::event`] into the flag a shell
    /// drains through
    /// [`RenderRoot::take_paste_request`](crate::app::RenderRoot::take_paste_request).
    ///
    /// **Data-free and idempotent**, like [`PENDING_RESULT_FLUSH`]: only the
    /// *fact* that a paste was asked for rides here, because the answer is the
    /// shell's to compose (it reads the host clipboard and dispatches
    /// [`InputEvent::EditCommand`]`(`[`EditCommand::Paste`]`)`). Two widgets
    /// asking in one pass owe exactly one read — there is one host clipboard and
    /// one focused widget to deliver it to, so "who asked" adds nothing.
    ///
    /// Pass-scoped and thread-local for [`CLIPBOARD_WRITE`]'s reasons.
    static PASTE_REQUEST: Cell<bool> = const { Cell::new(false) };

    /// Edit commands one widget dispatched to another during the request pass
    /// currently running on this thread — pushed by
    /// [`EventCtx::dispatch_edit_command`] and drained, in order, by
    /// [`EventCtx::take_edit_commands`].
    ///
    /// **A widget-to-widget channel, not a shell-facing one**, which is what
    /// makes it different from its three neighbours above: the cursor, the
    /// clipboard write and the paste request all resolve *at the root* into
    /// something a shell reads, whereas this queue is drained by another widget
    /// in the same pass and the root never looks at it. It rides here anyway
    /// because the two widgets cannot reach each other any other way — a
    /// selection toolbar floated through [`crate::overlay`] is a pod the *text
    /// input* owns but does not contain, so the toolbar's "Copy" tap has no
    /// container path down which to hand the verb back.
    ///
    /// **A FIFO, not a last-writer-wins slot**: a toolbar may answer one tap with
    /// several verbs (a "cut" that is a copy then a delete), and order is
    /// meaning.
    ///
    /// Pass-scoped like its neighbours, and for a sharper reason: an undrained
    /// command must never re-fire in a later pass — a stale `Cut` applied to
    /// whatever is selected two gestures later would silently destroy text. A
    /// pass that ends with the queue non-empty therefore clears it (and says so
    /// in a debug build — see [`RequestPass::take`]) rather than carrying it.
    ///
    /// Thread-local for its neighbours' UI-thread-affinity reason.
    static EDIT_COMMAND_QUEUE: Cell<Vec<EditCommand>> = const { Cell::new(Vec::new()) };

    /// Whether a request pass is open on this thread — `false` at rest, `true` for
    /// the length of one, however many are nested. Owned by [`RequestPass`], which
    /// is the only thing that reads or writes it: [`RequestPass::enter`] captures
    /// the previous value into the guard and `Drop` puts exactly that value back,
    /// so an unwind through a nested pass restores the enclosing pass's state
    /// rather than leaving a counter to unwind correctly on its own.
    ///
    /// The one question it answers is whether [`RequestPass::enter`] found an
    /// *enclosing* pass's in-progress requests in the slots (restore them on exit)
    /// or stray requests made outside any pass (drop them), which the slots' own
    /// contents cannot distinguish. One flag covers all three slots because one
    /// guard brackets all three: they open and close together, per dispatch.
    static REQUEST_PASS_OPEN: Cell<bool> = const { Cell::new(false) };
}

/// One pass-scoped request slot's save/restore half — the mechanism [`RequestPass`]
/// owns three of.
///
/// Generic over the slot's payload rather than written out per channel: the
/// cursor, the clipboard write, and the paste flag differ only in what they
/// carry, and three hand-copied guards would be three places for the
/// stash-and-restore invariant to drift. `T::default()` is each slot's "nobody
/// asked" state (`None`, `None`, `false`), which is exactly what makes absence
/// the answer rather than a missing answer.
pub(crate) struct PassSlot<T: Default + 'static> {
    /// The thread-local this half brackets. A `&'static` handle so one generic
    /// body serves every channel — [`LocalKey::with`] needs the `'static`
    /// reference anyway.
    slot: &'static LocalKey<Cell<T>>,
    /// What the slot is restored to when this pass ends: the enclosing pass's
    /// in-progress request when nested, `T::default()` at the outermost level.
    restore: T,
}

impl<T: Default + 'static> PassSlot<T> {
    /// Open this slot for a pass, starting it from "nobody has asked for
    /// anything" and stashing whatever an enclosing pass had collected.
    ///
    /// `nested` is the shared [`REQUEST_PASS_OPEN`] answer: only an enclosing
    /// pass is owed its value back, since a value found in the slot with no pass
    /// open is a stray (see [`CURSOR_REQUEST`]).
    pub(crate) fn enter(slot: &'static LocalKey<Cell<T>>, nested: bool) -> Self {
        let stashed = slot.with(|cell| cell.take());
        Self {
            slot,
            restore: if nested { stashed } else { T::default() },
        }
    }

    /// Take what *this* pass recorded in this slot, leaving it empty.
    pub(crate) fn take(&self) -> T {
        self.slot.with(|cell| cell.take())
    }
}

impl<T: Default + 'static> Drop for PassSlot<T> {
    fn drop(&mut self) {
        let restore = std::mem::take(&mut self.restore);
        self.slot.with(|cell| cell.set(restore));
    }
}

/// A pass-scoped slot **carrying its own open flag** — [`PassSlot`] made
/// self-contained, for a channel bracketed by a *different* pass than the event
/// dispatch.
///
/// [`RequestPass`] brackets three channels that open and close together, so one
/// [`REQUEST_PASS_OPEN`] flag serves all three. A channel scoped to the **paint**
/// pass instead (the overlay registry and the selection-toolbar publish slot —
/// see [`crate::overlay`] and [`crate::selection_toolbar`]) cannot share that
/// flag: a paint pass runs with no event pass open, and an event pass with no
/// paint pass open, so borrowing the other's flag would answer "is an enclosing
/// pass of MY kind open?" with another kind's state and either restore a stray
/// or drop an enclosing pass's work. One flag per bracket is what keeps the
/// question well-posed.
///
/// Everything else is [`PassSlot`]'s, verbatim: enter stashes, [`take`](Self::take)
/// drains what this pass alone recorded, and `Drop` puts the enclosing pass's
/// stash back (or leaves the slot clear at the outermost level, so a value
/// written with no pass open is dropped rather than leaked into the next one).
///
/// Unlike [`RequestPass::take`] this drains behind `&self` rather than consuming
/// the guard: a paint pass resolves its channels *and then* keeps painting
/// (`RenderRoot::paint` drains the registry, then paints what it drained), so the
/// bracket has to outlive its own drain.
pub(crate) struct PassBracket<T: Default + 'static> {
    /// The value half, which owns the stash/restore invariant.
    slot: PassSlot<T>,
    /// This bracket's own "a pass is open" flag.
    open: &'static LocalKey<Cell<bool>>,
    /// Whether a pass of this kind was already open when this one entered — put
    /// back verbatim by `Drop`.
    was_open: bool,
}

impl<T: Default + 'static> PassBracket<T> {
    /// Open a pass over `slot`, tracked by `open`, starting from
    /// `T::default()` ("nobody has recorded anything").
    pub(crate) fn enter(
        slot: &'static LocalKey<Cell<T>>,
        open: &'static LocalKey<Cell<bool>>,
    ) -> Self {
        let was_open = open.with(|flag| flag.replace(true));
        Self {
            slot: PassSlot::enter(slot, was_open),
            open,
            was_open,
        }
    }

    /// Take what *this* pass recorded, leaving the slot empty.
    pub(crate) fn take(&self) -> T {
        self.slot.take()
    }
}

impl<T: Default + 'static> Drop for PassBracket<T> {
    fn drop(&mut self) {
        // The inner `PassSlot` restores the value as it drops, right after this.
        self.open.with(|flag| flag.set(self.was_open));
    }
}

/// Everything one request pass resolved: the three shell-facing values a
/// dispatch can produce, drained together by [`RequestPass::take`].
pub(crate) struct PassRequests {
    /// The cursor the pass's last [`EventCtx::set_cursor`] asked for; `None` when
    /// no widget asked, which resolves to [`CursorIcon::Default`].
    pub(crate) cursor: Option<CursorIcon>,
    /// The text the pass's last [`EventCtx::write_clipboard`] asked the shell to
    /// put on the host clipboard; `None` when no widget copied.
    pub(crate) clipboard_write: Option<String>,
    /// Whether any widget in the pass called [`EventCtx::request_paste`].
    pub(crate) paste_request: bool,
}

/// The open/close bracket around one request pass, and the guard that makes the
/// pass-scoped slots above survive re-entrancy.
///
/// # Why a guard rather than a bare clear/take pair
///
/// Each slot is *pass-scoped*: cleared before a dispatch, drained after it, so a
/// request never outlives the pass that made it. Spelled as a bare
/// `clear_cursor_request` + `take_cursor_request` pair that contract holds
/// only while passes never nest — a nested dispatch's clear would erase a request
/// the enclosing pass had already collected, and its drain would take one the
/// enclosing pass was still owed. Nothing in this workspace nests a pass today
/// ([`RenderRoot::event`](crate::app::RenderRoot::event) documents the rule, and
/// the devtools injector hops its synthetic events onto the UI thread's queue
/// rather than calling into a live dispatch), but the failure is silent and the
/// cost of ruling it out is one stack slot.
///
/// # One guard, three channels
///
/// The cursor, the clipboard write and the paste request are all "one value the
/// root resolves at the end of the dispatch", so they share a bracket and a
/// single [`REQUEST_PASS_OPEN`] flag rather than three copies of this reasoning;
/// the per-slot half is [`PassSlot`], instantiated once per channel. What differs
/// is only what the root *does* with each value — see
/// [`RenderRoot::event`](crate::app::RenderRoot::event), where the cursor commits
/// on pointer-move passes alone while the two clipboard values commit on every
/// pass.
///
/// # What nesting resolves to
///
/// Save-and-restore, so **every** pass — nested or not — resolves exactly the
/// requests made inside it, and an inner pass returns the slots to the enclosing
/// pass untouched:
///
/// * [`RequestPass::enter`] stashes whatever the enclosing pass had collected and
///   starts the inner pass from empty (the same "absence *is* the answer" state a
///   top-level pass starts from).
/// * [`RequestPass::take`] drains what this pass alone recorded, and **consumes
///   the guard**: a pass resolves exactly once, and the drain is what closes it.
/// * `Drop` puts the enclosing pass's stash back — or, at the outermost level,
///   leaves the slots clear, exactly as the bare pair did, so a request made
///   outside any pass (a reconciler's synthesized `Cancel`) is still dropped
///   rather than leaked into the next one.
pub(crate) struct RequestPass {
    /// The cursor half of the bracket.
    cursor: PassSlot<Option<CursorIcon>>,
    /// The clipboard-write half.
    clipboard_write: PassSlot<Option<String>>,
    /// The paste-request half.
    paste_request: PassSlot<bool>,
    /// The widget-to-widget edit-command queue. Bracketed here rather than
    /// resolved into [`PassRequests`]: the root must never see it (its consumer
    /// is another widget in the same pass), so this half exists only to bound the
    /// queue's lifetime to the pass — see [`EDIT_COMMAND_QUEUE`].
    edit_commands: PassSlot<Vec<EditCommand>>,
    /// Whether a pass was already open when this one entered — put back verbatim
    /// by `Drop`, so an inner pass leaves the enclosing one open and the
    /// outermost leaves the thread at rest.
    was_open: bool,
}

impl RequestPass {
    /// Open a request pass, starting every slot from "nobody has asked for
    /// anything".
    pub(crate) fn enter() -> Self {
        let was_open = REQUEST_PASS_OPEN.with(|open| open.replace(true));
        Self {
            cursor: PassSlot::enter(&CURSOR_REQUEST, was_open),
            clipboard_write: PassSlot::enter(&CLIPBOARD_WRITE, was_open),
            paste_request: PassSlot::enter(&PASTE_REQUEST, was_open),
            edit_commands: PassSlot::enter(&EDIT_COMMAND_QUEUE, was_open),
            was_open,
        }
    }

    /// Take what *this* pass recorded — the values the root resolves into its
    /// shell-facing cursor, clipboard write and paste request.
    ///
    /// Consumes the guard, so the pass ends here: a second drain of the same pass
    /// is unrepresentable rather than a silent set of empties (the slots are
    /// drained destructively, so a repeat call would report "nobody asked" for a
    /// pass that had already resolved).
    pub(crate) fn take(self) -> PassRequests {
        // The edit-command queue is drained here but NOT reported: anything left
        // in it is a command whose intended consumer never called
        // `EventCtx::take_edit_commands` — a wiring bug in the dispatching
        // widget, not something the root can act on. Dropping it is the safe
        // resolution (a command that survived into a later pass would apply to
        // whatever is selected *then*), and a debug build says so rather than
        // swallowing it silently.
        let leaked = self.edit_commands.take();
        #[cfg(debug_assertions)]
        if !leaked.is_empty() {
            eprintln!(
                "frust-core: {} edit command(s) dispatched but never taken in this \
                 pass ({leaked:?}); clearing — the dispatching widget's consumer \
                 must call EventCtx::take_edit_commands in the same pass",
                leaked.len()
            );
        }
        drop(leaked);
        PassRequests {
            cursor: self.cursor.take(),
            clipboard_write: self.clipboard_write.take(),
            paste_request: self.paste_request.take(),
        }
    }
}

impl Drop for RequestPass {
    fn drop(&mut self) {
        // Each `PassSlot` restores its own slot as it drops, right after this.
        REQUEST_PASS_OPEN.with(|open| open.set(self.was_open));
    }
}

/// Clear any pending cursor request, so the pass about to run starts from
/// "nobody has asked for anything".
///
/// Absence is not a missing answer — it *is* the answer
/// ([`CursorIcon::Default`]), which is why the clear is what makes the request
/// model stateless: a widget that stops asking stops being obeyed, with nothing
/// to release.
///
/// **Test-only.** Production code brackets a pass with [`RequestPass`], which
/// owns both ends; this is the bare clear a test that drives a widget *with no
/// root above it* needs to start from a known slot (`component.rs`'s
/// component-boundary cursor test is the one caller).
#[cfg(test)]
pub(crate) fn clear_cursor_request() {
    CURSOR_REQUEST.with(|slot| slot.set(None));
}

/// Take (and clear) the cursor requested during this pass, `None` when no widget
/// asked — the drain half of the test-only pair (see
/// [`clear_cursor_request`]); a root reaches the same value through
/// [`RequestPass::take`].
#[cfg(test)]
pub(crate) fn take_cursor_request() -> Option<CursorIcon> {
    CURSOR_REQUEST.with(|slot| slot.take())
}

/// What the contact pass running on this thread knows about the pointer it is
/// dispatching — the state [`ContactPass`] brackets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ContactPassState {
    /// Whether a root opened a contact pass at all. A
    /// [`EventCtx::capture_contacts`] made outside one (a widget driven with no
    /// root above it) records nothing here.
    open: bool,
    /// The contact being dispatched — what [`EventCtx::new`] seeds a fresh
    /// context's [`EventCtx::pointer_id`] with.
    pointer_id: PointerId,
    /// Whether the dispatch is a **non-claimant** contact travelling down a live
    /// capture's path (rule (c) of [`InputEvent::PointerContact`]'s contract).
    /// While set, [`crate::widget::ChildPod::set_active`] refuses to drop a
    /// recorded active link, so another contact's `Up`/`Cancel` cannot release
    /// the claimant's capture inside any container.
    secondary: bool,
    /// Whether a widget called [`EventCtx::capture_contacts`] during this pass
    /// — since the last [`EventCtx::release_captured_child`] that released the
    /// opted-in widget, which resets it (so the flag ends the pass naming only
    /// an opt-in that is still on the active path).
    contacts_requested: bool,
    /// Whether a container released the gesture's opted-in widget from the
    /// active path during this pass ([`EventCtx::release_captured_child`]).
    capture_released: bool,
}

impl ContactPassState {
    /// Nothing open: the mouse, no secondary contact, nothing requested.
    const IDLE: ContactPassState = ContactPassState {
        open: false,
        pointer_id: PointerId::MOUSE,
        secondary: false,
        contacts_requested: false,
        capture_released: false,
    };
}

thread_local! {
    /// The contact pass currently running on this thread — opened by
    /// [`crate::app::RenderRoot::event`] for each dispatch through a
    /// [`ContactPass`] guard.
    ///
    /// **A slot as well as a per-context field**, for the component boundary's
    /// sake: a [`crate::component`] element dispatches its subtree through a
    /// *fresh* [`EventCtx`] over its own local state, so anything carried only
    /// in the context would stop at it. Seeding [`EventCtx::new`] from here
    /// keeps [`EventCtx::pointer_id`] right below a component, and recording
    /// [`EventCtx::capture_contacts`] here lets the root see an opt-in made
    /// anywhere in the tree — the same reason the cursor request rides a slot
    /// (see [`CURSOR_REQUEST`]).
    ///
    /// Thread-local for its neighbours' UI-thread-affinity reason.
    static CONTACT_PASS: Cell<ContactPassState> = const { Cell::new(ContactPassState::IDLE) };
}

/// The bracket around one root dispatch's contact identity: entering publishes
/// which contact is being dispatched (and whether it is a non-claimant one),
/// dropping restores whatever the enclosing pass had — so a dispatch that
/// re-enters [`crate::app::RenderRoot::event`] (the overlay pre-pass does)
/// scopes its own opt-in and hands the slot back on exit, unwind included.
pub(crate) struct ContactPass {
    saved: ContactPassState,
}

impl ContactPass {
    /// Open a pass dispatching `pointer_id`; `secondary` marks a non-claimant
    /// contact routed down a live capture's path.
    pub(crate) fn enter(pointer_id: PointerId, secondary: bool) -> Self {
        let saved = CONTACT_PASS.with(|slot| {
            slot.replace(ContactPassState {
                open: true,
                pointer_id,
                secondary,
                contacts_requested: false,
                capture_released: false,
            })
        });
        Self { saved }
    }

    /// Whether a widget called [`EventCtx::capture_contacts`] during this pass
    /// (and no later [`EventCtx::release_captured_child`] in it released that
    /// widget from the active path).
    pub(crate) fn contacts_requested(&self) -> bool {
        CONTACT_PASS.with(|slot| slot.get().contacts_requested)
    }

    /// Whether a container released the gesture's opted-in widget from the
    /// active path during this pass ([`EventCtx::release_captured_child`]).
    pub(crate) fn capture_released(&self) -> bool {
        CONTACT_PASS.with(|slot| slot.get().capture_released)
    }
}

impl Drop for ContactPass {
    fn drop(&mut self) {
        CONTACT_PASS.with(|slot| slot.set(self.saved));
    }
}

/// The contact the pass running on this thread is dispatching —
/// [`PointerId::MOUSE`] outside any pass.
pub(crate) fn current_pointer_id() -> PointerId {
    CONTACT_PASS.with(|slot| slot.get().pointer_id)
}

/// Whether the pass running on this thread is delivering a non-claimant
/// contact down a live capture's path (see [`ContactPassState::secondary`]).
pub(crate) fn in_secondary_contact_pass() -> bool {
    CONTACT_PASS.with(|slot| slot.get().secondary)
}

/// Record a [`EventCtx::capture_contacts`] call in the open pass, if any, and
/// in the dispatch frame that made it.
fn note_contacts_requested() {
    CONTACT_PASS.with(|slot| {
        let mut state = slot.get();
        if state.open {
            state.contacts_requested = true;
            slot.set(state);
        }
    });
    CONTACT_FRAME.with(|slot| {
        let mut frame = slot.get();
        frame.opted_in = true;
        slot.set(frame);
    });
}

/// Record an [`EventCtx::release_captured_child`] that took the opted-in
/// widget off the active path: the pass's opt-in is void from here on, unless a
/// later [`EventCtx::capture_contacts`] in the same pass (the container that
/// took over opting in itself) records a fresh one.
fn note_capture_released() {
    CONTACT_PASS.with(|slot| {
        let mut state = slot.get();
        if state.open {
            state.capture_released = true;
            state.contacts_requested = false;
            slot.set(state);
        }
    });
}

/// What one widget dispatch — a [`crate::widget::ChildPod::event_child`] call,
/// or the root's own call into its root widget — learned about the gesture's
/// contact opt-in. The state [`ContactFrame`] brackets.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ContactFrameState {
    /// The dispatched widget itself called [`EventCtx::capture_contacts`].
    opted_in: bool,
    /// A widget below it did (folded in when each nested frame closes).
    opted_in_below: bool,
    /// The dispatch runs inside the handler of the widget that holds the live
    /// opt-in (that widget's own frame included): a release made here never
    /// takes the opt-in off the active path, because its holder stays on it.
    under_captor: bool,
}

thread_local! {
    /// The innermost widget dispatch's [`ContactFrameState`]. A slot rather
    /// than an [`EventCtx`] field for [`CONTACT_PASS`]'s reason: a component
    /// boundary dispatches through a fresh context, and the opt-in must still be
    /// attributed to the pod it was made under.
    static CONTACT_FRAME: Cell<ContactFrameState> =
        const { Cell::new(ContactFrameState { opted_in: false, opted_in_below: false, under_captor: false }) };
}

/// The bracket around one widget dispatch's contact opt-in bookkeeping: entering
/// opens a clean frame, [`ContactFrame::close`] reports whether the dispatched
/// widget opted in itself and whether anything at or below it did, and dropping
/// restores the enclosing frame with this one's opt-in folded into its
/// `opted_in_below` (unwind included).
pub(crate) struct ContactFrame {
    saved: ContactFrameState,
}

impl ContactFrame {
    /// Open a frame for one widget dispatch; `captor` marks the dispatched
    /// widget as the holder of the live opt-in.
    pub(crate) fn enter(captor: bool) -> Self {
        let saved = CONTACT_FRAME.with(|slot| {
            let saved = slot.get();
            slot.set(ContactFrameState {
                opted_in: false,
                opted_in_below: false,
                under_captor: saved.under_captor || captor,
            });
            saved
        });
        Self { saved }
    }

    /// Close the frame: `(opted_in, opted_in_at_or_below)` for the dispatched
    /// widget.
    pub(crate) fn close(self) -> (bool, bool) {
        let frame = CONTACT_FRAME.with(|slot| slot.get());
        (frame.opted_in, frame.opted_in || frame.opted_in_below)
    }
}

impl Drop for ContactFrame {
    fn drop(&mut self) {
        CONTACT_FRAME.with(|slot| {
            let frame = slot.get();
            let mut restored = self.saved;
            restored.opted_in_below |= frame.opted_in || frame.opted_in_below;
            slot.set(restored);
        });
    }
}

/// Whether the dispatch running on this thread is inside the handler of the
/// widget that holds the live contact opt-in (see
/// [`ContactFrameState::under_captor`]).
fn under_contact_captor() -> bool {
    CONTACT_FRAME.with(|slot| slot.get().under_captor)
}

/// One level of a **secondary-contact walk**: the real event, in the coordinate
/// space of the container currently being handed the walk's carrier, and what
/// the walk delivered below that container.
struct SecondaryWalkFrame {
    /// The non-claimant contact's event, in the space of the container whose
    /// handler is running (i.e. what that container's own `ChildPod`s
    /// receive in their parent's space).
    event: InputEvent,
    /// The result of the delivery the walk made below this container, `None`
    /// until one happened.
    delivered: Option<EventResult>,
}

thread_local! {
    /// The secondary-contact walk running on this thread, if any — innermost
    /// level only; each level saves and restores its enclosing one.
    ///
    /// A slot for the same component-boundary reason as [`CONTACT_PASS`], and
    /// because the walk's real event has to cross a container's handler that is
    /// only ever handed the inert [`secondary_walk_carrier`].
    static SECONDARY_WALK: std::cell::RefCell<Option<SecondaryWalkFrame>> =
        const { std::cell::RefCell::new(None) };

    /// The [`OverlayKey`] the walk's carrier is addressed to — allocated once
    /// per thread from [`OverlayKey::next`], so it is distinct from every key an
    /// overlay owner holds and the carrier is ignored by all of them.
    static SECONDARY_WALK_KEY: OverlayKey = OverlayKey::next();
}

/// The event a container on the active chain is handed while a non-claimant
/// contact walks past it: an [`InputEvent::Overlay`] broadcast addressed to a
/// key no owner holds. Every container already forwards a broadcast to its
/// children before running any gesture, capture, focus or hit-test logic of its
/// own (the broadcast-first rule), and every widget ignores an overlay event
/// addressed to someone else — so a container's handler runs, but none of its
/// pointer machinery does.
pub(crate) fn secondary_walk_carrier() -> InputEvent {
    InputEvent::Overlay(OverlayEvent {
        key: SECONDARY_WALK_KEY.with(|key| *key),
        kind: OverlayEventKind::OutsideDown,
    })
}

/// Whether `event` is [`secondary_walk_carrier`]'s carrier.
fn is_secondary_walk_carrier(event: &InputEvent) -> bool {
    matches!(event, InputEvent::Overlay(overlay)
        if overlay.key == SECONDARY_WALK_KEY.with(|key| *key))
}

/// The real event a secondary-contact walk is carrying past the container
/// that just routed `event` — `Some` only when a walk is running **and** `event`
/// is its carrier (a container that synthesizes an event of its own mid-walk
/// dispatches it normally).
pub(crate) fn secondary_walk_event(event: &InputEvent) -> Option<InputEvent> {
    if !is_secondary_walk_carrier(event) {
        return None;
    }
    SECONDARY_WALK.with(|slot| slot.borrow().as_ref().map(|frame| frame.event.clone()))
}

/// Restores the enclosing walk level on drop (unwind included).
struct SecondaryWalkRestore(Option<SecondaryWalkFrame>);

impl Drop for SecondaryWalkRestore {
    fn drop(&mut self) {
        let saved = self.0.take();
        SECONDARY_WALK.with(|slot| *slot.borrow_mut() = saved);
    }
}

/// Run `f` as one level of a secondary-contact walk carrying `event` (in the
/// space of the container `f` hands the carrier to), returning `f`'s result and
/// what the walk delivered below it — `None` when the carrier never reached a
/// `ChildPod` on the active chain.
pub(crate) fn run_secondary_walk<R>(
    event: InputEvent,
    f: impl FnOnce() -> R,
) -> (R, Option<EventResult>) {
    let saved = SECONDARY_WALK.with(|slot| {
        slot.borrow_mut().replace(SecondaryWalkFrame {
            event,
            delivered: None,
        })
    });
    let restore = SecondaryWalkRestore(saved);
    let result = f();
    let delivered =
        SECONDARY_WALK.with(|slot| slot.borrow().as_ref().and_then(|frame| frame.delivered));
    drop(restore);
    (result, delivered)
}

/// Run `f` with no secondary-contact walk in force — how the walk hands the
/// real event to the widget it ends at, whose own subtree then routes it the
/// ordinary way.
pub(crate) fn without_secondary_walk<R>(f: impl FnOnce() -> R) -> R {
    let saved = SECONDARY_WALK.with(|slot| slot.borrow_mut().take());
    let _restore = SecondaryWalkRestore(saved);
    f()
}

/// Record that the walk level in force delivered the real event below its
/// container, with `result`.
pub(crate) fn note_secondary_delivered(result: EventResult) {
    SECONDARY_WALK.with(|slot| {
        if let Some(frame) = slot.borrow_mut().as_mut() {
            frame.delivered = Some(result);
        }
    });
}

/// What kind of content a focused editable field holds — the hint a widget
/// publishes so each shell can configure the platform input method.
///
/// This is the framework's **input-purpose vocabulary**: renderer- and
/// platform-neutral names a widget states its intent in, which each shell maps
/// onto its own host API. It is deliberately tiny — it exists to let a secret
/// field tell the platform it is secret, not to model every keyboard layout.
///
/// # Why this exists (security, not ergonomics)
///
/// Visual masking (`TextInput::obscured`) hides the glyphs the *app* draws; it
/// says nothing to the input method. A stock soft keyboard given no hint will
/// happily render the field's text in its suggestion strip **above** the masked
/// field, and may commit it to its persistent learned-word dictionary. Only a
/// content-type hint suppresses that; an accessibility `Role::PasswordInput`
/// does not.
///
/// # Platform mapping
///
/// Each shell owns its own constants (core holds no platform integers). The
/// intended mapping, which downstream shell work must honour:
///
/// | Variant | Android (`InputType` / `EditorInfo.imeOptions`) | iOS (`UITextInputTraits`) | Desktop (winit) |
/// |---|---|---|---|
/// | [`Normal`](Self::Normal) | `TYPE_CLASS_TEXT` | platform defaults | `ImePurpose::Normal` |
/// | [`Password`](Self::Password) | `TYPE_CLASS_TEXT \| TYPE_TEXT_VARIATION_PASSWORD`, plus `TYPE_TEXT_FLAG_NO_SUGGESTIONS` and `IME_FLAG_NO_PERSONALIZED_LEARNING` | `isSecureTextEntry = true`, `textContentType = .password`, `autocorrectionType = .no`, `spellCheckingType = .no`, plus smart-punctuation suppression (see [`Terminal`](Self::Terminal)) | `ImePurpose::Password` |
/// | [`NoSuggestions`](Self::NoSuggestions) | `TYPE_CLASS_TEXT \| TYPE_TEXT_FLAG_NO_SUGGESTIONS`, plus `IME_FLAG_NO_PERSONALIZED_LEARNING` | `autocorrectionType = .no`, `spellCheckingType = .no`, plus smart-punctuation suppression | no equivalent — `ImePurpose::Normal` |
/// | [`Terminal`](Self::Terminal) | `TYPE_CLASS_TEXT \| TYPE_TEXT_FLAG_NO_SUGGESTIONS`, plus `IME_FLAG_NO_PERSONALIZED_LEARNING` (same as `NoSuggestions`) | `isSecureTextEntry = false`, `autocorrectionType = .no`, `spellCheckingType = .no`, `smartQuotesType = .no`, `smartDashesType = .no`, `smartInsertDeleteType = .no`, `autocapitalizationType = .none`, `textContentType = nil` | `ImePurpose::Terminal` |
///
/// Sources: Android `android.text.InputType` / `android.view.inputmethod.EditorInfo`
/// and Apple `UITextInputTraits` reference docs, retrieved 2026-08-01.
///
/// **Unsupported is a first-class outcome.** winit 0.30's
/// `Window::set_ime_purpose` is documented as unsupported on iOS/Android/Web/
/// Windows/X11/macOS/Orbital (Wayland text-input-v3 is the only implementation),
/// so the desktop shell may legitimately honour nothing here. A shell that
/// cannot express a hint drops it — it must never refuse to publish, and core
/// never asserts that a hint took effect.
///
/// # Matching rule for shells
///
/// This enum is `#[non_exhaustive]`: adding a variant later (numeric password,
/// email, one-time code…) must not break a shell. So a shell branches its
/// **security** behaviour on [`is_secret`](Self::is_secret) /
/// [`suppresses_suggestions`](Self::suppresses_suggestions), never on a variant
/// match with a `_ =>` fallback — a catch-all arm would silently downgrade a
/// future secret variant to a non-secret keyboard, which is exactly the leak
/// this type exists to close. Variant matching is fine for the *cosmetic*
/// choice (which keyboard layout to request).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ImeContentType {
    /// No hint: ordinary text, platform defaults (suggestions, autocorrect and
    /// personalized learning all as the user configured them).
    ///
    /// The default, and what every field publishes unless it opts in.
    #[default]
    Normal,
    /// Secret text (password / passphrase / PIN entered as text).
    ///
    /// The shell must request secure entry *and* suppress suggestions and
    /// personalized learning.
    Password,
    /// Non-secret text that must not be autocorrected, suggested, or learned
    /// (recovery codes, identifiers, usernames).
    ///
    /// Distinct from [`Password`](Self::Password): the platform does **not**
    /// switch to secure entry, so autofill/reveal-last-character behaviour is
    /// unchanged; only the suggestion/learning channel is closed.
    NoSuggestions,
    /// A raw byte-entry surface (a terminal/shell keystroke source): no
    /// suggestion strip, no autocorrect, no smart quotes/dashes/insert-delete,
    /// no autocapitalization. Text is **not** masked — this is not a secret
    /// field, it is a field where every character the user typed must reach
    /// the app byte-for-byte with zero platform "correction" applied to it.
    ///
    /// The defect this closes is the same class [`Password`](Self::Password)
    /// closes for secrets: a smart keyboard silently substituting `"` for a
    /// curly quote or `--` for an em dash corrupts a shell command exactly as
    /// it corrupts a password, just without the confidentiality angle. Distinct
    /// from [`NoSuggestions`](Self::NoSuggestions): that variant suppresses the
    /// suggestion/learning channel only, while `Terminal` additionally
    /// suppresses smart punctuation and autocapitalization, both of which
    /// silently rewrite the text a suggestion-only hint leaves untouched.
    Terminal,
}

impl ImeContentType {
    /// Whether the field holds a secret the platform must treat as such
    /// (secure entry on iOS, a password `InputType` variation on Android).
    ///
    /// Shells gate secure-entry configuration on this, not on a variant match
    /// (see the type docs' matching rule).
    ///
    /// This predicate and [`suppresses_suggestions`](Self::suppresses_suggestions)
    /// match exhaustively (no `_` arm) on purpose: adding a variant to this enum
    /// is a compile error here until it is classified as secret or not.
    pub fn is_secret(self) -> bool {
        match self {
            Self::Password => true,
            Self::Normal | Self::NoSuggestions | Self::Terminal => false,
        }
    }

    /// Whether the platform must suppress its suggestion strip, autocorrect,
    /// and persistent word learning for this field.
    ///
    /// True for every secret content type and for
    /// [`NoSuggestions`](Self::NoSuggestions) and [`Terminal`](Self::Terminal).
    pub fn suppresses_suggestions(self) -> bool {
        match self {
            Self::Password | Self::NoSuggestions | Self::Terminal => true,
            Self::Normal => false,
        }
    }
}

/// The IME-relevant surface a focused editable widget publishes for the shell.
///
/// Written by the focused widget through [`EventCtx::publish_ime_state`], it
/// bubbles up the focus chain and is stored on [`crate::app::RenderRoot`], where
/// the shell reads it via [`crate::app::RenderRoot::ime_state`] to drive the
/// platform IME (winit `set_ime_cursor_area`, Android `updateSelection`, iOS
/// `inputDelegate`). See the module docs for the index boundary rule.
///
/// # `editing` carries the real text, even for a secret field
///
/// [`content_type`](Self::content_type) marks a field secret; it does **not**
/// redact [`editing`](Self::editing). That is deliberate: this struct is one
/// half of a **bidirectional state-sync mirror** (see `docs/CODE_STANDARDS.md`'s
/// state-sync rule) — the platform keeps a local `Editable`/`UITextInput` mirror
/// seeded from these exact fields and hands a whole reconciled
/// [`EditingState`] back through [`ImeEvent::ApplyEditingState`]. Publishing
/// redacted or masked text would desynchronize that mirror (the platform would
/// compute deletions/replacements against text the widget does not have, and
/// would echo the mask back as the field's new value), and it would not close
/// the leak anyway: the keyboard process is where the characters originate.
/// What a hint *does* close is the suggestion strip reading the field's text and
/// the IME persisting it to a learned-word dictionary.
///
/// **Residual exposure:** the plaintext still crosses the FFI seam into the
/// platform IME. A hostile or non-compliant third-party keyboard can read it.
/// That is unavoidable on both mobile platforms short of not using the platform
/// IME at all. As partial mitigation, this type's [`fmt::Debug`] redacts the
/// text whenever the content type is secret, so a trace log never carries it.
#[derive(Clone, PartialEq)]
pub struct ImeState {
    /// Whether the focused widget currently wants IME active.
    pub active: bool,
    /// The current editing state (UTF-16 indexed at this shell-facing surface).
    pub editing: EditingState,
    /// The caret rectangle in logical coordinates, for IME candidate placement.
    pub caret: Option<Rect>,
    /// What kind of content the field holds, so the shell can configure the
    /// platform IME. Defaults to [`ImeContentType::Normal`] — a field that says
    /// nothing behaves exactly as it did before this hint existed.
    pub content_type: ImeContentType,
    /// Whether the field wants the platform input surface **without** an
    /// on-screen keyboard.
    ///
    /// A field whose text cannot be changed is still focusable and copyable
    /// (Material 3 and Apple's HIG both keep it so), and copying is exactly
    /// what needs the surface: the web overlay `<input>`'s DOM `copy`
    /// listener, Android's `InputConnection` and iOS's first responder are
    /// each the route a clipboard verb travels, and all three exist only while
    /// [`active`](Self::active) holds. What such a field does not need is
    /// somewhere to type — so this asks the shell to keep the surface wired and
    /// suppress the soft keyboard it would otherwise raise.
    ///
    /// Defaults to `false` — a field that says nothing behaves exactly as it
    /// did before this hint existed. It says nothing about an inactive surface
    /// (there is no keyboard up to suppress), and a shell with no on-screen
    /// keyboard of its own has nothing to do for it.
    pub suppress_soft_keyboard: bool,
}

impl Default for ImeState {
    /// A cleared, inactive surface with no hint — what a container publishes
    /// when it stops routing to an editable child.
    fn default() -> Self {
        Self {
            active: false,
            editing: EditingState::default(),
            caret: None,
            content_type: ImeContentType::Normal,
            suppress_soft_keyboard: false,
        }
    }
}

impl fmt::Debug for ImeState {
    /// Hand-written so a secret field's text never reaches a log.
    ///
    /// Everything except [`EditingState::text`] prints as derived; for a secret
    /// [`content_type`](Self::content_type) the text is replaced by
    /// `<redacted>` (no length, which would itself leak).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        struct Redacted<'a>(&'a EditingState);
        impl fmt::Debug for Redacted<'_> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_struct("EditingState")
                    .field("text", &"<redacted>")
                    .field("selection_base", &self.0.selection_base)
                    .field("selection_extent", &self.0.selection_extent)
                    .field("composing_base", &self.0.composing_base)
                    .field("composing_extent", &self.0.composing_extent)
                    .finish()
            }
        }

        let mut s = f.debug_struct("ImeState");
        s.field("active", &self.active);
        if self.content_type.is_secret() {
            s.field("editing", &Redacted(&self.editing));
        } else {
            s.field("editing", &self.editing);
        }
        s.field("caret", &self.caret)
            .field("content_type", &self.content_type)
            .field("suppress_soft_keyboard", &self.suppress_soft_keyboard)
            .finish()
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
/// [`crate::widget::LayoutCtx`] uses for the text context — so `frust-core`
/// carries no knowledge of the concrete app state type; a handler recovers it
/// with [`EventCtx::state_mut`].
pub struct EventCtx<'a> {
    state: &'a mut dyn Any,
    needs_redraw: bool,
    capture_requested: bool,
    /// Set by [`EventCtx::capture_contacts`]; bubbles up beside
    /// `capture_requested` so a container can see the opt-in.
    contacts_requested: bool,
    /// Set by [`EventCtx::release_captured_child`] when the release took the
    /// gesture's contact opt-in off the active path; bubbles up beside
    /// `capture_requested` ([`EventCtx::is_capture_released`]).
    capture_released: bool,
    /// Which contact this dispatch carries ([`EventCtx::pointer_id`]); threaded
    /// unchanged from parent to child.
    pointer_id: PointerId,
    /// Set by [`EventCtx::request_focus`]; read by the enclosing container to
    /// record which child holds the focus path (mirrors `capture_requested`).
    focus_requested: bool,
    /// Set by [`EventCtx::release_focus`]; drops the recorded focus path.
    focus_released: bool,
    /// Whether the receiving widget currently holds focus (threaded down from its
    /// pod's recorded focus flag; seeded from the root focus state at the root).
    has_focus: bool,
    /// Whether the receiving widget is on the recorded hover path — the pointer is
    /// over it or over a descendant of it, as of the last completed hover pass.
    /// Threaded down from its
    /// pod's recorded hover stamp (see `hover_epoch`), seeded from
    /// [`crate::app::RenderRoot`]'s own hover mirror at the root. The event-pass
    /// mirror of [`PaintCtx::is_hovered`](crate::widget::PaintCtx::is_hovered).
    hovered: bool,
    /// Set by [`EventCtx::claim_hover`]; read by the enclosing container, which
    /// stamps the claim onto its child's pod and bubbles it further up (the hover
    /// mirror of `focus_requested`).
    hover_claimed: bool,
    /// Whether a [`EventCtx::claim_hover`] call in this (sub)dispatch records
    /// anything at all. `false` unless the root marked this pass an uncaptured
    /// [`PointerPhase::Move`], and narrowed further on the way down: a pod holding
    /// the capture path, or a pass in which a claim was already recorded, hands
    /// its child an ineligible context. This is what makes "a captured pointer
    /// never creates hover" and "at most one claimant per pass" true by
    /// construction rather than by a check the root has to remember.
    hover_eligible: bool,
    /// The hover epoch of the last **completed** hover pass — what a pod's
    /// recorded stamp must equal for its link to still count
    /// ([`EventCtx::hover_epoch`]). A claim made during *this* pass records
    /// [`EventCtx::hover_claim_epoch`] instead (one past this value), because the
    /// root advances its own epoch when the pass ends.
    hover_epoch: u64,
    /// The identity of the [`crate::app::RenderRoot`] running this pass, stamped
    /// onto a pod beside the claim epoch ([`EventCtx::hover_root`]) so a pod
    /// dropped later can tell its own root's live link from another root's
    /// identically-numbered epoch. `0` outside a root-driven pass.
    hover_root: u64,
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
            contacts_requested: false,
            capture_released: false,
            // The contact the running root pass is dispatching, so a context
            // built mid-pass (a component's inner one) reports the same id its
            // enclosing context does; the mouse outside any pass.
            pointer_id: current_pointer_id(),
            focus_requested: false,
            focus_released: false,
            has_focus: false,
            hovered: false,
            hover_claimed: false,
            hover_eligible: false,
            hover_epoch: 0,
            hover_root: 0,
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
    /// widget. The [`ChildPod::event_child`](crate::widget::ChildPod::event_child)
    /// call that delivered the event reads [`EventCtx::is_pointer_captured`]
    /// after the dispatch returns and records the active path on its pod; the
    /// container clears it on `Up`/`Cancel`.
    ///
    /// **Capture is a `Down`-time concept here.** For a hit-tested pointer event,
    /// only the `Down` arm of [`RenderRoot::event`](crate::app::RenderRoot::event)
    /// folds a request into the root's own capture mirror, so a capture opened
    /// from a `Move` records the pod's active path (routing works) while the root
    /// still reads uncaptured. (A floated overlay surface's own input is the one
    /// exception: the root mirrors a capture claimed through it on any phase —
    /// see [`InputEvent::Overlay`].) For hover that means a `Move` that both
    /// captures and [`claim_hover`](EventCtx::claim_hover)s records the claim — the pod's
    /// eligibility gate reads the active flag as it stood *before* this dispatch —
    /// and then lapses on the next `Move`, where the now-active pod is ineligible.
    /// A gesture that wants hover chrome for its whole drag keeps its own pressed
    /// flag rather than relying on the link.
    pub fn capture_pointer(&mut self) {
        self.capture_requested = true;
    }

    /// Whether the widget requested pointer capture during this (sub)dispatch.
    pub fn is_pointer_captured(&self) -> bool {
        self.capture_requested
    }

    /// Which pointer contact this event comes from.
    ///
    /// [`PointerId::MOUSE`] unless the dispatch says otherwise: a shell's bare
    /// [`InputEvent::Pointer`] is the mouse, and a touch contact arrives as
    /// [`InputEvent::PointerContact`], which the root unwraps into
    /// `InputEvent::Pointer` while this reports its id. Meaningful for pointer
    /// events only; any other event reports whatever contact the pass carries
    /// (the mouse at the top level).
    ///
    /// A widget that tracks one gesture at a time never needs this — the root
    /// only ever delivers it the claimant's contact unless it opted in with
    /// [`EventCtx::capture_contacts`]. One that did opt in tells the contacts
    /// apart with it.
    pub fn pointer_id(&self) -> PointerId {
        self.pointer_id
    }

    /// Opt into the **other** contacts of the gesture this widget is capturing:
    /// call it on the same `Down` that calls [`EventCtx::capture_pointer`], and
    /// while that capture lives every additional contact's
    /// `Down`/`Move`/`Up`/`Cancel` is delivered here down the captured path, as
    /// [`InputEvent::Pointer`] with [`EventCtx::pointer_id`] naming the contact.
    /// A pinch or rotate recognizer is the intended caller.
    ///
    /// The opt-in is read with the capture it accompanies: on a `Down` that
    /// captures nothing it does nothing, and it ends with the capture.
    ///
    /// # Multi-contact contract
    ///
    /// * **(a)** With no live capture, a slot-`0` contact is hit-tested exactly
    ///   like a plain [`InputEvent::Pointer`]; the `Down` this widget captures
    ///   on makes that contact the gesture's **claimant**.
    /// * **(b)** With no live capture, a contact on slot `1` or above is dropped
    ///   at the root — so an additional finger only ever reaches a widget
    ///   through this opt-in.
    /// * **(c)** While the capture lives, the claimant's events arrive as usual;
    ///   every other contact's events arrive only because of this call (a
    ///   captor that did not make it never sees them), and **only here**: the
    ///   containers between the root and this widget on the recorded active
    ///   path forward them without running their own pointer handling (see
    ///   [`ChildPod::event_child`](crate::widget::ChildPod::event_child)), so an
    ///   enclosing scroll view or gesture detector never mistakes a second
    ///   finger for its own. Only the claimant's `Up`/`Cancel` ends the
    ///   capture — another contact's `Up`/`Cancel` is delivered but releases
    ///   nothing, so the handler must not treat it as the end of the gesture.
    ///   When the claimant's `Up`/`Cancel` arrives, the captor must drop every
    ///   other contact it was tracking: their later events no longer reach it.
    ///   The opt-in also ends early if an enclosing container takes the gesture
    ///   over and releases this widget from the active path
    ///   ([`EventCtx::release_captured_child`]).
    ///
    /// See [`InputEvent::PointerContact`] for the full contract.
    pub fn capture_contacts(&mut self) {
        self.contacts_requested = true;
        note_contacts_requested();
    }

    /// Whether a widget opted into the gesture's other contacts
    /// ([`EventCtx::capture_contacts`]) during this (sub)dispatch — the
    /// container-side read, bubbled by
    /// [`ChildPod::event_child`](crate::widget::ChildPod::event_child) exactly
    /// like [`EventCtx::is_pointer_captured`]. The root does not depend on the
    /// bubble: it reads the opt-in from the pass it opened, so a component
    /// boundary (whose fresh inner context this flag does not cross) cannot
    /// hide it.
    pub fn is_contact_capture_requested(&self) -> bool {
        self.contacts_requested
    }

    /// Release a captured child: the container-side half of a **takeover**, for
    /// a container that cancels the gesture its captured child was handling and
    /// keeps the gesture for itself (a scroll view crossing its drag slop).
    ///
    /// Clears `child`'s recorded active path exactly like
    /// [`ChildPod::set_active`](crate::widget::ChildPod::set_active)`(false)`
    /// (call it after delivering the child its `Cancel`), and when the released
    /// subtree held the widget that opted into the gesture's other contacts
    /// ([`EventCtx::capture_contacts`]) it also tells the root, which then stops
    /// routing those contacts — the widget that asked for them is no longer on
    /// the active path, and nothing else asked. The signal bubbles like
    /// [`EventCtx::is_pointer_captured`] ([`EventCtx::is_capture_released`]).
    ///
    /// The capture itself stays with the gesture's claimant: the container that
    /// took over is still on the active path (it was the released child's
    /// ancestor), so the claimant's later events keep reaching it and only the
    /// claimant's `Up`/`Cancel` ends the gesture. A container that wants the
    /// other contacts for itself calls [`EventCtx::capture_contacts`] after
    /// this, in the same dispatch.
    ///
    /// Nothing is released or signalled while the child holds no active path,
    /// while a non-claimant contact is being delivered (whose `Up`/`Cancel`
    /// must never break the claimant's gesture — see
    /// [`ChildPod::set_active`](crate::widget::ChildPod::set_active)), or when
    /// the opted-in widget is this container or one of its ancestors (it stays
    /// on the active path, so its opt-in stands).
    pub fn release_captured_child(&mut self, child: &mut crate::widget::ChildPod) {
        if !child.is_active() {
            return;
        }
        let held_opt_in = child.holds_contact_opt_in();
        child.set_active(false);
        if child.is_active() || !held_opt_in || under_contact_captor() {
            return;
        }
        self.capture_released = true;
        note_capture_released();
    }

    /// Whether a container released the gesture's opted-in widget from the
    /// active path during this (sub)dispatch
    /// ([`EventCtx::release_captured_child`]) — bubbled by
    /// [`ChildPod::event_child`](crate::widget::ChildPod::event_child) exactly
    /// like [`EventCtx::is_pointer_captured`]. As with the opt-in, the root reads
    /// the release from the pass it opened, so a component boundary cannot hide
    /// it.
    pub fn is_capture_released(&self) -> bool {
        self.capture_released
    }

    /// Request focus: subsequent keyboard/IME events should route to this widget.
    ///
    /// The [`ChildPod::event_child`](crate::widget::ChildPod::event_child) call
    /// that delivered the event reads the flag after the dispatch returns and
    /// records its pod as the focused path, stamped with the live focus session
    /// (the focus mirror of [`EventCtx::capture_pointer`]); the request bubbles,
    /// so every pod up to the root records it. Focus-routed events are delivered
    /// down that recorded chain with no hit test.
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

    /// Claim the hover link: the pointer is over *this* widget, so the next paint
    /// pass reports [`PaintCtx::is_hovered`](crate::widget::PaintCtx::is_hovered)
    /// for it — and, because the claim is recorded as a path, for every ancestor
    /// enclosing it as well (see the [module docs](crate::event)).
    ///
    /// # The consumer contract
    ///
    /// Three things together, all three required:
    ///
    /// 1. **Claim from the [`PointerPhase::Move`] arm**, once the widget has
    ///    hit-tested the event's `position` inside its own bounds — the same local
    ///    test a press arm does on `Up`.
    /// 2. **Keep an internal hover flag**, updated from that same hit test, and
    ///    gate `request_redraw` on its *changed*-return. This call requests no
    ///    frame of its own (below), and the root manufactures one only when a hover
    ///    ends with nothing taking it — so a widget without this flag paints no
    ///    hover chrome on entry, and none when the link moves from a sibling to it.
    /// 3. **Read [`PaintCtx::is_hovered`](crate::widget::PaintCtx::is_hovered) in
    ///    `paint` and self-correct the flag from it.** It is authoritative: the
    ///    flag can be stale (a pointer that left the widget never delivers it
    ///    another event; a container clearing or lapsing a link never tells the
    ///    widget either), and this read is what fixes it.
    ///
    /// ```ignore
    /// PointerPhase::Move => {
    ///     if !self.captured {
    ///         // Uncaptured move: this is the hover pass.
    ///         let over = inside(p.position, ctx.size());
    ///         if over { ctx.claim_hover(); }
    ///         if self.state_layer.set_hovered(over) { ctx.request_redraw(); }
    ///         return EventResult::Ignored;
    ///     }
    ///     // ... captured drag handling
    /// }
    ///
    /// fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
    ///     // Authoritative; corrects the flag above whenever it went stale.
    ///     self.state_layer.set_hovered(ctx.is_hovered());
    ///     // ... paint the overlay
    /// }
    /// ```
    ///
    /// **Claim on every qualifying `Move`, not just on entry.** The claim is
    /// per-pass, not sticky: a widget that stops claiming stops being hovered on
    /// the next hover pass. That is the mechanism, not a defect — it is what makes
    /// "the pointer moved somewhere else" self-clearing with no leave event to
    /// deliver.
    ///
    /// **A container claims *after* routing the `Move` to its children, never
    /// before.** One claim per pass is recorded and the first one recorded wins, so
    /// an ancestor that claims before it forwards makes every descendant ineligible
    /// for the pass: the child under the pointer reads
    /// [`is_hovered`](EventCtx::is_hovered) `== false` forever while step 2 above
    /// keeps flipping its flag and asking for a frame on every move — hover chrome
    /// that never appears, plus a repaint per event. Claiming after routing is
    /// correct in every case: a descendant's claim is recorded first and wins, the
    /// container's own late call is then a silent no-op yet it still reads hovered
    /// through the stamped path (below), and when no descendant claims, the
    /// container's claim is what records, so its own chrome still works. A
    /// container therefore never arbitrates — it orders.
    ///
    /// **Its sibling channel resolves the opposite way.** A claim is
    /// *first*-writer-wins; [`set_cursor`](EventCtx::set_cursor) is
    /// *last*-writer-wins. So the same "claim/ask after routing" placement means
    /// two different things in one handler: the container's claim is a **fallback**
    /// its child beats, while the container's cursor request is an **override**
    /// that beats its child's. A container that wants the child's cursor to win
    /// must ask *before* it routes — the mirror image of the ordering here.
    ///
    /// # When it does nothing
    ///
    /// A call is silently ignored unless the pass is hover-eligible: a captured
    /// pointer (anywhere on the path), any phase other than an uncaptured `Move`,
    /// and any claim after the first one in the same pass all record nothing. A
    /// **leaf** therefore never has to ask whether claiming is allowed — it claims
    /// whenever the pointer is over it and the pipeline decides. A **container**
    /// gets the same freedom only by claiming after it routes: it never asks
    /// either, but *when* it claims decides whether its children may, per the
    /// ordering rule above.
    ///
    /// A `Down`, `Up`, or `Cancel` *ends* whatever hover stood without opening a
    /// new one, so a consumer re-claims on the next `Move` rather than expecting
    /// its chrome to survive a click.
    ///
    /// # It does not request a redraw
    ///
    /// Deliberately: a pointer moving *within* one widget claims on every event,
    /// and repainting each time would be pure waste. The widget owns the change
    /// detection instead — which is what makes step 2 above part of the contract
    /// rather than an optimization.
    pub fn claim_hover(&mut self) {
        if self.hover_eligible {
            self.hover_claimed = true;
        }
    }

    /// Whether the receiving widget **or a descendant of it** holds the hover link
    /// — i.e. whether the last completed hover pass recorded a claim path running
    /// through this widget.
    ///
    /// So a container reads `true` while the pointer is over a claiming child
    /// (CSS `:hover` semantics), and a widget that never claims can still read
    /// `true` when a descendant does; a sibling or any other off-path widget reads
    /// `false`.
    ///
    /// Threaded down from the widget's pod
    /// ([`ChildPod::hover_epoch`](crate::widget::ChildPod::hover_epoch) against
    /// the live epoch) and seeded at the root from `RenderRoot`'s hover mirror, so
    /// it reflects state as of *before* this dispatch: a
    /// [`claim_hover`](EventCtx::claim_hover) made in this pass does not flip it.
    /// Mirrors [`EventCtx::has_focus`]; the paint-pass form is
    /// [`PaintCtx::is_hovered`](crate::widget::PaintCtx::is_hovered), which is the
    /// authoritative read for a widget's own hover chrome.
    pub fn is_hovered(&self) -> bool {
        self.hovered
    }

    /// Ask the host to show `icon` while the pointer is where it is now.
    ///
    /// The request is per-pass and stateless, exactly like
    /// [`request_redraw`](EventCtx::request_redraw) and
    /// [`claim_hover`](EventCtx::claim_hover): it says what the cursor should be
    /// *for this pass*, and a widget that stops asking falls back to
    /// [`CursorIcon::Default`] with nothing to clear.
    ///
    /// # When to call it
    ///
    /// From a [`PointerPhase::Move`] arm, on the same hit test a
    /// [`claim_hover`](EventCtx::claim_hover) rides — the two are siblings, and a
    /// widget that wants hover chrome usually wants a cursor too:
    ///
    /// ```ignore
    /// PointerPhase::Move => {
    ///     if !self.captured {
    ///         if inside(p.position, ctx.size()) {
    ///             ctx.claim_hover();
    ///             ctx.set_cursor(CursorIcon::Pointer);
    ///         }
    ///         return EventResult::Ignored;
    ///     }
    ///     // Captured drag: this widget owns the pass, so its request wins
    ///     // wherever the pointer has gone.
    ///     ctx.set_cursor(CursorIcon::Grabbing);
    ///     // ... drag handling
    /// }
    /// ```
    ///
    /// **Ask on every `Move`, not just on entry**, and ask from the captured
    /// `Move`s too if a drag should keep its own shape: a captured pass routes
    /// only to the capturing widget, so re-asking there is what keeps a
    /// `Grabbing` cursor alive while the pointer is dragged outside the widget's
    /// own bounds.
    ///
    /// # Which pass the root actually resolves
    ///
    /// Only a pointer [`PointerPhase::Move`] — captured or not — re-resolves the
    /// cursor ([`crate::app::RenderRoot::cursor`]). A request made on any other
    /// pass records nothing, and, just as importantly, no other pass *resets* the
    /// cursor: a `Down`/`Up` whose handlers say nothing about the cursor leaves
    /// the standing shape alone rather than blinking it back to `Default` for the
    /// duration of a click. A widget wanting a press-specific cursor therefore
    /// keys it off its own pressed state from the `Move` arm rather than setting
    /// it on `Down`.
    ///
    /// # Last writer wins
    ///
    /// One value is resolved per pass, and the last `set_cursor` of the pass is
    /// it. Because a container routes to its child from the middle of its own
    /// handler, the innermost widget the route reaches normally speaks last and
    /// therefore wins — which is what makes a specific control override the
    /// generic surface behind it. A container that deliberately overrides its
    /// children sets the cursor *after* routing.
    ///
    /// **Note the asymmetry with [`claim_hover`](EventCtx::claim_hover)**, which
    /// is first-writer-wins: a container claiming after routing yields hover to
    /// its child, while a container asking for a cursor after routing overrides
    /// its child. Placing the two calls side by side in one `Move` arm — the
    /// example above — is correct precisely because a leaf has no child to order
    /// against; a *container* writing both has to place them separately.
    ///
    /// # It does not request a redraw
    ///
    /// Deliberately, for [`claim_hover`](EventCtx::claim_hover)'s reason: a
    /// pointer moving within one widget re-asks on every event, and the shell
    /// applies the resolved cursor whether or not a frame is painted.
    ///
    /// Takes `&mut self` like every other request on this context even though the
    /// pass's request slot is not a field of it (`CURSOR_REQUEST`, above): asking
    /// is something a widget does *through its context*, and keeping the signature
    /// honest about that leaves the storage free to move.
    pub fn set_cursor(&mut self, icon: CursorIcon) {
        CURSOR_REQUEST.with(|slot| slot.set(Some(icon)));
    }

    /// Ask the shell to put `text` on the host clipboard.
    ///
    /// The answer to an [`InputEvent::EditCommand`]`(`[`EditCommand::Copy`]`)` or
    /// [`EditCommand::Cut`]: the widget owns the selection, so it is the only
    /// thing that can say what "copy" means, and the shell owns the host
    /// clipboard, so it is the only thing that can perform the write. A widget
    /// with nothing selected simply does not call this, and nothing is written.
    ///
    /// ```ignore
    /// InputEvent::EditCommand(EditCommand::Cut) => {
    ///     if let Some(sel) = self.selected_text() {
    ///         ctx.write_clipboard(sel);
    ///         self.delete_selection();
    ///         ctx.request_redraw();
    ///     }
    ///     EventResult::Handled
    /// }
    /// ```
    ///
    /// # Pass-scoped, last writer wins
    ///
    /// The request rides the same kind of per-pass slot as
    /// [`set_cursor`](EventCtx::set_cursor) (`CLIPBOARD_WRITE`, bracketed by the
    /// same [`RequestPass`] guard), so exactly one write is resolved per dispatch
    /// and the pass's last caller is it — which, since a container routes to its
    /// child from the middle of its own handler, normally makes the innermost
    /// widget the route reaches the one that speaks. Nothing accumulates between
    /// passes and there is nothing to clear: a widget that stops copying stops
    /// writing.
    ///
    /// The root resolves the slot at the end of **every** pass (not just a
    /// clipboard one — a copy can be answered from a key chord a widget decoded
    /// itself), and a shell drains it with
    /// [`RenderRoot::take_clipboard_write`](crate::app::RenderRoot::take_clipboard_write)
    /// immediately after the dispatch, beside
    /// [`cursor()`](crate::app::RenderRoot::cursor) and
    /// [`ime_state()`](crate::app::RenderRoot::ime_state).
    ///
    /// # It does not request a redraw
    ///
    /// [`set_cursor`](EventCtx::set_cursor)'s reason: copying paints nothing. A
    /// `Cut` that mutates the document asks for its own redraw, for the mutation.
    ///
    /// Takes `&mut self` like every other request on this context even though the
    /// pass's slot is not a field of it: asking is something a widget does
    /// *through its context*, and keeping the signature honest about that leaves
    /// the storage free to move.
    pub fn write_clipboard(&mut self, text: String) {
        CLIPBOARD_WRITE.with(|slot| slot.set(Some(text)));
    }

    /// Ask the shell to read the host clipboard and deliver it back as an
    /// [`InputEvent::EditCommand`]`(`[`EditCommand::Paste`]`)`.
    ///
    /// The inverse of [`write_clipboard`](EventCtx::write_clipboard), and the
    /// reason a paste arrives with its text already attached: only the shell can
    /// touch the host clipboard, so a widget that wants a paste it was not given
    /// — an in-widget context-menu item, a chord the widget decoded itself —
    /// raises this flag and receives the text on a *later* dispatch rather than
    /// inline.
    ///
    /// # Idempotent, pass-scoped, and answered out of band
    ///
    /// Data-free: two widgets asking in one pass owe exactly one clipboard read,
    /// because there is one host clipboard and one focused widget to deliver it
    /// to. The flag rides a per-pass slot bracketed by the same [`RequestPass`]
    /// guard as the cursor, so an ask made outside any dispatch is dropped rather
    /// than leaking into the next pass; the root resolves it at the end of every
    /// pass and a shell drains it with
    /// [`RenderRoot::take_paste_request`](crate::app::RenderRoot::take_paste_request).
    ///
    /// The answer is a **new dispatch**, never a return value: the shell's read
    /// may be asynchronous (a permission prompt, a cross-process fetch), and by
    /// the time it lands the pass that asked is long over. The synthesized
    /// [`EditCommand::Paste`] carries text and no identity of its own, and focus
    /// routing hands it to whoever holds focus *at delivery*: a **release** does
    /// drop it — with nothing focused it reaches no widget — but a focus *move*
    /// lands it in the new field, not in the one that asked.
    ///
    /// A synchronous read has no in-flight window and needs no guard. An
    /// asynchronous one must bind its answer to the session that asked:
    /// snapshot [`RenderRoot::focus_epoch`](crate::app::RenderRoot::focus_epoch)
    /// (reached shell-side as `AppTree::focus_epoch`) when the request is
    /// drained, and drop an answer whose epoch no longer matches — never
    /// [`focus_ime_generation`](crate::app::RenderRoot::focus_ime_generation),
    /// which an edit or a caret move inside one session also moves.
    ///
    /// A `Cut` may write and ask in the same pass; the two slots are independent.
    pub fn request_paste(&mut self) {
        PASTE_REQUEST.with(|slot| slot.set(true));
    }

    /// Hand an [`EditCommand`] to whichever widget drains the queue later in
    /// **this** pass — the widget-to-widget half of the clipboard story.
    ///
    /// # Why this is not just an `InputEvent::EditCommand`
    ///
    /// A selection toolbar and the text input it acts on are two different
    /// widgets, and the toolbar is a pod its owner floats rather than contains
    /// (see [`crate::overlay`]), so there is no container path from the toolbar's
    /// "Copy" tap back down to the field. Re-entering
    /// [`crate::app::RenderRoot::event`] with a focus-routed
    /// [`InputEvent::EditCommand`] would be the other option, and is worse: a
    /// dispatch may not re-enter the root (see that method's reentrancy
    /// contract), and the toolbar's tap is *already* being routed as an overlay
    /// broadcast when it decides. So the verb rides a pass-scoped FIFO the owner
    /// drains the instant its forward returns, and applies to the field itself —
    /// synchronously, inside the same dispatch.
    ///
    /// Order is preserved: commands drain in the order they were dispatched.
    ///
    /// A command nobody takes before the pass ends is **dropped** (with a
    /// debug-build diagnostic) rather than carried into the next pass, where it
    /// would apply to whatever happened to be selected by then.
    pub fn dispatch_edit_command(&mut self, cmd: EditCommand) {
        EDIT_COMMAND_QUEUE.with(|slot| {
            let mut queue = slot.take();
            queue.push(cmd);
            slot.set(queue);
        });
    }

    /// Drain everything [`EventCtx::dispatch_edit_command`] queued so far in this
    /// pass, in dispatch order, leaving the queue empty.
    ///
    /// An overlay owner calls this immediately after forwarding an event into its
    /// floated pod, and applies what comes back to itself. Draining the queue
    /// (rather than peeking) is what keeps a verb from being applied twice when
    /// two owners forward in the same pass.
    pub fn take_edit_commands(&mut self) -> Vec<EditCommand> {
        EDIT_COMMAND_QUEUE.with(|slot| slot.take())
    }

    /// Publish this widget's IME surface (editing state + caret) for the shell.
    ///
    /// The value bubbles up the focus chain to [`crate::app::RenderRoot`], where
    /// the shell reads it via [`crate::app::RenderRoot::ime_state`]. Called by the
    /// focused editable widget after any state change so the platform IME stays in
    /// sync.
    ///
    /// Core carries the whole [`ImeState`] — including its
    /// [`ImeContentType`](ImeState::content_type) hint — opaquely: nothing
    /// between here and the shell inspects or rewrites it.
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

    /// Whether a widget claimed hover during this (sub)dispatch (container-side).
    pub(crate) fn is_hover_claimed(&self) -> bool {
        self.hover_claimed
    }

    /// Whether a [`EventCtx::claim_hover`] call in this (sub)dispatch would record
    /// anything — read by [`crate::widget::ChildPod::event_child`], which narrows
    /// it further before handing it to a child.
    pub(crate) fn is_hover_eligible(&self) -> bool {
        self.hover_eligible
    }

    /// Seed whether the receiving (root) widget holds the hover link — the hover
    /// mirror of [`EventCtx::set_has_focus`].
    pub(crate) fn set_hovered(&mut self, hovered: bool) {
        self.hovered = hovered;
    }

    /// Seed whether this pass may record a hover claim at all. Called by
    /// [`crate::app::RenderRoot::event`], which sets it only for an **uncaptured**
    /// [`PointerPhase::Move`].
    pub(crate) fn set_hover_eligible(&mut self, eligible: bool) {
        self.hover_eligible = eligible;
    }

    /// Seed the live hover epoch (the last completed hover pass's). Called by
    /// [`crate::app::RenderRoot::event`] at the root and threaded unchanged into
    /// every child by [`crate::widget::ChildPod::event_child`].
    pub(crate) fn set_hover_epoch(&mut self, epoch: u64) {
        self.hover_epoch = epoch;
    }

    /// The live hover epoch: a pod whose recorded stamp equals this still holds
    /// the hover link.
    pub(crate) fn hover_epoch(&self) -> u64 {
        self.hover_epoch
    }

    /// Seed the identity of the root running this pass. Called by
    /// [`crate::app::RenderRoot::event`] at the root and threaded unchanged into
    /// every child by [`crate::widget::ChildPod::event_child`], which stamps it
    /// beside the claim epoch.
    pub(crate) fn set_hover_root(&mut self, root: u64) {
        self.hover_root = root;
    }

    /// The identity of the root running this pass — stamped onto a claiming pod
    /// so its destructor can qualify its epoch (see [`mark_hover_orphaned`]).
    pub(crate) fn hover_root(&self) -> u64 {
        self.hover_root
    }

    /// The epoch a claim recorded during *this* pass takes — one past the live
    /// one, because [`crate::app::RenderRoot::event`] advances its epoch when the
    /// hover pass ends. Wrapping is deliberate and harmless: the stamp is only
    /// ever compared for equality, never ordered, and a wrap would need 2^64 hover
    /// passes to collide with a link recorded before it.
    pub(crate) fn hover_claim_epoch(&self) -> u64 {
        self.hover_epoch.wrapping_add(1)
    }

    /// The receiving widget's origin in its **parent's** coordinate space.
    ///
    /// Not the window-space origin, and **not** the same frame of reference as
    /// [`PaintCtx::origin`](crate::widget::PaintCtx::origin), which is absolute:
    /// the paint pass accumulates each child's parent-relative offset onto its
    /// parent's already-absolute origin, while the event pass instead translates
    /// the *event* into the child's local space
    /// ([`ChildPod::event_child`](crate::widget::ChildPod::event_child)) and hands
    /// down the pod's own offset unaccumulated. So an event position is already
    /// local (compare it against `Point::ZERO` and [`EventCtx::size`], never
    /// against this), and anything anchored in window space — an overlay, a
    /// popup, a reported rect — must be computed from `PaintCtx::origin` in
    /// `paint`, not from this value.
    pub fn origin(&self) -> Point {
        self.origin
    }

    /// The receiving widget's resolved size.
    pub fn size(&self) -> Size {
        self.size
    }

    /// Create a fresh sub-context for a child at `origin`/`size`, reborrowing the
    /// same erased state. The child's `needs_redraw`/`capture_requested`/focus and
    /// hover-claim flags start clear; `has_focus` reflects the child pod's recorded
    /// focus flag, `hovered` its recorded hover link, and `hover_eligible` whether
    /// the child may claim hover at all (the caller narrows it — see
    /// [`crate::widget::ChildPod::event_child`]). The live hover epoch, the
    /// running root's identity and the dispatch's [`EventCtx::pointer_id`] are
    /// threaded down unchanged (a claim anywhere in the subtree is stamped with
    /// the first two). The parent folds the results back in with
    /// [`EventCtx::absorb_child`].
    pub(crate) fn child_ctx(
        &mut self,
        origin: Point,
        size: Size,
        focused: bool,
        hovered: bool,
        hover_eligible: bool,
    ) -> EventCtx<'_> {
        EventCtx {
            state: &mut *self.state,
            needs_redraw: false,
            capture_requested: false,
            contacts_requested: false,
            capture_released: false,
            pointer_id: self.pointer_id,
            focus_requested: false,
            focus_released: false,
            has_focus: focused,
            hovered,
            hover_claimed: false,
            hover_eligible,
            hover_epoch: self.hover_epoch,
            hover_root: self.hover_root,
            ime_state: None,
            origin,
            size,
        }
    }

    /// Fold a child dispatch's redraw/capture/contact-opt-in/capture-release/
    /// hover-claim/focus flags (and any published IME surface) back into this
    /// context.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn absorb_child(
        &mut self,
        child_needs_redraw: bool,
        child_captured: bool,
        child_contacts_requested: bool,
        child_capture_released: bool,
        child_hover_claimed: bool,
        child_focus_requested: bool,
        child_focus_released: bool,
        child_ime_state: Option<ImeState>,
    ) {
        self.needs_redraw |= child_needs_redraw;
        self.capture_requested |= child_captured;
        self.contacts_requested |= child_contacts_requested;
        self.capture_released |= child_capture_released;
        // A claim bubbles like a focus request: every pod between the claimant and
        // the root records it, so the whole path carries the same stamp.
        self.hover_claimed |= child_hover_claimed;
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
    fn transformed_maps_every_positioned_variant_like_translated() {
        // A pure translation through `transformed` must agree with `translated`
        // for every variant, positioned or not.
        let offset = Vec2::new(-5.0, -7.0);
        let affine = Affine::translate(offset);
        let scale = ScaleEvent {
            phase: ScalePhase::Update,
            scale_delta: 1.5,
            focal: Point::new(20.0, 30.0),
            velocity: 0.25,
        };
        let events = [
            down(20.0, 30.0),
            InputEvent::PointerContact {
                pointer_id: PointerId::touch(1),
                event: PointerEvent {
                    phase: PointerPhase::Move,
                    position: Point::new(20.0, 30.0),
                    button: PointerButton::Primary,
                },
            },
            InputEvent::Scroll {
                position: Point::new(20.0, 30.0),
                delta: ScrollDelta::Pixels(3.0, 4.0),
            },
            InputEvent::Scale(scale),
            InputEvent::Housekeeping,
        ];
        for event in &events {
            assert_eq!(event.transformed(&affine), event.translated(offset));
        }
    }

    #[test]
    fn transformed_maps_positions_through_scale_and_keeps_magnitudes() {
        // Inverse of scale(2) then translate(10, 20): container (30, 60) is
        // local (10, 20).
        let inverse = (Affine::translate(Vec2::new(10.0, 20.0)) * Affine::scale(2.0)).inverse();
        assert_eq!(
            down(30.0, 60.0).transformed(&inverse).position(),
            Point::new(10.0, 20.0)
        );
        let scroll = InputEvent::Scroll {
            position: Point::new(30.0, 60.0),
            delta: ScrollDelta::Pixels(3.0, 4.0),
        };
        assert_eq!(
            scroll.transformed(&inverse),
            InputEvent::Scroll {
                position: Point::new(10.0, 20.0),
                delta: ScrollDelta::Pixels(3.0, 4.0),
            }
        );
        let scale = InputEvent::Scale(ScaleEvent {
            phase: ScalePhase::Begin,
            scale_delta: 1.25,
            focal: Point::new(30.0, 60.0),
            velocity: 2.0,
        });
        assert_eq!(
            scale.transformed(&inverse),
            InputEvent::Scale(ScaleEvent {
                phase: ScalePhase::Begin,
                scale_delta: 1.25,
                focal: Point::new(10.0, 20.0),
                velocity: 2.0,
            })
        );
    }

    #[test]
    fn absorb_child_folds_flags_upward() {
        let mut count = 0u32;
        let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::ZERO);
        {
            let mut child = ctx.child_ctx(
                Point::new(1.0, 2.0),
                Size::new(3.0, 4.0),
                false,
                false,
                false,
            );
            child.request_redraw();
            child.capture_pointer();
            let (redraw, cap) = (child.needs_redraw(), child.is_pointer_captured());
            ctx.absorb_child(redraw, cap, false, false, false, false, false, None);
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
        let focused_child = ctx.child_ctx(Point::ZERO, Size::ZERO, true, false, false);
        assert!(focused_child.has_focus());
        let unfocused_child = ctx.child_ctx(Point::ZERO, Size::ZERO, false, false, false);
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
            content_type: ImeContentType::Normal,
            suppress_soft_keyboard: false,
        };
        {
            let mut child = ctx.child_ctx(Point::ZERO, Size::ZERO, false, false, false);
            child.request_focus();
            child.publish_ime_state(published.clone());
            let (fr, frl, ime) = (
                child.is_focus_requested(),
                child.is_focus_released(),
                child.take_ime_state(),
            );
            ctx.absorb_child(false, false, false, false, false, fr, frl, ime);
        }
        assert!(ctx.is_focus_requested());
        assert!(!ctx.is_focus_released());
        assert_eq!(ctx.take_ime_state(), Some(published));
    }

    #[test]
    fn claim_hover_records_nothing_unless_the_pass_is_eligible() {
        let mut count = 0u32;
        let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::ZERO);
        // A bare context is ineligible by default — the safe direction: a widget
        // that claims on a pass the root never marked a hover pass records nothing.
        assert!(!ctx.is_hover_eligible());
        ctx.claim_hover();
        assert!(
            !ctx.is_hover_claimed(),
            "an ineligible claim records nothing"
        );

        ctx.set_hover_eligible(true);
        ctx.claim_hover();
        assert!(ctx.is_hover_claimed());
        // A claim never touches the redraw channel: the widget owns change
        // detection (see `claim_hover`'s docs).
        assert!(!ctx.needs_redraw());
    }

    #[test]
    fn child_ctx_seeds_hover_and_absorb_bubbles_a_claim() {
        let mut count = 0u32;
        let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::ZERO);
        ctx.set_hover_epoch(7);
        {
            // A hovered, eligible child: it sees its own link and its claim
            // bubbles into the parent so the whole path records the same stamp.
            let mut child = ctx.child_ctx(Point::ZERO, Size::ZERO, false, true, true);
            assert!(child.is_hovered());
            assert_eq!(child.hover_epoch(), 7, "the live epoch threads down");
            assert_eq!(child.hover_claim_epoch(), 8, "a claim takes the next epoch");
            child.claim_hover();
            let claimed = child.is_hover_claimed();
            ctx.absorb_child(false, false, false, false, claimed, false, false, None);
        }
        assert!(ctx.is_hover_claimed());
    }

    #[test]
    fn content_type_defaults_to_no_hint() {
        assert_eq!(ImeContentType::default(), ImeContentType::Normal);
        assert!(!ImeContentType::default().is_secret());
        assert!(!ImeContentType::default().suppresses_suggestions());
    }

    #[test]
    fn content_type_predicates_classify_every_variant() {
        // `is_secret` gates secure entry; `suppresses_suggestions` gates the
        // suggestion strip + personalized learning. A shell branches on these,
        // never on a `_` arm (the enum is `#[non_exhaustive]`).
        assert!(ImeContentType::Password.is_secret());
        assert!(ImeContentType::Password.suppresses_suggestions());

        assert!(!ImeContentType::NoSuggestions.is_secret());
        assert!(ImeContentType::NoSuggestions.suppresses_suggestions());

        assert!(!ImeContentType::Terminal.is_secret());
        assert!(ImeContentType::Terminal.suppresses_suggestions());

        assert!(!ImeContentType::Normal.is_secret());
        assert!(!ImeContentType::Normal.suppresses_suggestions());
    }

    #[test]
    fn default_ime_state_is_cleared_and_unhinted() {
        let s = ImeState::default();
        assert!(!s.active);
        assert!(s.caret.is_none());
        assert_eq!(s.content_type, ImeContentType::Normal);
        // `−1` sentinels, not the derived zeros: no selection, no composition.
        assert_eq!(
            s.editing,
            EditingState {
                text: String::new(),
                selection_base: -1,
                selection_extent: -1,
                composing_base: -1,
                composing_extent: -1,
            }
        );
    }

    /// The content-type hint must survive core's opaque passthrough untouched —
    /// core never inspects or rewrites it, it only carries it to the shell.
    #[test]
    fn publish_ime_state_round_trips_the_content_type() {
        let mut count = 0u32;
        let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::ZERO);
        let published = ImeState {
            active: true,
            editing: EditingState {
                text: "hunter2".to_string(),
                selection_base: 7,
                selection_extent: 7,
                composing_base: -1,
                composing_extent: -1,
            },
            caret: Some(Rect::new(0.0, 0.0, 1.0, 10.0)),
            content_type: ImeContentType::Password,
            suppress_soft_keyboard: false,
        };
        ctx.publish_ime_state(published.clone());
        let taken = ctx.take_ime_state().expect("published state");
        assert_eq!(taken, published);
        assert_eq!(taken.content_type, ImeContentType::Password);
        // The secret's text is published verbatim — the platform IME mirror
        // needs it (see `ImeState`'s docs); the hint, not redaction, is what
        // tells the shell to lock the keyboard down.
        assert_eq!(taken.editing.text, "hunter2");
    }

    /// …and it survives the focus-chain bubble a real widget publication takes.
    #[test]
    fn content_type_bubbles_up_the_focus_chain() {
        let mut count = 0u32;
        let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::ZERO);
        let published = ImeState {
            content_type: ImeContentType::NoSuggestions,
            ..ImeState::default()
        };
        {
            let mut child = ctx.child_ctx(Point::ZERO, Size::ZERO, true, false, false);
            child.publish_ime_state(published.clone());
            let ime = child.take_ime_state();
            ctx.absorb_child(false, false, false, false, false, false, false, ime);
        }
        assert_eq!(ctx.take_ime_state(), Some(published));
    }

    /// The keyboard-suppression hint is carried, not interpreted: core passes
    /// it through untouched, and it prints plainly (it is a routing hint, not a
    /// secret) so a trace shows why no keyboard came up.
    #[test]
    fn suppress_soft_keyboard_defaults_off_round_trips_and_prints_plainly() {
        assert!(
            !ImeState::default().suppress_soft_keyboard,
            "a publisher that says nothing must behave as it did before the hint existed"
        );
        let mut count = 0u32;
        let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::ZERO);
        let published = ImeState {
            active: true,
            suppress_soft_keyboard: true,
            ..ImeState::default()
        };
        ctx.publish_ime_state(published.clone());
        let taken = ctx.take_ime_state().expect("published state");
        assert_eq!(taken, published);
        assert!(taken.suppress_soft_keyboard);
        assert!(format!("{taken:?}").contains("suppress_soft_keyboard: true"));
    }

    #[test]
    fn debug_redacts_a_secret_field_but_not_a_normal_one() {
        let secret = ImeState {
            active: true,
            editing: EditingState {
                text: "hunter2".to_string(),
                ..EditingState::default()
            },
            caret: None,
            content_type: ImeContentType::Password,
            suppress_soft_keyboard: false,
        };
        let rendered = format!("{secret:?}");
        assert!(
            !rendered.contains("hunter2"),
            "secret text leaked: {rendered}"
        );
        assert!(rendered.contains("<redacted>"));
        assert!(rendered.contains("Password"));

        let plain = ImeState {
            content_type: ImeContentType::Normal,
            ..secret
        };
        assert!(format!("{plain:?}").contains("hunter2"));
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

    #[test]
    fn pending_result_flush_peek_observes_the_mark_without_draining_it() {
        // The peek is what a mobile shell reads while gathering its frame-gate
        // inputs, BEFORE deciding whether the frame runs — so it must be
        // non-destructive: draining stays `RenderRoot::rebuild`'s job on a frame
        // that actually runs. A peek that consumed the mark would leave the
        // rebuild with nothing to flush, which is worse than never peeking.
        //
        // Thread-affine like mark/take, and libtest gives each test its own
        // thread, so this needs no cross-test lock — but drain first anyway so
        // it never inherits a mark from earlier work on this thread.
        let _ = take_pending_result_flush();
        assert!(!has_pending_result_flush(), "starts clear");

        mark_pending_result_flush();
        assert!(has_pending_result_flush(), "the peek observes the mark");
        // Repeated peeks are idempotent — the mark survives every one of them.
        assert!(has_pending_result_flush());
        assert!(has_pending_result_flush());

        // Only the drain clears it, and the drain still reports the mark it took.
        assert!(take_pending_result_flush(), "the drain still sees the mark");
        assert!(
            !has_pending_result_flush(),
            "the drain is what clears it, not the peek"
        );
    }

    #[test]
    fn a_hover_mark_belongs_to_the_root_whose_link_it_names() {
        // Two roots on one thread hold colliding epoch integers by construction
        // (every root's counter starts at 1 and advances per hover pass), so the
        // published link and the mark are both qualified by the root's identity.
        // Simulated here with two ids rather than two `RenderRoot`s: this is the
        // channel's own contract, and the pods on either side of it only ever
        // reach it through these four functions.
        const ROOT_A: u64 = 11;
        const ROOT_B: u64 = 22;
        const EPOCH: u64 = 7;

        set_live_hover_link(ROOT_B, EPOCH);
        assert!(
            live_hover_link_is(ROOT_B, EPOCH),
            "the publishing root's pod recognizes its own live link"
        );
        assert!(
            !live_hover_link_is(ROOT_A, EPOCH),
            "the same epoch integer under another root is not this link"
        );
        assert!(
            !live_hover_link_is(ROOT_B, 0),
            "epoch 0 is 'no link' and matches nothing"
        );

        // A mark raised for one root is not the other's to consume: draining the
        // wrong one must neither report nor clear it.
        mark_hover_orphaned(ROOT_A);
        assert!(
            !take_hover_orphaned(ROOT_B),
            "a root does not end its hover on another root's severance"
        );
        assert!(
            take_hover_orphaned(ROOT_A),
            "and the mark is still standing for the root that owns it"
        );
        assert!(
            !take_hover_orphaned(ROOT_A),
            "the drain is destructive for the matching root"
        );

        // Leave the thread-local at rest for anything else on this thread.
        set_live_hover_link(0, 0);
    }

    #[test]
    fn a_cursor_request_is_one_slot_the_last_writer_owns() {
        // Thread-affine like the two flags above, and libtest gives each test its
        // own thread — but clear first anyway so nothing earlier on this thread
        // leaks in (which is exactly what `RenderRoot::event` does per pass).
        clear_cursor_request();
        assert_eq!(take_cursor_request(), None, "absence means Default");

        let mut count = 0u32;
        let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::ZERO);
        ctx.set_cursor(CursorIcon::Text);
        // A second widget on the same routed path speaks later and therefore wins;
        // there is no per-pod recording to merge, only this one slot.
        {
            let mut child = ctx.child_ctx(Point::ZERO, Size::ZERO, false, false, false);
            child.set_cursor(CursorIcon::Pointer);
        }
        assert_eq!(
            take_cursor_request(),
            Some(CursorIcon::Pointer),
            "the last set_cursor of the pass is the resolved one"
        );
        assert_eq!(
            take_cursor_request(),
            None,
            "the drain is destructive — one request per pass"
        );
    }

    #[test]
    fn a_nested_request_pass_resolves_its_own_and_hands_the_slot_back() {
        // The reentrancy guard on the pass-scoped slot. `RenderRoot::event`
        // forbids re-entering itself, so this shape is not reachable today —
        // which is the point: the failure it would produce (an inner dispatch
        // silently eating the request the outer pass had already collected, or
        // draining one the outer pass was still owed) is invisible, so the
        // bracket enforces the scoping rather than the convention doing it.
        let outer = RequestPass::enter();
        let mut count = 0u32;
        {
            let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::ZERO);
            ctx.set_cursor(CursorIcon::Grab);
        }
        // A nested pass starts from absence, like any other — and draining it
        // ends it, handing the enclosing pass's request straight back.
        assert_eq!(
            RequestPass::enter().take().cursor,
            None,
            "a nested pass starts from absence, like any other"
        );
        {
            let inner = RequestPass::enter();
            let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::ZERO);
            ctx.set_cursor(CursorIcon::Text);
            drop(ctx);
            assert_eq!(
                inner.take().cursor,
                Some(CursorIcon::Text),
                "and resolves exactly what was asked inside it"
            );
        }
        // `take` consumes the guard, so the outermost pass ends here: the slot is
        // cleared rather than restored, and a `set_cursor` made outside any pass
        // (a reconciler's synthesized `Cancel`) still cannot leak into the next.
        assert_eq!(
            outer.take().cursor,
            Some(CursorIcon::Grab),
            "the enclosing pass's request survived the nested one"
        );
        {
            let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::ZERO);
            ctx.set_cursor(CursorIcon::NotAllowed);
        }
        let next = RequestPass::enter();
        assert_eq!(
            next.take().cursor,
            None,
            "a request made between passes belongs to no pass"
        );
    }

    #[test]
    fn the_default_cursor_is_the_platform_arrow() {
        // `Default::default()` is what an absent request resolves to at the root,
        // so the derive must land on the arrow and not on some named shape.
        assert_eq!(CursorIcon::default(), CursorIcon::Default);
    }

    #[test]
    fn a_nested_request_pass_hands_the_clipboard_slots_back_too() {
        // The cursor's reentrancy proof above, for the two channels sharing its
        // bracket: one flag guards all three slots, so a nested pass must return
        // an enclosing pass's *undrained copy* and *unanswered paste request*
        // exactly as it returns its cursor.
        let outer = RequestPass::enter();
        let mut count = 0u32;
        {
            let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::ZERO);
            ctx.write_clipboard("outer".to_string());
            ctx.request_paste();
        }
        {
            let inner = RequestPass::enter();
            let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::ZERO);
            ctx.write_clipboard("inner".to_string());
            drop(ctx);
            let resolved = inner.take();
            assert_eq!(
                resolved.clipboard_write.as_deref(),
                Some("inner"),
                "a nested pass resolves exactly what was written inside it"
            );
            assert!(
                !resolved.paste_request,
                "and starts from absence rather than inheriting the enclosing ask"
            );
        }
        let resolved = outer.take();
        assert_eq!(
            resolved.clipboard_write.as_deref(),
            Some("outer"),
            "the enclosing pass's write survived the nested one"
        );
        assert!(resolved.paste_request, "and so did its paste request");

        // Outside any pass now: a stray write belongs to nobody.
        {
            let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::ZERO);
            ctx.write_clipboard("stray".to_string());
            ctx.request_paste();
        }
        let next = RequestPass::enter().take();
        assert_eq!(next.clipboard_write, None);
        assert!(!next.paste_request);
    }

    #[test]
    fn the_last_clipboard_write_of_a_pass_wins() {
        let pass = RequestPass::enter();
        let mut count = 0u32;
        {
            let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::ZERO);
            ctx.write_clipboard("container".to_string());
            ctx.write_clipboard("leaf".to_string());
        }
        assert_eq!(
            pass.take().clipboard_write.as_deref(),
            Some("leaf"),
            "one write is resolved per pass, and the last caller is it"
        );
    }

    #[test]
    fn a_paste_request_is_idempotent_within_a_pass() {
        let pass = RequestPass::enter();
        let mut count = 0u32;
        {
            let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::ZERO);
            ctx.request_paste();
            ctx.request_paste();
        }
        // Data-free: two asks owe one clipboard read, and the flag cannot
        // represent anything else.
        assert!(pass.take().paste_request);
    }

    #[test]
    fn an_edit_command_is_focus_routed_with_zero_position() {
        let copy = InputEvent::EditCommand(EditCommand::Copy);
        assert!(copy.is_focus_routed(), "a clipboard verb follows the focus");
        assert!(!copy.is_broadcast(), "and is not a broadcast");
        assert_eq!(copy.position(), Point::ZERO);
        assert_eq!(
            copy.translated(Vec2::new(10.0, 20.0)),
            copy,
            "a positionless event is returned unchanged by a container's translate"
        );
    }

    /// An overlay event standing in for one routed into a floated surface.
    fn overlay(kind: OverlayEventKind) -> InputEvent {
        InputEvent::Overlay(OverlayEvent {
            key: OverlayKey::next(),
            kind,
        })
    }

    #[test]
    fn overlay_is_a_broadcast_and_housekeeping_is_the_only_other_one() {
        let routed = overlay(OverlayEventKind::Pointer(PointerEvent {
            phase: PointerPhase::Down,
            position: Point::new(120.0, 80.0),
            button: PointerButton::Primary,
        }));
        assert!(
            routed.is_broadcast(),
            "an overlay event reaches its owner by broadcast, wherever the owner sits"
        );
        assert!(
            !routed.is_focus_routed(),
            "and not down the focus chain — the surface's owner need not be focused"
        );
        assert!(InputEvent::Housekeeping.is_broadcast());

        // ...and nothing else is. Spelled as an exhaustive walk rather than three
        // spot checks, so a variant added later has to state its own answer here.
        for event in [
            InputEvent::Pointer(PointerEvent {
                phase: PointerPhase::Down,
                position: Point::ZERO,
                button: PointerButton::Primary,
            }),
            InputEvent::Scroll {
                position: Point::ZERO,
                delta: ScrollDelta::Lines(0.0, 1.0),
            },
            InputEvent::Key(KeyEvent {
                key: Key::Named(NamedKey::Enter),
                modifiers: Modifiers::default(),
                repeat: false,
            }),
            InputEvent::Ime(ImeEvent::Enabled),
            InputEvent::EditCommand(EditCommand::Copy),
        ] {
            assert!(
                !event.is_broadcast(),
                "only Housekeeping and Overlay broadcast, but {event:?} claims to"
            );
        }
    }

    #[test]
    fn an_overlay_event_carries_no_local_position_and_is_never_translated() {
        let routed = overlay(OverlayEventKind::Pointer(PointerEvent {
            phase: PointerPhase::Move,
            position: Point::new(120.0, 80.0),
            button: PointerButton::Primary,
        }));
        assert_eq!(
            routed.position(),
            Point::ZERO,
            "a broadcast is never hit-tested, so it reports no position to hit-test on"
        );
        // The payload's own position is WINDOW space, and the container chain the
        // broadcast travels describes where the *owner* sits — not where the
        // floated surface does — so translating it would corrupt it.
        assert_eq!(
            routed.translated(Vec2::new(-10.0, -20.0)),
            routed,
            "the container chain must not shift a window-space payload"
        );
        let scrolled = overlay(OverlayEventKind::Scroll {
            position: Point::new(120.0, 80.0),
            delta: ScrollDelta::Pixels(0.0, 12.0),
        });
        assert_eq!(scrolled.translated(Vec2::new(5.0, 5.0)), scrolled);
        let outside = overlay(OverlayEventKind::OutsideDown);
        assert_eq!(outside.position(), Point::ZERO);
        assert_eq!(outside.translated(Vec2::new(5.0, 5.0)), outside);
    }

    #[test]
    fn edit_commands_drain_in_dispatch_order_within_one_pass() {
        let pass = RequestPass::enter();
        let mut count = 0u32;
        let taken = {
            let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::ZERO);
            // The toolbar's "cut" answered as two verbs: order is meaning.
            ctx.dispatch_edit_command(EditCommand::Copy);
            ctx.dispatch_edit_command(EditCommand::SelectAll);
            ctx.dispatch_edit_command(EditCommand::Cut);
            ctx.take_edit_commands()
        };
        assert_eq!(
            taken,
            vec![EditCommand::Copy, EditCommand::SelectAll, EditCommand::Cut],
            "a FIFO, not a last-writer-wins slot"
        );

        // The drain empties the queue, so a second owner forwarding in the same
        // pass cannot re-apply the first owner's verbs.
        let mut second = 0u32;
        let mut ctx = EventCtx::new(&mut second, Point::ZERO, Size::ZERO);
        assert!(ctx.take_edit_commands().is_empty());
        drop(ctx);
        drop(pass.take());
    }

    #[test]
    fn a_leaked_edit_command_is_cleared_with_the_pass_and_never_reaches_the_next() {
        {
            let pass = RequestPass::enter();
            let mut count = 0u32;
            let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::ZERO);
            // Dispatched, and nobody drains it: a wiring bug in the dispatching
            // widget. The pass resolving is what reports (debug builds) and clears
            // it — a `Cut` surviving into a later pass would apply to whatever is
            // selected by then, which is how text gets destroyed silently.
            ctx.dispatch_edit_command(EditCommand::Cut);
            drop(ctx);
            drop(pass.take());
        }

        let next = RequestPass::enter();
        let mut count = 0u32;
        let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::ZERO);
        assert!(
            ctx.take_edit_commands().is_empty(),
            "a leaked command must not survive into the next pass"
        );
        drop(ctx);
        drop(next.take());
    }

    #[test]
    fn a_nested_pass_hands_the_enclosing_passs_edit_commands_back() {
        let outer = RequestPass::enter();
        let mut count = 0u32;
        {
            let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::ZERO);
            ctx.dispatch_edit_command(EditCommand::Copy);
        }
        {
            // A nested dispatch (the overlay pre-pass re-entering `RenderRoot::event`
            // is the shipped case) starts from an empty queue and must not eat the
            // enclosing pass's undrained command.
            let inner = RequestPass::enter();
            let mut inner_state = 0u32;
            let mut ctx = EventCtx::new(&mut inner_state, Point::ZERO, Size::ZERO);
            assert!(ctx.take_edit_commands().is_empty());
            ctx.dispatch_edit_command(EditCommand::SelectAll);
            assert_eq!(ctx.take_edit_commands(), vec![EditCommand::SelectAll]);
            drop(ctx);
            drop(inner.take());
        }
        let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::ZERO);
        assert_eq!(
            ctx.take_edit_commands(),
            vec![EditCommand::Copy],
            "the enclosing pass's queue is handed back intact"
        );
        drop(ctx);
        drop(outer.take());
    }

    #[test]
    fn debug_redacts_a_pasted_payload() {
        // The clipboard has no content-type hint to key a decision off (see the
        // `Debug` impl), so the payload is redacted unconditionally.
        let rendered = format!("{:?}", EditCommand::Paste("hunter2".to_string()));
        assert!(
            !rendered.contains("hunter2"),
            "paste payload leaked: {rendered}"
        );
        assert!(rendered.contains("<redacted>"));
        assert!(
            rendered.contains("Paste"),
            "the verb still prints: {rendered}"
        );
        // No length either — that leaks too.
        assert!(!rendered.contains('7'));
        assert_eq!(format!("{:?}", EditCommand::SelectAll), "SelectAll");
    }

    #[test]
    fn a_pointer_contact_is_positioned_and_translated_like_a_pointer() {
        let contact = InputEvent::PointerContact {
            pointer_id: PointerId::touch(2),
            event: PointerEvent {
                phase: PointerPhase::Move,
                position: Point::new(30.0, 40.0),
                button: PointerButton::Primary,
            },
        };
        assert_eq!(contact.position(), Point::new(30.0, 40.0));
        assert_eq!(
            contact.translated(Vec2::new(-10.0, -20.0)),
            InputEvent::PointerContact {
                pointer_id: PointerId::touch(2),
                event: PointerEvent {
                    phase: PointerPhase::Move,
                    position: Point::new(20.0, 20.0),
                    button: PointerButton::Primary,
                },
            },
            "the position shifts; the id and the rest of the event do not"
        );
        // Hit-tested, like the pointer it wraps: neither class of non-positional
        // event.
        assert!(!contact.is_focus_routed());
        assert!(!contact.is_broadcast());
    }

    #[test]
    fn pointer_ids_name_the_mouse_and_touch_slots() {
        assert_eq!(
            PointerId::MOUSE,
            PointerId {
                source: PointerSource::Mouse,
                slot: 0
            }
        );
        assert_eq!(
            PointerId::touch(3),
            PointerId {
                source: PointerSource::Touch,
                slot: 3
            }
        );
        assert_ne!(PointerId::MOUSE, PointerId::touch(0));
    }

    #[test]
    fn a_context_reports_the_mouse_outside_any_contact_pass() {
        let mut count = 0u32;
        let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::ZERO);
        assert_eq!(ctx.pointer_id(), PointerId::MOUSE);
        // An opt-in with no pass open is still visible to the container that
        // reads it, but records nothing a later pass could inherit.
        ctx.capture_contacts();
        assert!(ctx.is_contact_capture_requested());
        drop(ctx);
        let pass = ContactPass::enter(PointerId::touch(0), false);
        assert!(!pass.contacts_requested());
    }

    #[test]
    fn a_contact_pass_seeds_every_context_and_collects_the_opt_in() {
        let mut count = 0u32;
        let outer = ContactPass::enter(PointerId::touch(1), true);
        assert!(in_secondary_contact_pass());
        {
            // A fresh context (what a component builds over its own state)
            // reports the pass's contact, and so does every child context.
            let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::ZERO);
            assert_eq!(ctx.pointer_id(), PointerId::touch(1));
            let (contacts, id) = {
                let mut child = ctx.child_ctx(Point::ZERO, Size::ZERO, false, false, false);
                let id = child.pointer_id();
                child.capture_contacts();
                (child.is_contact_capture_requested(), id)
            };
            assert_eq!(id, PointerId::touch(1));
            ctx.absorb_child(false, false, contacts, false, false, false, false, None);
            assert!(ctx.is_contact_capture_requested(), "the opt-in bubbles");
        }
        assert!(outer.contacts_requested(), "and is recorded in the pass");

        // A nested pass scopes its own contact and opt-in, then hands back.
        {
            let inner = ContactPass::enter(PointerId::MOUSE, false);
            assert_eq!(current_pointer_id(), PointerId::MOUSE);
            assert!(!in_secondary_contact_pass());
            assert!(!inner.contacts_requested());
        }
        assert_eq!(current_pointer_id(), PointerId::touch(1));
        assert!(in_secondary_contact_pass());
        assert!(outer.contacts_requested());
        drop(outer);
        assert_eq!(current_pointer_id(), PointerId::MOUSE);
        assert!(!in_secondary_contact_pass());
    }
}
