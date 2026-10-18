//! Keyboard, clipboard-command and editing-state vocabulary for focus-routed input.

use std::fmt;

/// A non-character key the focus path and the shortcut registry route; typed text arrives as
/// [`Key::Character`]. `Copy`, `Cut` and `Paste` are the hardware keys a shell maps straight onto
/// [`EditCommand`]; `Insert` exists for the `Shift+Insert`/`Ctrl+Insert` clipboard chords;
/// `F1`..`F24` are the function keys, which no widget consumes by default.
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
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    F13,
    F14,
    F15,
    F16,
    F17,
    F18,
    F19,
    F20,
    F21,
    F22,
    F23,
    F24,
}

impl NamedKey {
    /// The function-key number (`1..=24`) of `F1`..`F24`, `None` for every other key.
    pub fn function_number(self) -> Option<u8> {
        match self {
            NamedKey::F1 => Some(1),
            NamedKey::F2 => Some(2),
            NamedKey::F3 => Some(3),
            NamedKey::F4 => Some(4),
            NamedKey::F5 => Some(5),
            NamedKey::F6 => Some(6),
            NamedKey::F7 => Some(7),
            NamedKey::F8 => Some(8),
            NamedKey::F9 => Some(9),
            NamedKey::F10 => Some(10),
            NamedKey::F11 => Some(11),
            NamedKey::F12 => Some(12),
            NamedKey::F13 => Some(13),
            NamedKey::F14 => Some(14),
            NamedKey::F15 => Some(15),
            NamedKey::F16 => Some(16),
            NamedKey::F17 => Some(17),
            NamedKey::F18 => Some(18),
            NamedKey::F19 => Some(19),
            NamedKey::F20 => Some(20),
            NamedKey::F21 => Some(21),
            NamedKey::F22 => Some(22),
            NamedKey::F23 => Some(23),
            NamedKey::F24 => Some(24),
            _ => None,
        }
    }

    /// The `F<n>` key for `n` in `1..=24`.
    pub fn function(n: u8) -> Option<NamedKey> {
        match n {
            1 => Some(NamedKey::F1),
            2 => Some(NamedKey::F2),
            3 => Some(NamedKey::F3),
            4 => Some(NamedKey::F4),
            5 => Some(NamedKey::F5),
            6 => Some(NamedKey::F6),
            7 => Some(NamedKey::F7),
            8 => Some(NamedKey::F8),
            9 => Some(NamedKey::F9),
            10 => Some(NamedKey::F10),
            11 => Some(NamedKey::F11),
            12 => Some(NamedKey::F12),
            13 => Some(NamedKey::F13),
            14 => Some(NamedKey::F14),
            15 => Some(NamedKey::F15),
            16 => Some(NamedKey::F16),
            17 => Some(NamedKey::F17),
            18 => Some(NamedKey::F18),
            19 => Some(NamedKey::F19),
            20 => Some(NamedKey::F20),
            21 => Some(NamedKey::F21),
            22 => Some(NamedKey::F22),
            23 => Some(NamedKey::F23),
            24 => Some(NamedKey::F24),
            _ => None,
        }
    }
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
