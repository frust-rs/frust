//! IME vocabulary: composition events, the content-type hint, and the published IME surface.

use std::fmt;

use kurbo::Rect;

use super::key::EditingState;

/// An IME event, delivered down the focus path. Desktop sends `Compose`/`Commit`, mobile bridges
/// sync whole values (`ApplyEditingState`), and `Enabled`/`Disabled` bracket a session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImeEvent {
    /// Preedit (marked) text; `cursor` is a byte-indexed selection inside it, as winit reports.
    Compose {
        text: String,
        cursor: Option<(usize, usize)>,
    },
    /// Insert the finished composition and clear the composing region.
    Commit(String),
    /// Replace the whole editing state (mobile state-sync path).
    ApplyEditingState(EditingState),
    Enabled,
    Disabled,
}

/// What a focused editable holds — the hint a shell maps onto its platform IME. Masking alone
/// does not stop a soft keyboard showing or learning the text; only this hint does. Shells branch
/// security on [`is_secret`](Self::is_secret), never on a `_ =>` arm (the enum is non-exhaustive).
///
/// # Platform mapping
/// | Variant | Android (`InputType` / `EditorInfo.imeOptions`) | iOS (`UITextInputTraits`) | Desktop (winit) |
/// |---|---|---|---|
/// | [`Normal`](Self::Normal) | `TYPE_CLASS_TEXT` | platform defaults | `ImePurpose::Normal` |
/// | [`Password`](Self::Password) | `TYPE_CLASS_TEXT \| TYPE_TEXT_VARIATION_PASSWORD`, plus `TYPE_TEXT_FLAG_NO_SUGGESTIONS` and `IME_FLAG_NO_PERSONALIZED_LEARNING` | `isSecureTextEntry = true`, `textContentType = .password`, `autocorrectionType = .no`, `spellCheckingType = .no`, plus smart-punctuation suppression (see [`Terminal`](Self::Terminal)) | `ImePurpose::Password` |
/// | [`NoSuggestions`](Self::NoSuggestions) | `TYPE_CLASS_TEXT \| TYPE_TEXT_FLAG_NO_SUGGESTIONS`, plus `IME_FLAG_NO_PERSONALIZED_LEARNING` | `autocorrectionType = .no`, `spellCheckingType = .no`, plus smart-punctuation suppression | no equivalent — `ImePurpose::Normal` |
/// | [`Terminal`](Self::Terminal) | `TYPE_CLASS_TEXT \| TYPE_TEXT_FLAG_NO_SUGGESTIONS`, plus `IME_FLAG_NO_PERSONALIZED_LEARNING` (same as `NoSuggestions`) | `isSecureTextEntry = false`, `autocorrectionType = .no`, `spellCheckingType = .no`, `smartQuotesType = .no`, `smartDashesType = .no`, `smartInsertDeleteType = .no`, `autocapitalizationType = .none`, `textContentType = nil` | `ImePurpose::Terminal` |
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ImeContentType {
    /// No hint: platform defaults. What every field publishes unless it opts in.
    #[default]
    Normal,
    /// Secret text: secure entry, with suggestions and personalized learning suppressed.
    Password,
    /// Non-secret text never suggested, autocorrected or learned; no secure entry.
    NoSuggestions,
    /// Byte-exact entry: `NoSuggestions` plus no smart punctuation or autocapitalization; unmasked.
    Terminal,
}

impl ImeContentType {
    /// Whether the platform must treat the field as secret (secure entry). Both predicates match
    /// exhaustively, so a new variant fails to compile until it is classified.
    pub fn is_secret(self) -> bool {
        match self {
            Self::Password => true,
            Self::Normal | Self::NoSuggestions | Self::Terminal => false,
        }
    }

    /// Whether the platform must suppress suggestions, autocorrect and word learning.
    pub fn suppresses_suggestions(self) -> bool {
        match self {
            Self::Password | Self::NoSuggestions | Self::Terminal => true,
            Self::Normal => false,
        }
    }
}

/// The IME surface a focused editable publishes through `EventCtx::publish_ime_state`; the shell
/// reads it from [`crate::app::RenderRoot::ime_state`]. `editing` is plaintext even for a secret
/// field (the platform mirror needs it); [`fmt::Debug`] redacts it then.
#[derive(Clone, PartialEq)]
pub struct ImeState {
    /// Whether the focused widget wants IME active.
    pub active: bool,
    pub editing: EditingState,
    /// The caret rectangle in logical coordinates, for candidate placement.
    pub caret: Option<Rect>,
    pub content_type: ImeContentType,
    /// Keep the input surface wired (copy still needs it) without raising an on-screen keyboard.
    pub suppress_soft_keyboard: bool,
}

impl Default for ImeState {
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
