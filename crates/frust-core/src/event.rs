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

use std::any::Any;
use std::cell::Cell;
use std::fmt;

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
/// Pointer gestures and scroll are **hit-tested** (routed by position); keyboard
/// and IME events are **focus-routed** — delivered straight down the recorded
/// focus chain with no hit test and no meaningful position (see
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
    /// seconds away or may never reach that widget at all (FINDINGS #56: a
    /// pop-result callback measured 3.2s late on device, and was lost entirely
    /// when the next tap was consumed by chrome outside the navigator).
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
    /// Focus-routed events ([`InputEvent::Key`]/[`InputEvent::Ime`]) and the
    /// [`Housekeeping`](InputEvent::Housekeeping) broadcast have no spatial
    /// position — they are delivered down the focus chain, or to every child, not
    /// hit-tested — so this reports [`Point::ZERO`] for them; callers must never
    /// hit-test on it (routing helpers early-return both classes).
    pub fn position(&self) -> Point {
        match self {
            InputEvent::Pointer(p) => p.position,
            InputEvent::Scroll { position, .. } => *position,
            InputEvent::Key(_) | InputEvent::Ime(_) | InputEvent::Housekeeping => Point::ZERO,
        }
    }

    /// Return a copy of this event with its position shifted by `offset`.
    ///
    /// Containers use this (with `offset = -child_origin`) to translate an event
    /// from their own coordinate space into a child's local space before
    /// forwarding it — see [`crate::widget::ChildPod::event_child`]. Focus-routed
    /// events ([`InputEvent::Key`]/[`InputEvent::Ime`]) and the
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
            InputEvent::Key(_) | InputEvent::Ime(_) | InputEvent::Housekeeping => self.clone(),
        }
    }

    /// Whether this event is focus-routed (delivered down the focus chain with no
    /// hit test) rather than hit-tested by position.
    ///
    /// [`Housekeeping`](InputEvent::Housekeeping) is **not** focus-routed — it
    /// reaches every child, focused or not; see
    /// [`is_broadcast`](InputEvent::is_broadcast).
    pub fn is_focus_routed(&self) -> bool {
        matches!(self, InputEvent::Key(_) | InputEvent::Ime(_))
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
    /// FINDINGS #56 failure. UI-thread affinity is the same argument
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
/// | [`Password`](Self::Password) | `TYPE_CLASS_TEXT \| TYPE_TEXT_VARIATION_PASSWORD`, plus `TYPE_TEXT_FLAG_NO_SUGGESTIONS` and `IME_FLAG_NO_PERSONALIZED_LEARNING` | `isSecureTextEntry = true`, `textContentType = .password`, `autocorrectionType = .no`, `spellCheckingType = .no` | `ImePurpose::Password` |
/// | [`NoSuggestions`](Self::NoSuggestions) | `TYPE_CLASS_TEXT \| TYPE_TEXT_FLAG_NO_SUGGESTIONS`, plus `IME_FLAG_NO_PERSONALIZED_LEARNING` | `autocorrectionType = .no`, `spellCheckingType = .no` | no equivalent — `ImePurpose::Normal` |
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
            Self::Normal | Self::NoSuggestions => false,
        }
    }

    /// Whether the platform must suppress its suggestion strip, autocorrect,
    /// and persistent word learning for this field.
    ///
    /// True for every secret content type and for
    /// [`NoSuggestions`](Self::NoSuggestions).
    pub fn suppresses_suggestions(self) -> bool {
        match self {
            Self::Password | Self::NoSuggestions => true,
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
            content_type: ImeContentType::Normal,
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
            let mut child = ctx.child_ctx(Point::ZERO, Size::ZERO, true);
            child.publish_ime_state(published.clone());
            let ime = child.take_ime_state();
            ctx.absorb_child(false, false, false, false, ime);
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
}
