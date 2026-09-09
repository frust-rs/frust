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
//! on window-leave).
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

/// An input event delivered to the widget tree.
///
/// Pointer gestures and scroll are **hit-tested** (routed by position); keyboard,
/// IME, and edit-command events are **focus-routed** — delivered straight down
/// the recorded focus chain with no hit test and no meaningful position (see
/// [`crate::widget::ChildPod`]'s focus bookkeeping and `frust-widgets`'
/// `route_event`). [`InputEvent::Housekeeping`] is neither: it is a **broadcast**
/// that reaches every child unconditionally.
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
}

impl InputEvent {
    /// The event's location, in the receiving widget's local coordinate space.
    ///
    /// Focus-routed events ([`InputEvent::Key`]/[`InputEvent::Ime`]/
    /// [`InputEvent::EditCommand`]) and the
    /// [`Housekeeping`](InputEvent::Housekeeping) broadcast have no spatial
    /// position — they are delivered down the focus chain, or to every child, not
    /// hit-tested — so this reports [`Point::ZERO`] for them; callers must never
    /// hit-test on it (routing helpers early-return both classes).
    pub fn position(&self) -> Point {
        match self {
            InputEvent::Pointer(p) => p.position,
            InputEvent::Scroll { position, .. } => *position,
            InputEvent::Key(_)
            | InputEvent::Ime(_)
            | InputEvent::EditCommand(_)
            | InputEvent::Housekeeping => Point::ZERO,
        }
    }

    /// Return a copy of this event with its position shifted by `offset`.
    ///
    /// Containers use this (with `offset = -child_origin`) to translate an event
    /// from their own coordinate space into a child's local space before
    /// forwarding it — see [`crate::widget::ChildPod::event_child`]. Focus-routed
    /// events ([`InputEvent::Key`]/[`InputEvent::Ime`]/
    /// [`InputEvent::EditCommand`]) and the
    /// [`Housekeeping`](InputEvent::Housekeeping) broadcast carry no position, so
    /// they are returned unchanged (cloned).
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
            InputEvent::Key(_)
            | InputEvent::Ime(_)
            | InputEvent::EditCommand(_)
            | InputEvent::Housekeeping => self.clone(),
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
    /// routing — today exactly [`InputEvent::Housekeeping`].
    ///
    /// Every routing helper branches on this **first**, before its capture,
    /// focus, and hit-test branches (`frust-widgets`'
    /// `route_event`/`route_event_single`, and this crate's own
    /// [`crate::component`] mirror), so a broadcast can never be swallowed by a
    /// captured child or a `contains()` miss.
    pub fn is_broadcast(&self) -> bool {
        matches!(self, InputEvent::Housekeeping)
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
struct PassSlot<T: Default + 'static> {
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
    fn enter(slot: &'static LocalKey<Cell<T>>, nested: bool) -> Self {
        let stashed = slot.with(|cell| cell.take());
        Self {
            slot,
            restore: if nested { stashed } else { T::default() },
        }
    }

    /// Take what *this* pass recorded in this slot, leaving it empty.
    fn take(&self) -> T {
        self.slot.with(|cell| cell.take())
    }
}

impl<T: Default + 'static> Drop for PassSlot<T> {
    fn drop(&mut self) {
        let restore = std::mem::take(&mut self.restore);
        self.slot.with(|cell| cell.set(restore));
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
    /// widget. The enclosing container reads [`EventCtx::is_pointer_captured`]
    /// after the dispatch returns to record the active child.
    ///
    /// **Capture is a `Down`-time concept here.** Only the `Down` arm of
    /// [`RenderRoot::event`](crate::app::RenderRoot::event) folds a request into
    /// the root's own capture mirror, so a capture opened from a `Move` records
    /// the pod's active path (routing works) while the root still reads
    /// uncaptured. For hover that means a `Move` that both captures and
    /// [`claim_hover`](EventCtx::claim_hover)s records the claim — the pod's
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
    /// the time it lands the pass that asked is long over. Focus routing makes
    /// the late delivery safe — if focus moved or was released in between, the
    /// synthesized [`EditCommand::Paste`] reaches no widget and is dropped, so a
    /// shell may answer unconditionally without checking who asked.
    ///
    /// A `Cut` may write and ask in the same pass; the two slots are independent.
    pub fn request_paste(&mut self) {
        PASTE_REQUEST.with(|slot| slot.set(true));
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
    /// [`crate::widget::ChildPod::event_child`]). The live hover epoch and the
    /// running root's identity are threaded down unchanged (a claim anywhere in
    /// the subtree is stamped with both). The parent folds the results back in with
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

    /// Fold a child dispatch's redraw/capture/hover-claim/focus flags (and any
    /// published IME surface) back into this context.
    pub(crate) fn absorb_child(
        &mut self,
        child_needs_redraw: bool,
        child_captured: bool,
        child_hover_claimed: bool,
        child_focus_requested: bool,
        child_focus_released: bool,
        child_ime_state: Option<ImeState>,
    ) {
        self.needs_redraw |= child_needs_redraw;
        self.capture_requested |= child_captured;
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
            ctx.absorb_child(redraw, cap, false, false, false, None);
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
            ctx.absorb_child(false, false, false, fr, frl, ime);
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
            ctx.absorb_child(false, false, claimed, false, false, None);
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
            ctx.absorb_child(false, false, false, false, false, ime);
        }
        assert_eq!(ctx.take_ime_state(), Some(published));
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
}
