//! Keyboard, clipboard-command and editing-state vocabulary for focus-routed input.

use std::fmt;

/// A non-character key with editing semantics; typed text arrives as [`Key::Character`]. `Copy`,
/// `Cut` and `Paste` are the hardware keys a shell maps straight onto [`EditCommand`]; `Insert`
/// exists for the `Shift+Insert`/`Ctrl+Insert` clipboard chords.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NamedKey {
    Enter,
    Backspace,
    Delete,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    ArrowDown,
    Home,
    End,
    Escape,
    Tab,
    Copy,
    Cut,
    Paste,
    Insert,
}

/// A logical key: a [`NamedKey`], or the resolved text a key produced (dead keys and IME already
/// applied), which a widget inserts verbatim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Key {
    Named(NamedKey),
    Character(String),
}

/// The modifier chord held when a [`KeyEvent`] fired; `meta` is Command on macOS, Super elsewhere.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub meta: bool,
}

/// A keyboard key event, delivered down the focus path (never hit-tested).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyEvent {
    pub key: Key,
    pub modifiers: Modifiers,
    /// Whether this is an auto-repeat (key held down), not a fresh press.
    pub repeat: bool,
}

/// A decoded clipboard/selection verb for the focused editable, so no widget decodes OS chords.
/// `Copy`/`Cut` carry nothing (the widget answers with `EventCtx::write_clipboard`); `Paste`
/// carries the text the shell already read. A widget may refuse any verb. Exhaustive on purpose:
/// a fifth verb must break every `match`.
#[derive(Clone, PartialEq, Eq)]
pub enum EditCommand {
    /// Copy the selection to the host clipboard, leaving the document unchanged.
    Copy,
    /// Copy the selection, then delete it.
    Cut,
    /// Replace the selection (or insert at the caret) with this text.
    Paste(String),
    /// Select the widget's entire content.
    SelectAll,
}

impl fmt::Debug for EditCommand {
    /// Hand-written so pasted text never reaches a log: the payload prints `<redacted>`, no length.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EditCommand::Copy => f.write_str("Copy"),
            EditCommand::Cut => f.write_str("Cut"),
            EditCommand::Paste(_) => f.debug_tuple("Paste").field(&"<redacted>").finish(),
            EditCommand::SelectAll => f.write_str("SelectAll"),
        }
    }
}

/// The full editing state of a text field — the value every IME bridge syncs (`-1` = none).
/// Index boundary rule: at the `AppTree`/shell seam the indices count UTF-16 code units; widgets
/// and `frust-text` convert to byte offsets, and core interprets none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditingState {
    pub text: String,
    pub selection_base: i32,
    pub selection_extent: i32,
    pub composing_base: i32,
    pub composing_extent: i32,
}

impl Default for EditingState {
    /// An empty field: the anchors are the `-1` "none" sentinel, not the derived `0`.
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
