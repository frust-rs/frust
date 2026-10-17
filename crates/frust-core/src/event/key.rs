use std::fmt;

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
/// dispatches it as [`InputEvent::EditCommand`](crate::event::InputEvent::EditCommand), so no widget has to know which
/// chord means copy on which OS. Every widget sees the same four verbs.
///
/// # Why paste carries its text and copy does not
///
/// The clipboard itself lives in the shell (only the shell has a host clipboard
/// to talk to), and the two directions are deliberately asymmetric:
///
/// * [`Copy`](EditCommand::Copy) / [`Cut`](EditCommand::Cut) carry nothing —
///   the widget owns the selection, so it answers by writing its own text into
///   the pass's clipboard slot ([`EventCtx::write_clipboard`](crate::event::EventCtx::write_clipboard)), which the shell
///   drains and hands to the host.
/// * [`Paste`](EditCommand::Paste) carries the text — the *shell* owns the
///   host clipboard, so by the time the command reaches the tree the read has
///   already happened. A widget that wants a paste it did not receive asks for
///   one ([`EventCtx::request_paste`](crate::event::EventCtx::request_paste)) and the shell answers with this variant.
///
/// # Refusal is the widget's call
///
/// Nothing here is a permission: a read-only or secret field is free to ignore
/// a [`Copy`](EditCommand::Copy)/[`Cut`](EditCommand::Cut) it does not want to
/// honour, and a widget with no selection simply reports
/// [`EventResult::Ignored`](crate::event::EventResult::Ignored). The vocabulary states what was *asked for*.
///
/// Exhaustive on purpose (no `#[non_exhaustive]`): these four verbs are the
/// whole clipboard contract, and a widget matching on them should be told by
/// the compiler if that ever stops being true.
#[derive(Clone, PartialEq, Eq)]
pub enum EditCommand {
    /// Copy the current selection to the host clipboard, leaving the document
    /// unchanged. A widget answers by calling [`EventCtx::write_clipboard`](crate::event::EventCtx::write_clipboard).
    Copy,
    /// Copy the current selection and delete it. A widget answers by calling
    /// [`EventCtx::write_clipboard`](crate::event::EventCtx::write_clipboard) *and* mutating its own text — the shell
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
    /// [`ImeState`](crate::event::ImeState)'s reason, one step earlier in the pipeline (see its `Debug`):
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
