use std::fmt;

use kurbo::Rect;

use super::key::EditingState;

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
/// Written by the focused widget through [`EventCtx::publish_ime_state`](crate::event::EventCtx::publish_ime_state), it
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
