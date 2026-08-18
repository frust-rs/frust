//! The `TextInput` interactive widget: an editable text field, single-line by
//! default and optionally wrapped multi-line.
//!
//! [`text_input`] produces a [`TextInputView`] carrying the current `value`, a
//! `placeholder`, an `on_change` callback, and an optional `on_submit`. Like the
//! other interactive widgets it is a **controlled component**
//! (`docs/CODE_STANDARDS.md`'s Interaction Semantics): it never owns the durable
//! value. Each edit reports the *requested* text through `on_change`, and the
//! next `rebuild` reconciles the app-confirmed `value` back into the underlying
//! [`TextEditor`] — set-if-different, preserving the selection while the text is
//! unchanged.
//!
//! [`TextInputView::text_style`] sets the content text's style (family/weight/
//! style/size/letter-spacing/line-height, plus color when set explicitly) and
//! never touches the chrome colors or geometry (see *Chrome*). Mirroring
//! [`Text`](crate::TextView)'s [`effective_style`](TextInputWidget::effective_style),
//! a field that did not call `.text_style(...)` gets `on_surface` resolved at LAYOUT
//! time — where `TextInput`, like `Text`, bakes color into the shaped editor
//! state — falling back to black with no theme threaded: explicit > theme >
//! black. Baked-at-layout resolution is safe only under the `set_theme` →
//! `ChangeFlags::LAYOUT` contract (`docs/CODE_STANDARDS.md` Theming), which
//! `RenderRoot::set_theme` guarantees.
//!
//! # Text context ownership
//!
//! Unlike the [`Text`](crate::TextView) leaf (which shapes against the shared
//! `TextContext` threaded through `LayoutCtx`), a `TextInput` must apply edits
//! *synchronously during the event pass*, where no context is threaded, so it owns
//! its own [`TextContext`]: `on_change` and the published [`ImeState`] observe the
//! fresh editing value at once, and layout needs no threaded one.
//!
//! **App fonts still reach it.** `TextContext::new` seeds from `frust-text`'s
//! process-wide app-font record, where every `frust::register_app_fonts` payload
//! lands when the shell drains it. A font registered *later* is picked up by
//! `TextContext::sync_app_fonts` at the top of [`Widget::layout`], which rebuilds
//! the editor so its retained parley layout re-shapes too — the shape cache
//! alone would not cover it.
//!
//! # Focus, IME and blink
//!
//! A `Down` inside the field requests focus, places the caret, and publishes an
//! [`ImeState`] so the shell can drive the platform input method — the focus/IME
//! channel `docs/CORE_ARCHITECTURE.md`'s Focus/IME Lifecycle owns. Keyboard
//! editing (`Key`) and IME composition/state-sync (`Ime`) route down the focus
//! path; after any edit the widget fires `on_change`, resets the caret to
//! visible, and republishes the IME surface.
//!
//! The caret blinks while focused, its phase measured in `paint` from the shell
//! frame clock ([`PaintCtx::frame_time`] — no wall-clock reads in widget code).
//! Its continuation frame is a paced (`CosmeticLoop`) request at the blink's own
//! [`BLINK_MS`] half-period ([`PaintCtx::request_frame_paced_at`]) — its own
//! slower cadence rather than the theme's cap rate, one paint per visibility
//! toggle being all a 500ms blink needs. A concurrent cap-rate request repaints it
//! more often through the MIN-lattice with no visible effect, safe at any cadence
//! because the phase is `frame_time - blink_epoch`
//! ([`caret_visible_at`](TextInputWidget::caret_visible_at)) — a pure function of
//! this frame's own timestamp, not of a delta between painted frames. An
//! edit/focus during the (clockless) event pass flags the blink for reset, the
//! next paint recording the epoch from `frame_time`.
//!
//! `reduce_motion` freezes the caret **visible** (lit, not hidden) rather than
//! mid-blink, and stops requesting blink frames while focused: unlike a purely
//! decorative loop the caret is also the edit-point cue, so freezing it dark
//! would hide where typing lands. The IME surface republishes every painted frame
//! regardless, and blinking resumes once the token clears.
//!
//! # Multi-line mode
//!
//! [`TextInputView::multiline(max_visible_lines)`](TextInputView::multiline)
//! feeds the layout width from the incoming constraints to the editor as a
//! soft-wrap width (`TextEditor::set_wrap_width`), so the field grows vertically
//! one line at a time as content wraps or newlines are inserted, capped at
//! `max_visible_lines`. Past the cap the box height freezes and the text scrolls
//! vertically by a **keep-caret-in-view** paint offset recomputed statelessly
//! each pass from the caret's line rect (momentum and a scrollbar are out of
//! scope), a rectangular clip keeping the overflow inside.
//!
//! Enter is governed by [`submit_on_enter`](TextInputView::submit_on_enter):
//! `true` (the single-line default) fires `on_submit`, `false` (the multi-line
//! default) inserts a literal newline, and **Shift+Enter always does the opposite
//! of the mode's default** (a single-line field ignores the knob and always
//! submits). On the mobile IME path a Return arrives as a `Commit` of a newline,
//! which submit-on-enter fields submit and a newline-inserting one inserts.
//!
//! # Disabled mode
//!
//! [`TextInputView::enabled(false)`](TextInputView::enabled) makes the field
//! inert and dims it. Inertness hangs off **one** hook, the focus gate: a `Down`
//! in a disabled field neither requests focus nor captures the pointer, and since
//! `Key`/`Ime` reach a widget only along the recorded focus path, refusing focus
//! makes keyboard and IME editing impossible with no per-handler guard. A field
//! disabled *while* focused releases the focus path on the first event reaching
//! it and stops behaving as focused (no caret, no blink frame, no active IME
//! surface) from the very next paint.
//!
//! Dimming multiplies the *resolved* role color's alpha rather than swapping in a
//! dedicated "disabled" token, at **both** resolution points (the layout-baked
//! glyph color in [`effective_style`](TextInputWidget::effective_style) and the
//! paint-time [`Chrome`]), so it behaves identically under every catalog and the
//! unthemed fallbacks: `on_surface_variant` is opaque under Material/Glyph but
//! translucent under Cupertino (`frust-theme`'s `ColorScheme` Cupertino arm,
//! `secondaryLabel` at alpha 153), so it is *not* a portable disabled token, and
//! it is keyed **strictly** off [`TextInputView::enabled`] at both points.
//!
//! # Read-only mode
//!
//! [`TextInputView::read_only(true)`](TextInputView::read_only) makes the field
//! non-interactive **without** dimming it — the presentation `enabled(false)`
//! cannot express, since disabled conflates two orthogonal questions
//! (interactive? / dimmed?) a static-but-live-styled mock must keep apart (a
//! splash screen's frozen preview must not pop to full alpha when it goes live).
//!
//! Non-interactivity reuses that same focus gate rather than a parallel path:
//! [`TextInputWidget::interactive`] is `enabled && !read_only`, and every gate
//! (the top of [`Widget::event`], the focused-while-painting check, the
//! disabled-while-focused IME-dismiss branch) reads it, so a field turned
//! read-only while focused releases focus and dismisses the platform IME exactly
//! like one turned disabled. **Dimming stays keyed to `enabled` alone**, never
//! `interactive`: both resolution points ([`Chrome::resolve`] and
//! [`effective_style`](TextInputWidget::effective_style)) branch on `enabled`.
//!
//! **Semantics.** Read-only is a real accessibility distinction from disabled —
//! a screen reader announces them differently — so [`Widget::semantics`] reports
//! `set_disabled()` only when `!enabled` and `set_read_only()` when
//! `enabled && read_only`, never both, and never by omitting the node
//! (`docs/CODE_STANDARDS.md` Semantics: a node with something to say keeps it).
//!
//! Read-only is orthogonal to [`obscured`](TextInputView::obscured) — never
//! touching masking, the mirror or the published `ImeContentType` hint, so a
//! read-only obscured field still reports `Role::PasswordInput` with a masked
//! a11y value, it just never focuses — and to the *Chrome* geometry setters.
//!
//! # Obscured (password) mode
//!
//! [`TextInputView::obscured(true)`](TextInputView::obscured) masks the rendered
//! glyphs with U+2022 BULLET. The [`TextEditor`]'s model text stays **real** —
//! every edit, selection, IME sync and `on_change` runs against the true buffer —
//! while the masked mirror lives in a parallel [`TextEditor`] (`mask_editor`).
//! That mirror, not the real editor, is measured, painted and hit-tested, since
//! masking only at glyph-emission time would leave the layout (and so the caret
//! rect, the field width and the pointer hit test) measured from the real text.
//! The mirror is a pure function of the real editing state, recomputed after every
//! edit, so it cannot drift; the mask is 1:1 per `char`, so offsets map with a
//! two-string walk ([`real_to_masked`]/[`masked_to_real`]) and a multi-byte
//! grapheme masks to exactly one bullet.
//!
//! **Scope boundary.** Obscuring is visual masking plus a [`Role::PasswordInput`]
//! semantics node plus an IME content-type hint: `obscured(true)` publishes
//! [`ImeContentType::Password`] on [`ImeState::content_type`], computed live on
//! every publication (event pass, paint-pass republish, disabled-while-focused
//! release), so a newly focused obscured field never has a window where it
//! publishes `Normal`. The published [`ImeState`] still carries the **real** text
//! (the platform IME mirror requires it), and the hint, not redaction, is what is
//! supposed to keep the platform's suggestion strip and learned-word dictionary
//! from seeing it — **a guarantee only as good as the shells honouring the
//! hint**, which this widget can ask for but never prove. `obscured` is the
//! field's sole content-type signal (see [`ImeContentType`] on why a shell must
//! not fall back to non-secret behaviour for an unrecognized variant).
//!
//! # Chrome
//!
//! The field's chrome **colors** (background/border/focus-accent/placeholder/
//! selection/caret) resolve from the active [`Theme`]'s `ColorScheme` (see
//! [`Chrome::resolve`]), so an app restyles them by installing a different
//! `Theme`. Its **geometry** ([`PAD_X`]/[`PAD_Y`] inner padding, [`BORDER_W`]
//! border thickness, [`RADIUS`] corner radius, [`CARET_W`] caret width) is
//! instead reachable per-instance, through [`TextInputView::padding`] and its
//! `border_width`/`corner_radius`/`caret_width` siblings; each defaults to the
//! constant it overrides, so a field calling none renders exactly as the defaults
//! do. Colors stay theme-only; geometry is builder-set rather than tokenized
//! because
//! [`PAD_X`]/[`CARET_W`] are read from the **event pass**
//! ([`TextInputWidget::editor_point`], [`TextInputWidget::current_ime_state`]),
//! which threads no `Theme` — `docs/CODE_STANDARDS.md` Theming cites these very
//! constants as that precedent — so a token would duplicate them anyway.
//!
//! **Focus treatment.** The focused state is an accent-colored border, no
//! separate halo/glow. [`TextInputView::focus_ring_width`] is the one escape hatch
//! on top — an optional border width used only while focused, defaulting to the
//! idle width — so an app reaches the "thicker, differently-colored focus outline"
//! look via a theme's `primary` role without a soft-glow primitive here.

use std::rc::Rc;
use std::time::Duration;

use frust_core::accesskit::Role;
use frust_core::{
    BoxConstraints, BuildCtx, ChangeFlags, EditingState, EventCtx, EventResult, FrameTime,
    ImeContentType, ImeEvent, ImeState, InputEvent, Key, LayoutCtx, NamedKey, PaintCtx, PaintScene,
    PointerPhase, SemanticsCtx, View, Widget,
};
use frust_text::{EditOp, EditingStateBytes, TextContext, TextEditor, TextStyle, utf16_to_byte};
use frust_theme::Theme;
use kurbo::{Point, Rect, Size, Vec2};
use peniko::Color;

use crate::authoring::presses;

/// Default corner radius of the field chrome, in logical px. Overridable
/// per-instance with [`TextInputView::corner_radius`] — see the module docs'
/// "Chrome" section for why this is a builder default rather than a `Theme`
/// token.
const RADIUS: f64 = 6.0;
/// Default border thickness, in logical px. Overridable per-instance with
/// [`TextInputView::border_width`] (see the module docs' "Chrome" section);
/// the focused border additionally honors [`TextInputView::focus_ring_width`].
const BORDER_W: f64 = 1.5;
/// Default horizontal inner padding (chrome edge to text), in logical px.
/// Overridable per-instance with [`TextInputView::padding`] (see the module
/// docs' "Chrome" section). Read from the event pass as well as layout/paint
/// (`docs/CODE_STANDARDS.md`'s Theming conventions cite this constant as the
/// precedent for why such a metric stays a plain value, not a `Theme` token).
const PAD_X: f64 = 8.0;
/// Default vertical inner padding (chrome edge to text), in logical px.
/// Overridable per-instance with [`TextInputView::padding`] — see [`PAD_X`].
const PAD_Y: f64 = 6.0;
/// Default caret width, in logical px. Overridable per-instance with
/// [`TextInputView::caret_width`] — see [`PAD_X`] for why it stays a plain
/// value rather than a `Theme` token.
const CARET_W: f32 = 1.5;
/// Blink half-period: caret visible 500 ms, hidden 500 ms.
const BLINK_MS: f64 = 500.0;
/// Default field width when the incoming constraints are horizontally unbounded.
const DEFAULT_WIDTH: f64 = 200.0;

/// Field background (unthemed fallback; a theme resolves this from `surface`).
const BG: Color = Color::WHITE;
/// Idle (unfocused) border color (unthemed fallback; themed from `outline`).
const BORDER: Color = Color::from_rgb8(0xD1, 0xD5, 0xDB);
/// Focused border color (unthemed fallback; themed from `primary`).
const ACCENT: Color = Color::from_rgb8(0x3B, 0x82, 0xF6);
/// Placeholder text color (unthemed fallback; themed from `on_surface_variant`).
const PLACEHOLDER: Color = Color::from_rgb8(0x9C, 0xA3, 0xAF);
/// Selection highlight color (unthemed fallback; themed from `primary` at alpha).
const SELECTION: Color = Color::from_rgb8(0xBF, 0xDB, 0xFE);
/// Caret color (unthemed fallback; themed from `primary`).
const CARET: Color = Color::from_rgb8(0x1D, 0x4E, 0xD8);
/// Alpha applied to `primary` for the themed selection highlight (a v1
/// simplification — a translucent primary stands in for a dedicated selection
/// role).
const SELECTION_ALPHA: f32 = 0.30;

/// Alpha multiplier applied to every resolved **content** color (glyphs,
/// placeholder, caret, selection) while the field is disabled.
///
/// Material 3's disabled state puts content at 38% of its enabled role color
/// (`m3.material.io` — *Styles → Color → Roles*, disabled-content opacity;
/// retrieved 2026-07-31). Applied as a *multiplier on the already-resolved
/// role*, never as a swap to a different token: `on_surface_variant` is opaque
/// under Material/Glyph but translucent under Cupertino (alpha 153 — see
/// `frust-theme`'s `ColorScheme` Cupertino arm), so a token swap would dim by
/// different amounts per design language while this multiplier does not.
const DISABLED_CONTENT_ALPHA: f32 = 0.38;
/// Alpha multiplier applied to the disabled field's **container** chrome (its
/// outline/accent border). Material 3 puts a disabled container/outline at 12%
/// (same source and retrieval date as [`DISABLED_CONTENT_ALPHA`]).
const DISABLED_CONTAINER_ALPHA: f32 = 0.12;

/// The glyph every character is replaced with in obscured (password) mode.
///
/// U+2022 BULLET is Android's `inputType=textPassword` default mask; the web
/// varies by browser and Apple's HIG names no glyph, so this follows the one
/// platform that publishes a specific character.
const MASK_CHAR: char = '\u{2022}';

/// The resolved text-field chrome colors. Themed (v1 simplification): background
/// `surface`, border `outline`, focus accent/caret `primary`, placeholder
/// `on_surface_variant`, selection `primary` at [`SELECTION_ALPHA`]. Unthemed:
/// the [`BG`]/[`BORDER`]/[`ACCENT`]/[`PLACEHOLDER`]/[`SELECTION`]/[`CARET`]
/// constants exactly, so a pre-theme app renders unchanged.
///
/// A disabled field dims the *resolved* values (see
/// [`DISABLED_CONTENT_ALPHA`]), so the themed and unthemed paths dim by the
/// same rule.
struct Chrome {
    bg: Color,
    border: Color,
    accent: Color,
    placeholder: Color,
    selection: Color,
    caret: Color,
}

impl Chrome {
    fn resolve(theme: Option<&Theme>, enabled: bool) -> Self {
        let mut chrome = match theme {
            Some(theme) => {
                let s = theme.scheme();
                Chrome {
                    bg: s.surface,
                    border: s.outline,
                    accent: s.primary,
                    placeholder: s.on_surface_variant,
                    selection: s.primary.with_alpha(SELECTION_ALPHA),
                    caret: s.primary,
                }
            }
            None => Chrome {
                bg: BG,
                border: BORDER,
                accent: ACCENT,
                placeholder: PLACEHOLDER,
                selection: SELECTION,
                caret: CARET,
            },
        };
        if !enabled {
            // `bg` is deliberately left opaque: it is this field's own
            // background painted over an arbitrary parent, so thinning it to
            // M3's 12% container value would show the parent through the field
            // rather than reading as "dimmed". The disabled cue is carried by
            // the outline and the content instead.
            chrome.border = chrome.border.multiply_alpha(DISABLED_CONTAINER_ALPHA);
            chrome.accent = chrome.accent.multiply_alpha(DISABLED_CONTAINER_ALPHA);
            chrome.placeholder = chrome.placeholder.multiply_alpha(DISABLED_CONTENT_ALPHA);
            chrome.selection = chrome.selection.multiply_alpha(DISABLED_CONTENT_ALPHA);
            chrome.caret = chrome.caret.multiply_alpha(DISABLED_CONTENT_ALPHA);
        }
        chrome
    }
}

/// The masked mirror of `text`: one [`MASK_CHAR`] per `char`, with `'\n'`
/// preserved so a multi-line field keeps its line structure (and therefore its
/// height) under masking.
fn mask_text(text: &str) -> String {
    text.chars().map(mask_char).collect()
}

/// The single character `ch` renders as while obscured.
fn mask_char(ch: char) -> char {
    if ch == '\n' { '\n' } else { MASK_CHAR }
}

/// Byte offset in the masked mirror of `text` corresponding to byte offset
/// `byte` in `text` itself.
///
/// An offset landing inside a multi-byte char snaps back to that char's start
/// (mirroring `frust_text`'s `byte_to_utf16`), so caret arithmetic across a
/// multi-byte grapheme stays exact.
fn real_to_masked(text: &str, byte: usize) -> usize {
    let mut masked = 0usize;
    for (off, ch) in text.char_indices() {
        if byte < off + ch.len_utf8() {
            return masked;
        }
        masked += mask_char(ch).len_utf8();
    }
    masked
}

/// The inverse of [`real_to_masked`]: a byte offset in the masked mirror mapped
/// back onto `text`. An offset inside a mask glyph snaps back to the start of
/// the char it stands for.
fn masked_to_real(text: &str, masked_byte: usize) -> usize {
    let mut masked = 0usize;
    for (off, ch) in text.char_indices() {
        let next = masked + mask_char(ch).len_utf8();
        if masked_byte < next {
            return off;
        }
        masked = next;
    }
    text.len()
}

/// A view-held, typed text callback (erased on build).
type OnText<State> = Rc<dyn Fn(&mut State, String)>;

/// A declarative text field, single-line by default. See the [module docs](self).
pub struct TextInputView<State: 'static> {
    value: String,
    placeholder: String,
    text_style: TextStyle,
    /// Whether the app set the content style explicitly (via
    /// [`TextInputView::text_style`]). When `false` and a theme is active, the
    /// glyph color resolves from the theme's `on_surface` role; an explicit
    /// style always wins (explicit > theme > black fallback).
    text_style_explicit: bool,
    /// `None` = single-line; `Some(n)` = wrapped multi-line capped at `n`
    /// visible lines (see [`TextInputView::multiline`]).
    max_visible_lines: Option<usize>,
    /// Whether Enter submits (vs. inserts a newline). `None` = use the
    /// mode-default (true single-line, false multi-line); `Some(_)` = an
    /// explicit [`TextInputView::submit_on_enter`] override.
    submit_on_enter: Option<bool>,
    /// Whether the field accepts input. `false` refuses focus (making keyboard
    /// and IME editing unreachable) and dims the chrome — see
    /// [`TextInputView::enabled`].
    enabled: bool,
    /// Whether the field refuses focus/editing without dimming — see
    /// [`TextInputView::read_only`]. Independent of `enabled`: dimming stays
    /// keyed to `enabled` alone (see the [module docs](self)' "Read-only mode"
    /// section).
    read_only: bool,
    /// Whether the rendered glyphs are masked (password mode) — see
    /// [`TextInputView::obscured`].
    obscured: bool,
    /// Horizontal inner padding — see [`TextInputView::padding`]. Defaults to
    /// [`PAD_X`].
    pad_x: f64,
    /// Vertical inner padding — see [`TextInputView::padding`]. Defaults to
    /// [`PAD_Y`].
    pad_y: f64,
    /// Border thickness — see [`TextInputView::border_width`]. Defaults to
    /// [`BORDER_W`].
    border_width: f64,
    /// Corner radius — see [`TextInputView::corner_radius`]. Defaults to
    /// [`RADIUS`].
    corner_radius: f64,
    /// Caret width — see [`TextInputView::caret_width`]. Defaults to
    /// [`CARET_W`].
    caret_width: f32,
    /// Focused-only border width override — see
    /// [`TextInputView::focus_ring_width`]. `None` = use `border_width` while
    /// focused too (unchanged appearance).
    focus_ring_width: Option<f64>,
    on_change: OnText<State>,
    on_submit: Option<OnText<State>>,
}

/// Create a controlled text field showing `value` that fires
/// `on_change(state, new_text)` on every edit.
///
/// The field is a controlled component: it reports the requested text through
/// `on_change` and adopts the app-confirmed `value` on the next rebuild — it is
/// never its own source of truth. Add a submit handler with
/// [`TextInputView::on_submit`] and a placeholder with
/// [`TextInputView::placeholder`].
pub fn text_input<State: 'static, F: Fn(&mut State, String) + 'static>(
    value: impl Into<String>,
    on_change: F,
) -> TextInputView<State> {
    TextInputView {
        value: value.into(),
        placeholder: String::new(),
        text_style: TextStyle::default(),
        text_style_explicit: false,
        max_visible_lines: None,
        submit_on_enter: None,
        enabled: true,
        read_only: false,
        obscured: false,
        pad_x: PAD_X,
        pad_y: PAD_Y,
        border_width: BORDER_W,
        corner_radius: RADIUS,
        caret_width: CARET_W,
        focus_ring_width: None,
        on_change: Rc::new(on_change),
        on_submit: None,
    }
}

/// PascalCase alias for [`text_input`].
#[allow(non_snake_case)]
pub fn TextInput<State: 'static, F: Fn(&mut State, String) + 'static>(
    value: impl Into<String>,
    on_change: F,
) -> TextInputView<State> {
    text_input(value, on_change)
}

impl<State: 'static> TextInputView<State> {
    /// Set the placeholder shown when the field is empty and unfocused.
    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// Set the submit handler fired on Enter (with the current text); focus is
    /// kept.
    pub fn on_submit<F: Fn(&mut State, String) + 'static>(mut self, on_submit: F) -> Self {
        self.on_submit = Some(Rc::new(on_submit));
        self
    }

    /// Set the content text's style (family/weight/style/size/color/
    /// letter-spacing/line-height), applied to the field's [`TextEditor`].
    /// Marks the color as explicitly set, so it wins over the themed default
    /// (explicit > theme > black fallback — see the [module docs](self)).
    /// Default (no call) resolves the color from the active theme's
    /// `on_surface` role, falling back to black with no theme threaded. Does
    /// not affect the chrome colors or padding/caret constants.
    pub fn text_style(mut self, text_style: TextStyle) -> Self {
        self.text_style = text_style;
        self.text_style_explicit = true;
        self
    }

    /// Make this a wrapped multi-line field that grows up to `max_visible_lines`
    /// lines tall, then scrolls internally to keep the caret in view (see the
    /// [module docs](self)). `max_visible_lines` is clamped to at least 1.
    ///
    /// Switches the default Enter behavior to insert a newline rather than
    /// submit; override with [`submit_on_enter`](Self::submit_on_enter).
    pub fn multiline(mut self, max_visible_lines: usize) -> Self {
        self.max_visible_lines = Some(max_visible_lines.max(1));
        self
    }

    /// Set whether Enter submits (`true`) or inserts a newline (`false`),
    /// overriding the mode default (submit single-line, newline multi-line).
    /// Shift+Enter always does the opposite. A single-line field always submits
    /// on Enter regardless of this setting.
    pub fn submit_on_enter(mut self, submit_on_enter: bool) -> Self {
        self.submit_on_enter = Some(submit_on_enter);
        self
    }

    /// Set whether the field accepts input (default `true`).
    ///
    /// A disabled field refuses focus, so a tap neither places the caret nor
    /// raises the keyboard and keyboard/IME editing cannot reach it at all; it
    /// paints no caret and dims its text, placeholder and outline (see the
    /// [module docs](self)). A field disabled while focused drops its focus.
    ///
    /// This is **disabled**, not *read-only*: Material 3 and Apple's HIG both
    /// keep a read-only field focusable and copyable, which this option
    /// deliberately does not do.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Set whether the field is read-only (default `false`): non-interactive
    /// like a disabled field (no focus, no caret, no editing — see
    /// [`enabled`](Self::enabled)) but painted at **full alpha**, not dimmed.
    ///
    /// This is the presentation `enabled(false)` cannot express: an
    /// undimmed-but-inert field, e.g. a static mock that should look identical
    /// before and after a live handoff. Reuses
    /// `enabled(false)`'s one suppression hook (the focus gate) rather than a
    /// parallel path — a field made read-only while focused releases focus and
    /// dismisses the platform IME exactly like disabling it would. See the
    /// [module docs](self)' "Read-only mode" section for the semantics
    /// distinction from disabled and the interaction with `obscured`/the
    /// chrome geometry setters.
    pub fn read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    /// Mask the rendered text with U+2022 BULLET (password mode, default
    /// `false`).
    ///
    /// The underlying value is untouched — `on_change` and the published
    /// `ImeState` still carry the real text, and caret/selection/editing
    /// arithmetic is unchanged, including across multi-byte graphemes. The
    /// field reports itself to accessibility as
    /// [`Role::PasswordInput`]. See the [module docs](self) for what obscuring
    /// deliberately does *not* do (no platform password keyboard, no autofill).
    pub fn obscured(mut self, obscured: bool) -> Self {
        self.obscured = obscured;
        self
    }

    /// Override the chrome's inner padding (chrome edge to text), in logical
    /// px. Defaults to [`PAD_X`]/[`PAD_Y`] (8×6) — a field that never calls
    /// this renders identically to before this setter existed. Independent of
    /// [`text_style`](Self::text_style)'s glyph metrics; see the module docs'
    /// "Chrome" section for why this is a per-instance value rather than a
    /// `Theme` token.
    pub fn padding(mut self, x: f64, y: f64) -> Self {
        self.pad_x = x;
        self.pad_y = y;
        self
    }

    /// Override the chrome's border thickness, in logical px. Defaults to
    /// [`BORDER_W`] (1.5). Affects only the idle/unfocused border unless
    /// [`focus_ring_width`](Self::focus_ring_width) is left unset, in which
    /// case the focused border uses this width too (unchanged relative
    /// behavior). See the module docs' "Chrome" section.
    pub fn border_width(mut self, width: f64) -> Self {
        self.border_width = width;
        self
    }

    /// Override the chrome's corner radius, in logical px. Defaults to
    /// [`RADIUS`] (6.0). See the module docs' "Chrome" section.
    pub fn corner_radius(mut self, radius: f64) -> Self {
        self.corner_radius = radius;
        self
    }

    /// Override the caret width, in logical px. Defaults to [`CARET_W`]
    /// (1.5). See the module docs' "Chrome" section.
    pub fn caret_width(mut self, width: f32) -> Self {
        self.caret_width = width;
        self
    }

    /// Use `width` as the border thickness only while the field is focused,
    /// instead of [`border_width`](Self::border_width). Unset by default, so
    /// a focused field's border is the same width as its idle border,
    /// matching the current behavior exactly. This is the narrow focus-ring
    /// escape hatch described in the module docs' "Chrome" section — combined
    /// with a theme's accent color, it reaches a thicker/differently-colored
    /// focus outline without this widget growing a second rendering
    /// primitive (a halo) it does not otherwise have.
    pub fn focus_ring_width(mut self, width: f64) -> Self {
        self.focus_ring_width = Some(width);
        self
    }
}

/// The retained widget for a [`TextInputView`].
pub struct TextInputWidget {
    /// The editing engine, always holding the **real** (never masked) text.
    /// Driven through the widget-owned `text_ctx`.
    editor: TextEditor,
    /// The masked mirror of `editor`, present only while obscured. A pure
    /// function of `editor`'s editing state (re-derived by
    /// [`sync_mask`](Self::sync_mask) after every edit, so it cannot drift),
    /// and the editor every *display* read goes through — see
    /// [`display`](Self::display) and the [module docs](self).
    mask_editor: Option<TextEditor>,
    /// The widget's own font/layout context — see the [module docs](self).
    text_ctx: TextContext,
    /// The declared (builder) style — family/weight/style/size/letter-spacing/
    /// line-height, plus the app's own color when [`text_style_explicit`] is
    /// `true`. Layout metrics (`content_height`/`text_top`/`line_height`) read
    /// this directly; the *color* actually installed on `editor` may differ —
    /// see [`applied_style`](Self::applied_style).
    style: TextStyle,
    /// Whether the app set the content style explicitly — see [`TextInputView`].
    text_style_explicit: bool,
    /// The style last installed on `editor` (the resolved effective style —
    /// see [`effective_style`](Self::effective_style)). Compared against the
    /// freshly-resolved style at each `layout` to detect a theme swap or a
    /// rebuilt `style`/`text_style_explicit`, in which case [`apply_style`]
    /// rebuilds `editor` with the new style.
    ///
    /// [`apply_style`]: Self::apply_style
    applied_style: TextStyle,
    placeholder: String,
    /// `None` = single-line; `Some(n)` = wrapped multi-line capped at `n`
    /// visible lines. Drives the wrap-width feed in `layout`, the height cap,
    /// and the keep-caret-in-view scroll offset (see the [module docs](self)).
    max_visible_lines: Option<usize>,
    /// The soft-wrap width last installed on `editor` (`None` sentinel = not
    /// yet applied / editor just rebuilt). `layout` only calls
    /// [`TextEditor::set_wrap_width`] when the desired width differs from this —
    /// mirroring `Text`'s cached-shape reuse. parley's
    /// `PlainEditor::set_width` unconditionally marks its layout dirty and the
    /// next `refresh_layout` re-shapes, so an unconditional per-pass call
    /// re-shaped every frame; guarding it here reshapes only on an actual
    /// width/mode change (edits still reshape via their own `apply`).
    applied_wrap_width: Option<Option<f32>>,
    /// Resolved Enter behavior: `true` submits, `false` inserts a newline.
    /// Defaults to true single-line / false multi-line; Shift+Enter inverts it
    /// (multi-line only — a single-line field always submits).
    submit_on_enter: bool,
    /// Whether the field accepts input (see [`TextInputView::enabled`]). Feeds
    /// [`interactive`](Self::interactive) (the gate for the whole `event()`
    /// pass) and, alone, both theme-resolution points' dimming — see
    /// [`Chrome::resolve`]/[`effective_style`](Self::effective_style).
    enabled: bool,
    /// Whether the field is read-only (see [`TextInputView::read_only`]).
    /// Feeds [`interactive`](Self::interactive) alongside `enabled`, but never
    /// dimming — the whole point of the flag (see the [module docs](self)'
    /// "Read-only mode" section).
    read_only: bool,
    /// Whether the rendered glyphs are masked (see [`TextInputView::obscured`]).
    /// Kept alongside `mask_editor` (which it decides) so `rebuild` can compare
    /// it and `semantics` can pick its role without inspecting the mirror.
    obscured: bool,
    /// Set by a `rebuild` that disabled or read-onlied a field holding focus:
    /// the focus path lives on the widget's *pod*, which `rebuild` cannot reach
    /// (`BuildCtx` carries no focus seam), so the release is deferred to the
    /// first `event()` that arrives — meanwhile `paint` already refuses to
    /// behave as focused (no caret, no blink frame, an inactive IME surface).
    release_focus_pending: bool,
    /// The widget's event-pass view of its focus: set on a `Down` inside,
    /// cleared on Escape / a blur `Down` this widget observes. NOT authoritative
    /// for painting — `paint` reads `PaintCtx::has_focus()` (the pod-recorded
    /// focus path, ancestor-composed) and self-corrects this flag when a
    /// container-routed blur never reached `event()` (one-paint convergence).
    focused: bool,
    /// Armed by a `Down` inside (alongside `capture_pointer`) to drive
    /// drag-selection; cleared on `Up`/`Cancel`.
    captured: bool,
    /// The frame time the caret was last reset to visible (the blink phase's
    /// epoch). Recorded from the paint-pass [`PaintCtx::frame_time`], since the
    /// event pass carries no clock — see `blink_reset_pending`.
    blink_epoch: FrameTime,
    /// Set when an edit/focus during the (clockless) event pass requests a blink
    /// reset; the next paint records `blink_epoch` from `frame_time` and clears
    /// this. `true` initially so the first painted frame seeds the epoch.
    blink_reset_pending: bool,
    /// Resolved horizontal inner padding — see [`TextInputView::padding`].
    /// Read from both the event pass (`editor_point`/`current_ime_state`) and
    /// layout/paint (see [`PAD_X`]).
    pad_x: f64,
    /// Resolved vertical inner padding — see [`TextInputView::padding`] and
    /// [`PAD_Y`].
    pad_y: f64,
    /// Resolved border thickness — see [`TextInputView::border_width`] and
    /// [`BORDER_W`]. The idle border width; `paint` widens it to
    /// `focus_ring_width` instead while focused, when set.
    border_width: f64,
    /// Resolved corner radius — see [`TextInputView::corner_radius`] and
    /// [`RADIUS`].
    corner_radius: f64,
    /// Resolved caret width — see [`TextInputView::caret_width`] and
    /// [`CARET_W`].
    caret_width: f32,
    /// Focused-only border width override — see
    /// [`TextInputView::focus_ring_width`]. `None` = `paint` uses
    /// `border_width` while focused too (unchanged appearance).
    focus_ring_width: Option<f64>,
    on_change: crate::authoring::ErasedArgCallback<String>,
    on_submit: Option<crate::authoring::ErasedArgCallback<String>>,
}

/// Whether `pos` (widget-local) lies within a `size`-sized field.
fn inside(pos: Point, size: Size) -> bool {
    pos.x >= 0.0 && pos.y >= 0.0 && pos.x < size.width && pos.y < size.height
}

impl TextInputWidget {
    /// Whether the field can take focus and be edited — `false` if disabled
    /// *or* read-only. The single hook both flags suppress interactivity
    /// through: [`Widget::event`]'s top gate, `paint`'s `focused` computation,
    /// and the disabled/read-only-while-focused IME-dismiss branch all key off
    /// this rather than `enabled` alone, so `read_only` gets the exact same
    /// suppression `enabled(false)` already had with no parallel path. Dimming
    /// deliberately does **not** use this — see [`Chrome::resolve`] and
    /// [`effective_style`](Self::effective_style), which key off `enabled`
    /// alone (the [module docs](self)' "Read-only mode" section).
    fn interactive(&self) -> bool {
        self.enabled && !self.read_only
    }

    /// The style to shape the editor with: `style` unchanged when the color was
    /// set explicitly (or no theme is active), otherwise `style` with its color
    /// replaced by the theme's `on_surface` role. Mirrors
    /// [`crate::TextWidget`]'s `effective_style` — resolving the color here at
    /// LAYOUT time (where `TextInput`, like `Text`, bakes the glyph brush into
    /// the editor's shaped state) keeps the unthemed path pixel-identical to
    /// before this retrofit.
    ///
    /// While disabled, whatever color that resolution produced is dimmed by
    /// [`DISABLED_CONTENT_ALPHA`] — the layout half of the two-point dimming
    /// (the other is [`Chrome::resolve`]), and the reason a change to `enabled`
    /// must report `ChangeFlags::LAYOUT`.
    fn effective_style(&self, theme: Option<&Theme>) -> TextStyle {
        let mut style = if self.text_style_explicit {
            self.style.clone()
        } else {
            match theme {
                Some(theme) => {
                    let mut style = self.style.clone();
                    style.color = theme.scheme().on_surface;
                    style
                }
                None => self.style.clone(),
            }
        };
        if !self.enabled {
            style.color = style.color.multiply_alpha(DISABLED_CONTENT_ALPHA);
        }
        style
    }

    /// The editor every *display* read goes through: the masked mirror while
    /// obscured, the real editor otherwise. Layout metrics, glyph runs,
    /// selection rects, the caret rect and the pointer hit test all key off
    /// this, so masking changes what is measured and not just what is drawn
    /// (see the [module docs](self)).
    fn display(&self) -> &TextEditor {
        self.mask_editor.as_ref().unwrap_or(&self.editor)
    }

    /// (Re)create the masked mirror to match `obscured`, then seed it from the
    /// current editing state. Resets `applied_wrap_width` so the next `layout`
    /// re-installs the soft-wrap width on both editors (a freshly built
    /// [`TextEditor`] carries none, and that install is also what refreshes the
    /// new mirror's layout).
    fn rebuild_mask_editor(&mut self) {
        let style = self.applied_style.clone();
        self.mask_editor = self.obscured.then(|| TextEditor::new(&style));
        self.applied_wrap_width = None;
        self.sync_mask();
    }

    /// Re-derive the masked mirror from the real editing state. A no-op when
    /// not obscured, and (via [`EditOp::ApplyEditingState`]'s value-equality
    /// short-circuit) when nothing changed. Because the mirror is *derived*
    /// rather than edited in parallel, it can never drift from the real buffer.
    fn sync_mask(&mut self) {
        let Some(mask) = self.mask_editor.as_mut() else {
            return;
        };
        let real = self.editor.editing_state_bytes();
        let state = EditingStateBytes {
            text: mask_text(&real.text),
            base: real_to_masked(&real.text, real.base),
            extent: real_to_masked(&real.text, real.extent),
            composing: real
                .composing
                .map(|r| real_to_masked(&real.text, r.start)..real_to_masked(&real.text, r.end)),
        };
        mask.apply(EditOp::ApplyEditingState(state), &mut self.text_ctx);
    }

    /// Rebuild `editor` with `style`, preserving the current editing state
    /// (text/selection/composing) across the reconstruction — `TextEditor`
    /// exposes no post-construction style setter, so a style change (an app
    /// rebuild with a different `.text_style(...)`, or a theme swap re-resolving
    /// the themed color at the next `layout`) reconstructs the editor rather
    /// than mutating it in place.
    ///
    /// A desktop-path IME preedit (parley's own `raw_compose`) does not survive
    /// a mid-composition style swap — `ApplyEditingState` re-seeds it as a
    /// platform-tracked composing region instead (still reported correctly by
    /// `editing_state_utf16`/`editing_state_bytes`), a narrow, acceptable edge
    /// case since changing `text_style` mid-keystroke-composition is not a
    /// realistic app pattern.
    fn apply_style(&mut self, style: TextStyle) {
        let state = self.editor.editing_state_bytes();
        self.editor = TextEditor::new(&style);
        self.editor
            .apply(EditOp::ApplyEditingState(state), &mut self.text_ctx);
        self.applied_style = style;
        // The freshly built editor carries no wrap width (`TextEditor::new`
        // resets it to single-line); force `layout` to re-install the desired
        // width on the next pass. (`rebuild_mask_editor` re-asserts this too —
        // the masked mirror must be rebuilt with the same new style.)
        self.applied_wrap_width = None;
        self.rebuild_mask_editor();
    }

    /// The single-line text height from the editor's refreshed metrics, floored
    /// to a sensible line height for an empty field.
    fn content_height(&self) -> f64 {
        self.display()
            .layout_size()
            .height
            .max(self.style.size as f64 * 1.25)
    }

    /// The top-left of the text content within a `height`-tall field (vertically
    /// centered, never above the top padding). Single-line placement.
    fn text_top(&self, height: f64) -> f64 {
        ((height - self.content_height()) / 2.0).max(self.pad_y)
    }

    /// Height of one text line from the editor's own metrics, falling back to a
    /// sensible line height before the first layout / when the field is empty.
    fn line_height(&self) -> f64 {
        let h = self.display().layout_size().height;
        let n = self.display().line_count();
        if h > 0.0 && n > 0 {
            h / n as f64
        } else {
            self.style.size as f64 * 1.25
        }
    }

    /// The vertical scroll offset (content shifted up, in logical px) that keeps
    /// the caret's line in view once the content outgrows the visible box. Zero
    /// in single-line mode or while the content fits. Recomputed statelessly
    /// each pass from the caret rect — a v1 keep-caret-in-view stand-in for a
    /// full scroll composition (see the [module docs](self)).
    fn scroll_y(&self, field_height: f64) -> f64 {
        if self.max_visible_lines.is_none() {
            return 0.0;
        }
        let visible = (field_height - 2.0 * self.pad_y).max(0.0);
        let content = self.display().layout_size().height;
        if content <= visible {
            return 0.0;
        }
        let max_off = content - visible;
        let (y0, y1) = match self.display().cursor_rect(self.caret_width) {
            Some(c) => (c.y0, c.y1),
            None => (0.0, 0.0),
        };
        // Reveal the caret's bottom edge, clamp to the scrollable range, then
        // pull back up if that hid the caret's top edge (caret taller motion).
        let mut off = if y1 > visible { y1 - visible } else { 0.0 };
        off = off.clamp(0.0, max_off);
        if y0 < off {
            off = y0.clamp(0.0, max_off);
        }
        off
    }

    /// The y of the text content's top within a `height`-tall field: the
    /// single-line centered placement, or the multi-line top-padded placement
    /// shifted up by the keep-caret-in-view scroll offset.
    fn content_origin_y(&self, height: f64) -> f64 {
        if self.max_visible_lines.is_some() {
            self.pad_y - self.scroll_y(height)
        } else {
            self.text_top(height)
        }
    }

    /// Reset the blink so the caret is visible from the next painted frame
    /// (called on any edit / focus, during the clockless event pass). The actual
    /// epoch is recorded from `frame_time` on the next paint.
    fn reset_blink(&mut self) {
        self.blink_reset_pending = true;
    }

    /// Whether the caret is in its visible half-cycle at frame time `now`, phase
    /// measured from `blink_epoch`.
    fn caret_visible_at(&self, now: FrameTime) -> bool {
        let elapsed_ms = now.saturating_sub(self.blink_epoch).as_secs_f64() * 1000.0;
        ((elapsed_ms / BLINK_MS) as u64).is_multiple_of(2)
    }

    /// Replace the whole editing value (controlled reconcile / initial seed),
    /// placing the caret at the end. No callback fires.
    fn set_controlled_value(&mut self, value: &str) {
        let op = EditOp::ApplyEditingState(EditingStateBytes {
            text: value.to_string(),
            base: value.len(),
            extent: value.len(),
            composing: None,
        });
        self.editor.apply(op, &mut self.text_ctx);
        self.sync_mask();
    }

    /// Apply one editing op, then run the after-edit bookkeeping: reset the
    /// blink, fire `on_change` if the text actually changed, and republish the
    /// IME surface. Used for both text edits and selection-only moves.
    fn apply_edit(&mut self, ctx: &mut EventCtx, op: EditOp) {
        let before = self.editor.text().to_string();
        self.editor.apply(op, &mut self.text_ctx);
        self.finish_edit(ctx, before);
    }

    /// Shared after-edit bookkeeping (see [`apply_edit`](Self::apply_edit)),
    /// factored out so a multi-op edit (an IME commit) reports once.
    fn finish_edit(&mut self, ctx: &mut EventCtx, before: String) {
        // Re-derive the masked mirror first: the IME surface published below
        // (and any metric read this pass) takes its caret rect from it.
        self.sync_mask();
        self.reset_blink();
        let after = self.editor.text().to_string();
        if after != before {
            (self.on_change)(ctx, after);
        }
        self.publish(ctx);
        ctx.request_redraw();
    }

    /// Build the current editing state + caret (window coordinates) for the
    /// widget laid out at `origin`/`size`, so the shell can drive the platform
    /// IME. Shared by the event-pass [`publish`](Self::publish) and the
    /// paint-pass republish (see [`Widget::paint`]).
    fn current_ime_state(&self, origin: Point, size: Size) -> ImeState {
        let es = self.editor.editing_state_utf16();
        let editing = EditingState {
            text: es.text,
            selection_base: es.selection_base,
            selection_extent: es.selection_extent,
            composing_base: es.composing_base,
            composing_extent: es.composing_extent,
        };
        let offset = origin.to_vec2() + Vec2::new(self.pad_x, self.content_origin_y(size.height));
        // The caret rect is a *screen* placement hint, so it comes from the
        // displayed (possibly masked) layout — while `editing` above stays the
        // real text the platform IME mirror needs.
        let caret = self.display().cursor_rect(self.caret_width).map(|c| {
            Rect::new(
                c.x0 + offset.x,
                c.y0 + offset.y,
                c.x1 + offset.x,
                c.y1 + offset.y,
            )
        });
        ImeState {
            active: true,
            editing,
            caret,
            // `obscured` is the field's *only* content-type signal for now (no
            // builder exposes an override — see the module docs' rationale).
            // Computed live from `self.obscured` on every call, so there is no
            // window where a masked field publishes `Normal`: the very first
            // `ImeState` a newly focused obscured field emits already carries
            // `Password`, which is what actually closes the suggestion-strip
            // leak (a field that starts `Normal` and flips a frame later has
            // already leaked to the IME).
            content_type: if self.obscured {
                ImeContentType::Password
            } else {
                ImeContentType::Normal
            },
        }
    }

    /// Publish the current editing state + caret during the event pass so the
    /// shell can drive the platform IME.
    fn publish(&self, ctx: &mut EventCtx) {
        ctx.publish_ime_state(self.current_ime_state(ctx.origin(), ctx.size()));
    }

    /// Translate a widget-local pointer position into the editor's layout-local
    /// coordinate space (used for caret placement / drag-selection).
    fn editor_point(&self, pos: Point, height: f64) -> (f32, f32) {
        (
            (pos.x - self.pad_x) as f32,
            (pos.y - self.content_origin_y(height)) as f32,
        )
    }

    /// Place (or extend the selection to) the caret nearest a widget-local
    /// pointer position.
    ///
    /// Unobscured this is just [`EditOp::MoveToPoint`] on the real editor.
    /// Obscured, the point must be resolved against the *masked* layout — the
    /// glyphs the user actually sees, whose advances differ from the real
    /// text's — and the resulting offsets mapped back onto the real buffer, so
    /// a tap lands on the same character it visually points at.
    fn move_to_point(&mut self, ctx: &mut EventCtx, x: f32, y: f32, select: bool) {
        if self.mask_editor.is_none() {
            self.apply_edit(ctx, EditOp::MoveToPoint { x, y, select });
            return;
        }
        let (masked_base, masked_extent) = {
            let mask = self
                .mask_editor
                .as_mut()
                .expect("obscured field has a mask editor");
            mask.apply(EditOp::MoveToPoint { x, y, select }, &mut self.text_ctx);
            let m = mask.editing_state_bytes();
            (m.base, m.extent)
        };
        let real = self.editor.editing_state_bytes();
        let before = real.text.clone();
        let state = EditingStateBytes {
            base: masked_to_real(&real.text, masked_base),
            extent: masked_to_real(&real.text, masked_extent),
            ..real
        };
        self.editor
            .apply(EditOp::ApplyEditingState(state), &mut self.text_ctx);
        self.finish_edit(ctx, before);
    }

    /// Handle a keyboard key event (already focus-gated by the caller).
    fn handle_key(
        &mut self,
        ctx: &mut EventCtx,
        key: &Key,
        modifiers: frust_core::Modifiers,
    ) -> EventResult {
        match key {
            Key::Character(s) => {
                if modifiers.ctrl || modifiers.meta {
                    // The only editing shortcut wired for v1 is select-all; other
                    // chorded characters (copy/paste, …) are consumed, not typed.
                    if s.eq_ignore_ascii_case("a") {
                        self.apply_edit(ctx, EditOp::SelectAll);
                    }
                    return EventResult::Handled;
                }
                self.apply_edit(ctx, EditOp::Insert(s.clone()));
                EventResult::Handled
            }
            Key::Named(named) => {
                let select = modifiers.shift;
                match named {
                    NamedKey::Backspace => self.apply_edit(ctx, EditOp::Backdelete),
                    NamedKey::Delete => self.apply_edit(ctx, EditOp::Delete),
                    NamedKey::ArrowLeft => self.apply_edit(ctx, EditOp::MoveLeft { select }),
                    NamedKey::ArrowRight => self.apply_edit(ctx, EditOp::MoveRight { select }),
                    NamedKey::Home => self.apply_edit(ctx, EditOp::Home { select }),
                    NamedKey::End => self.apply_edit(ctx, EditOp::End { select }),
                    NamedKey::Enter => {
                        // Single-line always submits (Enter is never a newline).
                        // Multi-line: the resolved `submit_on_enter`, inverted by
                        // Shift, decides submit vs. insert-newline.
                        let submit = if self.max_visible_lines.is_some() {
                            self.submit_on_enter ^ modifiers.shift
                        } else {
                            true
                        };
                        if submit {
                            let text = self.editor.text().to_string();
                            if let Some(cb) = &mut self.on_submit {
                                cb(ctx, text);
                            }
                            ctx.request_redraw();
                        } else {
                            self.apply_edit(ctx, EditOp::InsertNewline);
                        }
                    }
                    NamedKey::Escape => {
                        ctx.release_focus();
                        self.focused = false;
                        ctx.request_redraw();
                    }
                    // Vertical motion drives the caret across lines in multi-line
                    // mode; in single-line mode there is only one line, so it (and
                    // Tab traversal) are no-ops.
                    NamedKey::ArrowUp => {
                        if self.max_visible_lines.is_some() {
                            self.apply_edit(ctx, EditOp::MoveUp { select });
                        } else {
                            return EventResult::Ignored;
                        }
                    }
                    NamedKey::ArrowDown => {
                        if self.max_visible_lines.is_some() {
                            self.apply_edit(ctx, EditOp::MoveDown { select });
                        } else {
                            return EventResult::Ignored;
                        }
                    }
                    NamedKey::Tab => {
                        return EventResult::Ignored;
                    }
                }
                EventResult::Handled
            }
        }
    }

    /// Handle an IME event (already focus-gated by the caller).
    fn handle_ime(&mut self, ctx: &mut EventCtx, event: &ImeEvent) -> EventResult {
        match event {
            ImeEvent::Compose { text, cursor } => {
                self.apply_edit(
                    ctx,
                    EditOp::Compose {
                        text: text.clone(),
                        cursor: *cursor,
                    },
                );
                EventResult::Handled
            }
            ImeEvent::Commit(s) => {
                // iOS Return contract: the Return key on a
                // UITextInput arrives as `insertText("\n")` → `Commit("\n")`. On a
                // single-line (or submit-on-enter) widget a lone newline commit means
                // SUBMIT, exactly like `NamedKey::Enter` — it must never insert a
                // literal '\n'. A newline-inserting multi-line field instead lands
                // the literal newline, matching its `NamedKey::Enter` behavior.
                if s == "\n" || s == "\r" || s == "\r\n" {
                    if self.max_visible_lines.is_some() && !self.submit_on_enter {
                        self.apply_edit(ctx, EditOp::InsertNewline);
                        return EventResult::Handled;
                    }
                    let text = self.editor.text().to_string();
                    if let Some(cb) = &mut self.on_submit {
                        cb(ctx, text);
                    }
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                // Commit the given text via the compose machinery so it replaces
                // any active preedit and lands at the caret in one edit.
                let before = self.editor.text().to_string();
                self.editor.apply(
                    EditOp::Compose {
                        text: s.clone(),
                        cursor: None,
                    },
                    &mut self.text_ctx,
                );
                self.editor.apply(EditOp::FinishCompose, &mut self.text_ctx);
                self.finish_edit(ctx, before);
                EventResult::Handled
            }
            ImeEvent::ApplyEditingState(state) => {
                self.apply_edit(ctx, editing_state_to_op(state));
                EventResult::Handled
            }
            // Bracket a composition session: nothing to mutate here.
            ImeEvent::Enabled | ImeEvent::Disabled => EventResult::Handled,
        }
    }
}

/// Resolve the effective Enter-submits behavior: an explicit
/// [`TextInputView::submit_on_enter`] wins, otherwise the mode default (submit
/// single-line, insert-newline multi-line).
fn resolve_submit_on_enter(
    max_visible_lines: Option<usize>,
    submit_on_enter: Option<bool>,
) -> bool {
    submit_on_enter.unwrap_or(max_visible_lines.is_none())
}

/// Convert a shell-facing (UTF-16-indexed) [`EditingState`] into the byte-indexed
/// [`EditOp::ApplyEditingState`] the editor consumes.
fn editing_state_to_op(state: &EditingState) -> EditOp {
    let text = &state.text;
    let base = utf16_to_byte(text, state.selection_base.max(0) as usize);
    let extent = utf16_to_byte(text, state.selection_extent.max(0) as usize);
    let composing = if state.composing_base >= 0 && state.composing_extent >= 0 {
        Some(
            utf16_to_byte(text, state.composing_base as usize)
                ..utf16_to_byte(text, state.composing_extent as usize),
        )
    } else {
        None
    };
    EditOp::ApplyEditingState(EditingStateBytes {
        text: text.clone(),
        base,
        extent,
        composing,
    })
}

impl<State: 'static> View<State> for TextInputView<State> {
    type Element = TextInputWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> TextInputWidget {
        let style = self.text_style.clone();
        let mut text_ctx = TextContext::new();
        let mut editor = TextEditor::new(&style);
        // Seed the initial controlled value (and refresh the layout metrics).
        editor.apply(
            EditOp::ApplyEditingState(EditingStateBytes {
                text: self.value.clone(),
                base: self.value.len(),
                extent: self.value.len(),
                composing: None,
            }),
            &mut text_ctx,
        );
        let mut widget = TextInputWidget {
            editor,
            mask_editor: None,
            text_ctx,
            style: style.clone(),
            text_style_explicit: self.text_style_explicit,
            applied_style: style,
            placeholder: self.placeholder.clone(),
            max_visible_lines: self.max_visible_lines,
            applied_wrap_width: None,
            submit_on_enter: resolve_submit_on_enter(self.max_visible_lines, self.submit_on_enter),
            enabled: self.enabled,
            read_only: self.read_only,
            obscured: self.obscured,
            release_focus_pending: false,
            focused: false,
            captured: false,
            blink_epoch: FrameTime::ZERO,
            blink_reset_pending: true,
            pad_x: self.pad_x,
            pad_y: self.pad_y,
            border_width: self.border_width,
            corner_radius: self.corner_radius,
            caret_width: self.caret_width,
            focus_ring_width: self.focus_ring_width,
            on_change: crate::authoring::erase_callback_arg(&self.on_change),
            on_submit: self
                .on_submit
                .as_ref()
                .map(crate::authoring::erase_callback_arg::<State, String>),
        };
        // Seeds the masked mirror when built obscured (a no-op otherwise).
        widget.rebuild_mask_editor();
        widget
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut TextInputWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        // Closures aren't comparable; reinstall the erased adapters unconditionally.
        element.on_change = crate::authoring::erase_callback_arg(&self.on_change);
        element.on_submit = self
            .on_submit
            .as_ref()
            .map(crate::authoring::erase_callback_arg::<State, String>);

        let mut flags = ChangeFlags::NONE;
        if prev.placeholder != self.placeholder {
            element.placeholder = self.placeholder.clone();
            flags |= ChangeFlags::PAINT;
        }
        // Reconcile the multi-line configuration. A change to the visible-line
        // cap changes the height clamp (relayout), and either knob can change
        // the resolved Enter behavior.
        if prev.max_visible_lines != self.max_visible_lines {
            element.max_visible_lines = self.max_visible_lines;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        element.submit_on_enter =
            resolve_submit_on_enter(self.max_visible_lines, self.submit_on_enter);
        // Enabled reconcile. LAYOUT (not just PAINT) because the disabled dim
        // is baked into the glyph color at layout time — the same
        // `set_theme` -> `ChangeFlags::LAYOUT` contract `text_style` rides.
        // A field disabled *while focused* must not be stranded focused: clear
        // the widget's own view of it now and flag the pod-level release for
        // the first event that reaches us (`BuildCtx` has no focus seam).
        if prev.enabled != self.enabled {
            element.enabled = self.enabled;
            if !self.enabled {
                element.release_focus_pending = element.focused;
                element.focused = false;
                element.captured = false;
            }
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        // Read-only reconcile: unlike `enabled`, PAINT only — `read_only`
        // never dims (see the module docs' "Read-only mode" section), so no
        // glyph color is baked differently at layout time. A field made
        // read-only *while focused* releases focus the same way a field
        // disabled while focused does (see `interactive`).
        if prev.read_only != self.read_only {
            element.read_only = self.read_only;
            if self.read_only {
                element.release_focus_pending = element.focused;
                element.focused = false;
                element.captured = false;
            }
            flags |= ChangeFlags::PAINT;
        }
        // Obscured reconcile: the masked mirror is what gets *measured*, and a
        // mask glyph's advance differs from the character it replaces, so this
        // resizes the field as well as repainting it.
        if prev.obscured != self.obscured {
            element.obscured = self.obscured;
            element.rebuild_mask_editor();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        // Text style reconcile: a changed declared style or explicit-
        // flag invalidates the style actually installed on the editor. The
        // editor itself is only rebuilt in `layout` (`effective_style`/
        // `apply_style`), mirroring `Text::rebuild`'s cache-invalidate-now,
        // resolve-at-layout split — this is also what makes a bare theme swap
        // (no view change at all, so `rebuild` never runs) still pick up the
        // new color, since `layout` always re-resolves against `applied_style`.
        if prev.text_style != self.text_style
            || prev.text_style_explicit != self.text_style_explicit
        {
            element.style = self.text_style.clone();
            element.text_style_explicit = self.text_style_explicit;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        // Controlled reconcile: adopt the app-confirmed value only when it differs
        // from what the editor currently holds, so an accepted edit leaves the
        // selection untouched and a rejected/normalized one is pulled back in.
        if self.value != element.editor.text() {
            element.set_controlled_value(&self.value);
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        // Chrome geometry reconcile (see the module docs' "Chrome" section).
        // Padding feeds the resolved field height and the multi-line wrap
        // width, so it needs a relayout; the rest (border width, corner
        // radius, caret width, the focus-ring override) are paint-only — none
        // of them change the `Size` `layout` returns.
        if prev.pad_x != self.pad_x || prev.pad_y != self.pad_y {
            element.pad_x = self.pad_x;
            element.pad_y = self.pad_y;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.border_width != self.border_width {
            element.border_width = self.border_width;
            flags |= ChangeFlags::PAINT;
        }
        if prev.corner_radius != self.corner_radius {
            element.corner_radius = self.corner_radius;
            flags |= ChangeFlags::PAINT;
        }
        if prev.caret_width != self.caret_width {
            element.caret_width = self.caret_width;
            flags |= ChangeFlags::PAINT;
        }
        if prev.focus_ring_width != self.focus_ring_width {
            element.focus_ring_width = self.focus_ring_width;
            flags |= ChangeFlags::PAINT;
        }
        flags
    }
}

impl Widget for TextInputWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // Late-registered app fonts (a shell's per-frame font drain, after this
        // widget's private context was built) — a lock and a length compare
        // when nothing is pending. On an actual registration the editor's own
        // retained parley layout is still shaped against the old faces, so it
        // has to be rebuilt: `apply_style` with the unchanged style does
        // exactly that (and takes the masked mirror with it), carrying the same
        // narrow mid-composition caveat a theme swap does — see `apply_style`.
        let fonts_changed = self.text_ctx.sync_app_fonts();
        // Resolve the themed style and rebuild the editor if it drifted from
        // what's currently installed (a theme swap, or a rebuild-invalidated
        // `style`/`text_style_explicit` — see `effective_style`/`apply_style`).
        let effective = self.effective_style(Theme::from_layout_ctx(ctx));
        if fonts_changed || effective != self.applied_style {
            self.apply_style(effective);
        }
        // Catch-all mirror refresh: a rebuild-driven value/obscured change lands
        // before this pass, and everything measured below reads `display()`.
        // Cheap when already in sync (value-equality short-circuit).
        self.sync_mask();

        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            DEFAULT_WIDTH
        };

        // Desired soft-wrap width: `Some(px)` reflows the content in multi-line
        // mode; `None` restores the single-line default (so `content_height`/
        // `content_origin_y` agree with the centered single-line placement,
        // resetting any stale wrap left by a dropped `.multiline(..)`). Only
        // re-install it when it actually changed — parley re-shapes on every
        // `set_width` regardless, so an unconditional per-pass call was the
        // TextInput mirror of `Text`'s re-shape-every-frame defect.
        let desired_wrap: Option<f32> = self
            .max_visible_lines
            .map(|_| (width - 2.0 * self.pad_x).max(0.0) as f32);
        if self.applied_wrap_width != Some(desired_wrap) {
            self.editor.set_wrap_width(desired_wrap, &mut self.text_ctx);
            // The masked mirror wraps at the same width (and this is also the
            // call that refreshes a freshly rebuilt mirror's layout).
            if let Some(mask) = self.mask_editor.as_mut() {
                mask.set_wrap_width(desired_wrap, &mut self.text_ctx);
            }
            self.applied_wrap_width = Some(desired_wrap);
        }

        let height = match self.max_visible_lines {
            Some(max_lines) => {
                // Clamp the reported content height to the [1, max_lines] line
                // band (+ padding); overflow scrolls in paint.
                let line_h = self.line_height();
                let content = self.display().layout_size().height.max(line_h);
                let capped = content.min(line_h * max_lines as f64);
                capped + 2.0 * self.pad_y
            }
            None => self.content_height() + 2.0 * self.pad_y,
        };
        bc.constrain(Size::new(width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let origin = ctx.origin();
        let size = ctx.size();
        let theme = Theme::from_paint_ctx(ctx);
        let reduce_motion = theme.map(|t| t.motion.reduce_motion).unwrap_or(false);
        let chrome = Chrome::resolve(theme, self.enabled);

        // Record the blink epoch from the shared frame clock once per pending
        // reset (a focus/edit flagged it during the clockless event pass).
        let now = ctx.frame_time();
        if self.blink_reset_pending {
            self.blink_epoch = now;
            self.blink_reset_pending = false;
        }

        // The pod's recorded focus path (threaded in via `PaintCtx::has_focus`)
        // is authoritative — not our own `self.focused`, which lags after a
        // *container-routed* blur (a sibling tap clears the pod's focus without
        // ever calling our `event()`). Observe that here and self-correct so the
        // widget converges one frame after the blur: the accent border, caret,
        // caret-blink continuation frame, and IME republish below all key off
        // `focused`, so they stop together and the field stops resurrecting the
        // IME surface the blur cleared.
        // A disabled or read-only field never *behaves* as focused, even if
        // the pod's focus path is still recorded (a `rebuild` that flipped
        // either flag on a focused field cannot reach it — see
        // `release_focus_pending`).
        let focused = ctx.has_focus() && self.interactive();
        if self.focused && !focused {
            self.focused = false;
        }

        // Chrome: a border-colored rounded rect with an inset background fills in
        // as the frame (there is no stroke-rect primitive on `PaintScene`).
        // The border width itself widens to `focus_ring_width` while focused,
        // when set — the seam's narrow focus-ring escape hatch (see the module
        // docs' "Chrome" section); unset, it stays `border_width` either way,
        // matching the pre-seam behavior exactly.
        let border_color = if focused {
            chrome.accent
        } else {
            chrome.border
        };
        let border_w = if focused {
            self.focus_ring_width.unwrap_or(self.border_width)
        } else {
            self.border_width
        };
        scene.fill_rounded_rect(origin, size, self.corner_radius, border_color);
        scene.fill_rounded_rect(
            Point::new(origin.x + border_w, origin.y + border_w),
            Size::new(
                (size.width - 2.0 * border_w).max(0.0),
                (size.height - 2.0 * border_w).max(0.0),
            ),
            (self.corner_radius - border_w).max(0.0),
            chrome.bg,
        );

        let text_origin = Point::new(
            origin.x + self.pad_x,
            origin.y + self.content_origin_y(size.height),
        );

        // Multi-line content can overflow the capped box; clip the text band so
        // scrolled-out lines stay inside the field. Popped at the end of paint.
        let clip_content = self.max_visible_lines.is_some();
        if clip_content {
            scene.push_clip(
                Point::new(origin.x + self.border_width, origin.y + self.pad_y),
                Size::new(
                    (size.width - 2.0 * self.border_width).max(0.0),
                    (size.height - 2.0 * self.pad_y).max(0.0),
                ),
            );
        }

        if self.editor.text().is_empty() && !focused {
            // Placeholder: shaped on demand through the widget-owned context.
            if !self.placeholder.is_empty() {
                let mut ph_style = self.style.clone();
                ph_style.color = chrome.placeholder;
                let layout = self.text_ctx.layout(&self.placeholder, &ph_style, None);
                for run in layout.to_scene_runs(text_origin) {
                    scene.draw_glyph_run(run);
                }
            }
        } else {
            let off = text_origin.to_vec2();
            // Selection highlights sit behind the glyphs. Both come from the
            // displayed layout — the masked mirror while obscured.
            for r in self.display().selection_rects() {
                scene.fill_rect(
                    Point::new(r.x0 + off.x, r.y0 + off.y),
                    Size::new(r.width(), r.height()),
                    chrome.selection,
                );
            }
            for run in self.display().to_scene_runs(text_origin) {
                scene.draw_glyph_run(run);
            }
        }

        // Caret: blink while focused. A paced (CosmeticLoop) continuation
        // request at the blink's own [`BLINK_MS`] half-period keeps the desktop
        // shell's wait-loop scheduling paints so the blink animates (the mobile
        // shells' continuous loops already do), while letting the mobile frame
        // gate throttle the cadence — the blink is an indefinite decorative
        // toggle with no endpoint, the same classification as the
        // design-system skeleton/progress/dots/toast/spinner loops, but naming
        // its own slower cadence instead of the theme's cap rate (module docs'
        // "Focus, IME and blink" section). At rest (unfocused) we stop
        // signalling. `reduce_motion` freezes the caret **visible** and stops
        // requesting blink frames entirely — it is a position cue, not a purely
        // decorative loop, so it is the one exception among the paced loops
        // that freezes lit rather than dark.
        if focused {
            if !reduce_motion {
                ctx.request_frame_paced_at(Duration::from_millis(BLINK_MS as u64));
            }
            // Republish the IME surface every painted frame while focused, so a
            // controlled change applied by a rebuild (a submit clearing the
            // field) refreshes the shell-facing state the event pass would
            // otherwise leave stale — the mobile IME mirror relies on this to
            // observe the clear (see `PaintCtx::publish_ime_state`).
            ctx.publish_ime_state(self.current_ime_state(origin, size));
            let caret_visible = reduce_motion || self.caret_visible_at(now);
            if caret_visible && let Some(c) = self.display().cursor_rect(self.caret_width) {
                let off = text_origin.to_vec2();
                scene.fill_rect(
                    Point::new(c.x0 + off.x, c.y0 + off.y),
                    Size::new(c.width(), c.height()),
                    chrome.caret,
                );
            }
        } else if !self.interactive() && ctx.has_focus() {
            // Disabled or read-only while still holding the pod's focus path:
            // publish an *inactive* IME surface so the shell dismisses the
            // keyboard on the very next frame rather than waiting for the
            // event-pass release (`release_focus_pending`). No caret, and no
            // frame request — a non-interactive field is at rest.
            let mut ime = self.current_ime_state(origin, size);
            ime.active = false;
            ime.caret = None;
            ctx.publish_ime_state(ime);
        }

        if clip_content {
            scene.pop_clip();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // The focus gate IS the disabled/read-only gate: refusing focus here
        // is what makes `Key`/`Ime` (focus-routed, never hit-tested)
        // unreachable, so `handle_key`/`handle_ime` need no disabled/read-only
        // guards of their own. A non-interactive field also consumes nothing
        // — it is inert, not a shield.
        if !self.interactive() {
            if ctx.has_focus() || self.release_focus_pending {
                ctx.release_focus();
                self.release_focus_pending = false;
                self.focused = false;
                self.captured = false;
                ctx.request_redraw();
            }
            return EventResult::Ignored;
        }
        match event {
            InputEvent::Pointer(p) => match p.phase {
                PointerPhase::Down => {
                    if inside(p.position, ctx.size()) {
                        // Caret placement and selection dragging are
                        // **primary-only**, deliberately stricter than a
                        // browser (which places the caret on a right-click
                        // before opening its context menu): frust has no
                        // context-menu contract to pair that with yet, so a
                        // secondary press moves nothing rather than silently
                        // relocating a caret the user cannot see a menu for.
                        // It is still consumed, and re-claims an *existing*
                        // focus session: the root reads any `Down` bubbling no
                        // claim as a blur, and a right-click must not blur a
                        // field being typed into or retract its keyboard. It
                        // never *starts* a session — right-clicking an
                        // unfocused field raises no keyboard.
                        if !presses(p) {
                            if ctx.has_focus() {
                                ctx.request_focus();
                            }
                            return EventResult::Handled;
                        }
                        ctx.request_focus();
                        ctx.capture_pointer();
                        self.focused = true;
                        self.captured = true;
                        let (x, y) = self.editor_point(p.position, ctx.size().height);
                        self.move_to_point(ctx, x, y, false);
                        EventResult::Handled
                    } else {
                        // A `Down` outside our bounds that still reaches us (we are
                        // the root) is a blur: drop focus. Nested, the container's
                        // routing clears our focus path instead.
                        if self.focused {
                            ctx.release_focus();
                            self.focused = false;
                            ctx.request_redraw();
                        }
                        EventResult::Ignored
                    }
                }
                PointerPhase::Move => {
                    if !self.captured {
                        return EventResult::Ignored;
                    }
                    let (x, y) = self.editor_point(p.position, ctx.size().height);
                    self.move_to_point(ctx, x, y, true);
                    EventResult::Handled
                }
                PointerPhase::Up => {
                    if !self.captured {
                        return EventResult::Ignored;
                    }
                    self.captured = false;
                    ctx.request_redraw();
                    EventResult::Handled
                }
                PointerPhase::Cancel => {
                    if !self.captured {
                        return EventResult::Ignored;
                    }
                    // A `Cancel` must never touch application state: only clear the
                    // drag flag and request a redraw.
                    self.captured = false;
                    ctx.request_redraw();
                    EventResult::Handled
                }
            },
            InputEvent::Key(k) => {
                if !ctx.has_focus() {
                    return EventResult::Ignored;
                }
                self.focused = true;
                self.handle_key(ctx, &k.key, k.modifiers)
            }
            InputEvent::Ime(e) => {
                if !ctx.has_focus() {
                    return EventResult::Ignored;
                }
                self.focused = true;
                self.handle_ime(ctx, e)
            }
            InputEvent::Scroll { .. } => EventResult::Ignored,
            // A leaf with nothing deferred: the broadcast is a harmless
            // fall-through (it must not touch the editor, the caret, or focus).
            InputEvent::Housekeeping => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // A single-line TextInput node exposing its current text as `value`.
        // accesskit tracks focus at the tree level, so a focused field records
        // itself as the pass's focus node rather than carrying a per-node flag.
        //
        // Obscured, the role becomes `PasswordInput` (the one piece of password
        // semantics this option delivers) and the reported value is the *masked*
        // mirror — an assistive-tech client reads the node value verbatim, so
        // publishing the real secret there would defeat the masking.
        let role = if self.obscured {
            Role::PasswordInput
        } else {
            Role::TextInput
        };
        let value = if self.obscured {
            mask_text(self.editor.text())
        } else {
            self.editor.text().to_string()
        };
        // Disabled and read-only are reported as distinct accesskit node
        // states, never conflated (a screen reader announces them
        // differently — see the module docs' "Read-only mode" section): a
        // disabled field says `set_disabled()`, a read-only *enabled* field
        // says `set_read_only()`. `!enabled` wins if somehow both flags are
        // set — a disabled field is the stronger claim.
        let id = ctx.push_node(role, |node| {
            node.set_value(value);
            if !self.enabled {
                node.set_disabled();
            } else if self.read_only {
                node.set_read_only();
            }
        });
        if self.focused {
            ctx.set_focused(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_core::{FrameTime, KeyEvent, Modifiers, PointerButton, PointerEvent, RenderRoot};
    use std::any::Any;

    #[derive(Default)]
    struct AppState {
        value: String,
        changes: u32,
        submits: u32,
        last_submit: String,
        reject: bool,
    }

    fn app_logic(state: &mut AppState) -> TextInputView<AppState> {
        text_input(state.value.clone(), |s: &mut AppState, v: String| {
            s.changes += 1;
            if !s.reject {
                s.value = v;
            }
        })
        .placeholder("type here")
        .on_submit(|s: &mut AppState, v: String| {
            s.submits += 1;
            s.last_submit = v;
        })
    }

    /// Build + lay out a render root over `app_logic`, ready for events.
    fn harness(state: &mut AppState) -> RenderRoot<AppState, TextInputView<AppState>> {
        let mut root = RenderRoot::new();
        root.rebuild(&mut app_logic, state);
        root.layout(Size::new(300.0, 200.0));
        root
    }

    fn widget(root: &RenderRoot<AppState, TextInputView<AppState>>) -> &TextInputWidget {
        let id = root.root_id().expect("root built");
        let w = root.tree().pod(id).expect("root pod").widget();
        (w as &dyn Any)
            .downcast_ref::<TextInputWidget>()
            .expect("root is a TextInputWidget")
    }

    fn pointer(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    /// The same event on the secondary (right) button.
    fn secondary_pointer(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Secondary,
        })
    }

    fn ch(text: &str) -> InputEvent {
        InputEvent::Key(KeyEvent {
            key: Key::Character(text.to_string()),
            modifiers: Modifiers::default(),
            repeat: false,
        })
    }

    fn named(key: NamedKey, modifiers: Modifiers) -> InputEvent {
        InputEvent::Key(KeyEvent {
            key: Key::Named(key),
            modifiers,
            repeat: false,
        })
    }

    #[test]
    fn tap_focuses_and_publishes_ime_state() {
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        assert!(!root.is_focus_active());
        assert!(root.ime_state().is_none());

        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));

        assert!(root.is_focus_active(), "tap inside focuses the field");
        assert!(widget(&root).focused);
        let ime = root.ime_state().expect("focus publishes an IME surface");
        assert!(ime.active);
        assert!(ime.caret.is_some(), "an IME surface carries a caret rect");
    }

    #[test]
    fn a_secondary_press_neither_focuses_nor_moves_the_caret_nor_blurs() {
        let mut state = AppState::default();
        let mut root = harness(&mut state);

        // Unfocused: a right-click takes no focus and opens no IME session,
        // deliberately stricter than a browser (which places a caret first).
        root.event(
            &mut state,
            &secondary_pointer(PointerPhase::Down, 10.0, 10.0),
        );
        assert!(!root.is_focus_active());
        assert!(!widget(&root).focused);
        assert!(root.ime_state().is_none());

        // Focused and typed into: a right-click anywhere in the field leaves the
        // caret where it is, and leaves the live session alone — it is consumed
        // as a tap *inside* the field, never read as an outside-tap blur.
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        root.event(&mut state, &pointer(PointerPhase::Up, 10.0, 10.0));
        for c in ["a", "b", "c"] {
            root.event(&mut state, &ch(c));
        }
        let caret = widget(&root).editor.editing_state_bytes().extent;
        assert_eq!(caret, 3, "the caret sits after the typed text");

        let outcome = root.event(
            &mut state,
            &secondary_pointer(PointerPhase::Down, 1.0, 10.0),
        );

        assert!(outcome.handled, "the field still swallows the press");
        assert_eq!(
            widget(&root).editor.editing_state_bytes().extent,
            caret,
            "a right-click places no caret"
        );
        assert!(!widget(&root).captured, "and starts no selection drag");
        assert!(root.is_focus_active(), "the typing session survives it");
        assert!(widget(&root).focused);
    }

    #[test]
    fn typing_inserts_and_fires_on_change() {
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));

        root.event(&mut state, &ch("h"));
        root.event(&mut state, &ch("i"));

        assert_eq!(state.value, "hi", "on_change fed each char into app state");
        assert_eq!(state.changes, 2);
        assert_eq!(widget(&root).editor.text(), "hi");
    }

    #[test]
    fn keys_ignored_while_unfocused() {
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        // No prior focus: a key is not consumed and does not edit.
        let outcome = root.event(&mut state, &ch("x"));
        assert!(!outcome.handled);
        assert_eq!(state.changes, 0);
        assert_eq!(widget(&root).editor.text(), "");
    }

    #[test]
    fn backspace_over_emoji_removes_grapheme() {
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        root.event(&mut state, &ch("a"));
        root.event(&mut state, &ch("\u{1F600}")); // grinning face (4 bytes)
        assert_eq!(widget(&root).editor.text(), "a\u{1F600}");

        root.event(
            &mut state,
            &named(NamedKey::Backspace, Modifiers::default()),
        );

        // The whole emoji code point is removed as a unit (the editor's
        // grapheme integrity), not a single byte.
        assert_eq!(widget(&root).editor.text(), "a");
        assert_eq!(state.value, "a");
    }

    #[test]
    fn arrows_with_shift_extend_selection() {
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        for c in ["a", "b", "c"] {
            root.event(&mut state, &ch(c));
        }
        let changes_before = state.changes;

        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        root.event(&mut state, &named(NamedKey::ArrowLeft, shift));

        let es = widget(&root).editor.editing_state_bytes();
        assert_ne!(es.base, es.extent, "shift+arrow extends the selection");
        assert_eq!(
            state.changes, changes_before,
            "a selection-only move does not fire on_change"
        );
    }

    #[test]
    fn enter_fires_on_submit_once_and_keeps_focus() {
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        root.event(&mut state, &ch("h"));
        root.event(&mut state, &ch("i"));

        root.event(&mut state, &named(NamedKey::Enter, Modifiers::default()));

        assert_eq!(state.submits, 1, "Enter fires on_submit exactly once");
        assert_eq!(state.last_submit, "hi");
        assert!(root.is_focus_active(), "submit keeps focus");
    }

    #[test]
    fn newline_commit_submits_instead_of_inserting() {
        // The iOS Return path: UITextInput's Return arrives as insertText("\n")
        // → ImeEvent::Commit("\n"). Must behave exactly like NamedKey::Enter on
        // this single-line widget — submit, keep the text newline-free.
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        root.event(&mut state, &ch("h"));
        root.event(&mut state, &ch("i"));

        root.event(
            &mut state,
            &InputEvent::Ime(ImeEvent::Commit("\n".to_string())),
        );

        assert_eq!(state.submits, 1, "newline commit fires on_submit once");
        assert_eq!(state.last_submit, "hi");
        assert!(
            !widget(&root).editor.text().contains('\n'),
            "no literal newline lands in the single-line field"
        );
        assert!(root.is_focus_active(), "submit keeps focus");

        // CRLF variant (some platforms/hardware keyboards): same behavior.
        root.event(
            &mut state,
            &InputEvent::Ime(ImeEvent::Commit("\r\n".to_string())),
        );
        assert_eq!(state.submits, 2);
        assert!(!widget(&root).editor.text().contains('\r'));
    }

    #[test]
    fn escape_releases_focus() {
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        assert!(root.is_focus_active());

        root.event(&mut state, &named(NamedKey::Escape, Modifiers::default()));

        assert!(!root.is_focus_active(), "Escape blurs the field");
        assert!(root.ime_state().is_none());
        assert!(!widget(&root).focused);
    }

    #[test]
    fn blur_via_outside_tap_unpublishes_ime_state() {
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        assert!(root.ime_state().is_some());

        // Tap outside the field's height (still reaches the root widget).
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 150.0));

        assert!(!root.is_focus_active(), "outside tap blurs the field");
        assert!(root.ime_state().is_none());
        assert!(!widget(&root).focused);
    }

    #[test]
    fn meta_a_selects_all_without_typing() {
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        for c in ["a", "b", "c"] {
            root.event(&mut state, &ch(c));
        }
        let changes_before = state.changes;

        let meta = Modifiers {
            meta: true,
            ..Modifiers::default()
        };
        root.event(
            &mut state,
            &InputEvent::Key(KeyEvent {
                key: Key::Character("a".to_string()),
                modifiers: meta,
                repeat: false,
            }),
        );

        let es = widget(&root).editor.editing_state_bytes();
        assert_eq!(es.base.min(es.extent), 0);
        assert_eq!(es.base.max(es.extent), 3, "whole buffer selected");
        assert_eq!(
            state.changes, changes_before,
            "select-all does not type an 'a'"
        );
        assert_eq!(widget(&root).editor.text(), "abc");
    }

    #[test]
    fn controlled_reconcile_rejected_value_shows_app_value() {
        let mut state = AppState {
            reject: true,
            ..AppState::default()
        };
        let mut root = harness(&mut state);
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));

        // The app rejects the edit: on_change fires but `value` stays empty while
        // the editor already holds "x".
        root.event(&mut state, &ch("x"));
        assert_eq!(state.changes, 1);
        assert_eq!(state.value, "");
        assert_eq!(widget(&root).editor.text(), "x");

        // The next rebuild reconciles the editor back to the app's (empty) value.
        root.rebuild(&mut app_logic, &mut state);
        assert_eq!(
            widget(&root).editor.text(),
            "",
            "a rejected edit is pulled back to the app's value on rebuild"
        );
    }

    #[test]
    fn controlled_reconcile_same_value_preserves_selection() {
        let mut state = AppState {
            value: "ab".to_string(),
            ..AppState::default()
        };
        let mut root = harness(&mut state);
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));

        let meta = Modifiers {
            meta: true,
            ..Modifiers::default()
        };
        root.event(
            &mut state,
            &InputEvent::Key(KeyEvent {
                key: Key::Character("a".to_string()),
                modifiers: meta,
                repeat: false,
            }),
        );
        let before = widget(&root).editor.editing_state_bytes();
        assert_ne!(before.base, before.extent);

        // Rebuild with the unchanged value: no reconcile, selection preserved.
        root.rebuild(&mut app_logic, &mut state);
        let after = widget(&root).editor.editing_state_bytes();
        assert_eq!(
            (before.base, before.extent),
            (after.base, after.extent),
            "an unchanged value leaves the selection intact"
        );
    }

    #[test]
    fn ime_apply_editing_state_syncs_value() {
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));

        // Mobile state-sync path: push a whole editing value (UTF-16 indexed).
        root.event(
            &mut state,
            &InputEvent::Ime(ImeEvent::ApplyEditingState(EditingState {
                text: "hello".to_string(),
                selection_base: 5,
                selection_extent: 5,
                composing_base: -1,
                composing_extent: -1,
            })),
        );

        assert_eq!(widget(&root).editor.text(), "hello");
        assert_eq!(state.value, "hello", "on_change reflects the synced value");
    }

    #[test]
    fn ime_commit_inserts_text() {
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));

        root.event(
            &mut state,
            &InputEvent::Ime(ImeEvent::Commit("ni".to_string())),
        );

        assert_eq!(widget(&root).editor.text(), "ni");
        assert_eq!(state.value, "ni");
    }

    #[test]
    fn cancel_disarms_drag_without_touching_state() {
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        assert!(widget(&root).captured);
        let changes_before = state.changes;

        root.event(&mut state, &pointer(PointerPhase::Cancel, 10.0, 10.0));

        assert!(!widget(&root).captured, "Cancel disarms the drag");
        assert_eq!(
            state.changes, changes_before,
            "Cancel must not fire on_change"
        );
    }

    #[test]
    fn blink_requests_frame_only_while_focused() {
        // A fresh, unfocused field does not ask for continuation frames.
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        let mut sink = NullScene;
        assert!(
            !root.paint(&mut sink, FrameTime::ZERO).needs_frame,
            "an unfocused field is at rest"
        );

        // Once focused, paint pumps the blink and asks for the next frame — a
        // paced (CosmeticLoop) request, the same classification the
        // design-system decorative loops (skeleton/progress/dots/toast) use,
        // since the blink is an indefinite toggle with no endpoint the mobile
        // frame gate may throttle.
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        let outcome = root.paint(&mut sink, FrameTime::ZERO);
        assert!(outcome.needs_frame, "a focused field blinks its caret");
        assert!(
            outcome.needs_frame_paced_only,
            "the caret blink is a CosmeticLoop request — the frame gate must be able to pace it"
        );
    }

    #[test]
    fn blink_paces_at_its_own_500ms_interval() {
        // The caret names its own (slower) cadence via
        // `request_frame_paced_at` rather than a bare `request_frame_paced`
        // (the theme's cosmetic-loop cap) — asserted on the paint outcome's
        // `paced_interval`, mirroring `frust-core`'s
        // `paint_surfaces_the_requested_paced_interval_on_outcome`.
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        let mut sink = NullScene;
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        let outcome = root.paint(&mut sink, FrameTime::ZERO);
        assert!(outcome.needs_frame_paced_only);
        assert_eq!(
            outcome.paced_interval,
            Some(Duration::from_millis(BLINK_MS as u64)),
            "the caret paces at its own 500ms half-period, not the theme cap"
        );
    }

    #[test]
    fn reduce_motion_freezes_the_caret_visible_and_stops_requesting_frames() {
        // reduce_motion is the one exception among the paced loops: the caret
        // freezes VISIBLE (it is a position cue), not hidden, and stops
        // requesting blink frames entirely while frozen.
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        let mut theme = Theme::neutral();
        theme.motion.reduce_motion = true;
        root.set_theme(Box::new(theme));
        let caret_color = Theme::neutral().scheme().primary;

        // Focus seeds `blink_epoch` at the first paint (t=0).
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        root.paint(&mut NullScene, ft_ms(0.0));

        // Mid-cycle from that epoch — an unfrozen caret would be hidden here
        // (see `caret_visibility_toggles_across_the_blink_period`) — but
        // reduce_motion must still paint it, and request no continuation frame.
        let mut mid = CaretRecorder {
            caret_color: Some(caret_color),
            caret_fills: 0,
        };
        let outcome = root.paint(&mut mid, ft_ms(BLINK_MS + 10.0));
        assert_eq!(
            mid.caret_fills, 1,
            "reduce_motion freezes the caret visible, even mid-blink-cycle"
        );
        assert!(
            !outcome.needs_frame,
            "a frozen caret must not request a continuation frame"
        );

        // Clearing the token resumes the ordinary paced blink.
        let mut theme = Theme::neutral();
        theme.motion.reduce_motion = false;
        root.set_theme(Box::new(theme));
        let resumed = root.paint(&mut NullScene, ft_ms(2.0 * BLINK_MS + 20.0));
        assert!(
            resumed.needs_frame_paced_only,
            "blinking resumes once reduce_motion clears"
        );
        assert_eq!(
            resumed.paced_interval,
            Some(Duration::from_millis(BLINK_MS as u64))
        );
    }

    /// A `FrameTime` `ms` milliseconds from the origin.
    fn ft_ms(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    #[test]
    fn caret_visibility_toggles_across_the_blink_period() {
        let mut state = AppState::default();
        let root = harness(&mut state);
        let w = widget(&root);
        // Deterministic phase math off the reset epoch (blink_epoch == ZERO).
        assert!(
            w.caret_visible_at(ft_ms(0.0)),
            "visible at the start of the cycle"
        );
        assert!(w.caret_visible_at(ft_ms(BLINK_MS - 1.0)));
        assert!(
            !w.caret_visible_at(ft_ms(BLINK_MS + 1.0)),
            "hidden mid-cycle"
        );
        assert!(
            w.caret_visible_at(ft_ms(2.0 * BLINK_MS + 1.0)),
            "visible again"
        );
    }

    /// A scene that counts caret fills — the caret is the only bare `fill_rect`
    /// emitted with the caret color once a non-empty selection isn't present.
    #[derive(Default)]
    struct CaretRecorder {
        caret_color: Option<Color>,
        caret_fills: usize,
    }

    impl PaintScene for CaretRecorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, color: Color) {
            if Some(color) == self.caret_color {
                self.caret_fills += 1;
            }
        }
        fn fill_rounded_rect(&mut self, _o: Point, _s: Size, _r: f64, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
    }

    #[test]
    fn caret_blink_phase_advances_from_paint_frame_time() {
        // Two-frame blink test: the caret is painted in the visible half
        // of the cycle and absent in the hidden half, with the phase measured
        // purely from the injected `RenderRoot::paint` frame time — proving the
        // blink advances off the shell clock, not a hidden wall clock.
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        // Focus so the caret is painted; the focus Down flags a blink reset.
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));

        // Frame 1 at t=0: seeds blink_epoch=0 and paints the caret (visible half).
        let mut f1 = CaretRecorder {
            caret_color: Some(CARET),
            caret_fills: 0,
        };
        root.paint(&mut f1, ft_ms(0.0));
        assert_eq!(f1.caret_fills, 1, "caret visible at the start of the cycle");

        // Frame 2 mid-cycle (t = BLINK_MS + a bit): the caret is hidden.
        let mut f2 = CaretRecorder {
            caret_color: Some(CARET),
            caret_fills: 0,
        };
        root.paint(&mut f2, ft_ms(BLINK_MS + 10.0));
        assert_eq!(
            f2.caret_fills, 0,
            "caret hidden mid-cycle (phase from paint time)"
        );

        // Frame 3 in the next visible half proves the phase keeps advancing.
        let mut f3 = CaretRecorder {
            caret_color: Some(CARET),
            caret_fills: 0,
        };
        root.paint(&mut f3, ft_ms(2.0 * BLINK_MS + 10.0));
        assert_eq!(f3.caret_fills, 1, "caret visible again in the next cycle");
    }

    // --- Themed chrome ---

    /// Records rounded-rect (chrome) and rect (selection/caret) fill colors.
    #[derive(Default)]
    struct ChromeRecorder {
        rrects: Vec<Color>,
        rects: Vec<Color>,
    }

    impl PaintScene for ChromeRecorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, color: Color) {
            self.rects.push(color);
        }
        fn fill_rounded_rect(&mut self, _o: Point, _s: Size, _r: f64, color: Color) {
            self.rrects.push(color);
        }
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn draw_glyph_run(&mut self, _run: frust_scene::GlyphRun) {}
    }

    /// Paint a focused field (so the accent border + caret show) at frame time 0.
    fn paint_chrome(root: &mut RenderRoot<AppState, TextInputView<AppState>>) -> ChromeRecorder {
        let mut rec = ChromeRecorder::default();
        root.paint(&mut rec, FrameTime::ZERO);
        rec
    }

    #[test]
    fn unthemed_chrome_uses_fallback_constants() {
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        let rec = paint_chrome(&mut root);
        // Border (focused → accent) then background.
        assert_eq!(rec.rrects, vec![ACCENT, BG]);
        // The only bare rect on an empty focused field is the caret.
        assert_eq!(rec.rects, vec![CARET]);
    }

    #[test]
    fn themed_chrome_resolves_roles() {
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        root.set_theme(Box::new(Theme::neutral()));
        let theme = Theme::neutral();
        let scheme = theme.scheme();
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        let rec = paint_chrome(&mut root);
        assert_eq!(
            rec.rrects,
            vec![scheme.primary, scheme.surface],
            "focused border is primary, background is surface"
        );
        assert_eq!(rec.rects, vec![scheme.primary], "caret is primary");
    }

    // --- Chrome geometry seam: padding / border_width / corner_radius /
    // caret_width / focus_ring_width ---

    /// Records rounded-rect (chrome) and rect (selection/caret) fill
    /// *geometry* — origin/size/radius/color — so the seam's effect on the
    /// actual painted chrome can be asserted, not just that the builder
    /// accepted a value.
    #[derive(Default)]
    struct ChromeGeometryRecorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        rects: Vec<(Point, Size, Color)>,
    }

    impl PaintScene for ChromeGeometryRecorder {
        fn fill_rect(&mut self, origin: Point, size: Size, color: Color) {
            self.rects.push((origin, size, color));
        }
        fn fill_rounded_rect(&mut self, origin: Point, size: Size, radius: f64, color: Color) {
            self.rrects.push((origin, size, radius, color));
        }
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn draw_glyph_run(&mut self, _run: frust_scene::GlyphRun) {}
    }

    #[test]
    fn default_chrome_geometry_matches_the_unthemed_constants() {
        // Pins the resolved default geometry: a field that calls none of the
        // geometry setters must render byte-for-byte at the constants.
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        let field_size = root.layout(Size::new(300.0, 200.0));
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));

        let mut rec = ChromeGeometryRecorder::default();
        root.paint(&mut rec, FrameTime::ZERO);

        let (outer_origin, outer_size, outer_radius, _) = rec.rrects[0];
        let (inner_origin, inner_size, inner_radius, _) = rec.rrects[1];
        assert_eq!(outer_size, field_size, "outer chrome rect covers the field");
        assert_eq!(outer_radius, RADIUS, "default corner radius is RADIUS");
        assert_eq!(
            inner_origin.x - outer_origin.x,
            BORDER_W,
            "default border width is BORDER_W"
        );
        assert_eq!(inner_origin.y - outer_origin.y, BORDER_W);
        assert_eq!(
            inner_size,
            Size::new(
                outer_size.width - 2.0 * BORDER_W,
                outer_size.height - 2.0 * BORDER_W
            )
        );
        assert_eq!(inner_radius, RADIUS - BORDER_W);

        let (_, caret_size, _) = *rec.rects.last().expect("caret painted while focused");
        assert!(
            (caret_size.width - CARET_W as f64).abs() < 1e-6,
            "default caret width is CARET_W"
        );
    }

    #[test]
    fn custom_padding_changes_the_resolved_height_and_published_caret_offset() {
        let mut default_state = AppState::default();
        let mut default_root = harness(&mut default_state);
        let default_size = default_root.layout(Size::new(300.0, 200.0));
        default_root.event(&mut default_state, &pointer(PointerPhase::Down, 10.0, 10.0));
        let default_caret_x = default_root
            .ime_state()
            .expect("focused")
            .caret
            .expect("caret rect")
            .x0;

        let mut state = AppState::default();
        let mut logic = |s: &mut AppState| {
            text_input(s.value.clone(), |s: &mut AppState, v: String| s.value = v)
                .padding(30.0, 40.0)
        };
        let mut root = RenderRoot::new();
        root.rebuild(&mut logic, &mut state);
        let custom_size = root.layout(Size::new(300.0, 200.0));
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        let custom_caret_x = root
            .ime_state()
            .expect("focused")
            .caret
            .expect("caret rect")
            .x0;

        assert_eq!(
            custom_size.height - default_size.height,
            2.0 * (40.0 - PAD_Y),
            "vertical padding is reflected in the resolved field height, not just accepted"
        );
        assert!(
            (custom_caret_x - default_caret_x - (30.0 - PAD_X)).abs() < 1e-6,
            "horizontal padding shifts the published caret rect"
        );
    }

    #[test]
    fn custom_border_width_and_corner_radius_resize_the_painted_chrome() {
        let mut state = AppState::default();
        let mut logic = |s: &mut AppState| {
            text_input(s.value.clone(), |s: &mut AppState, v: String| s.value = v)
                .border_width(4.0)
                .corner_radius(2.0)
        };
        let mut root = RenderRoot::new();
        root.rebuild(&mut logic, &mut state);
        root.layout(Size::new(300.0, 200.0));

        let mut rec = ChromeGeometryRecorder::default();
        root.paint(&mut rec, FrameTime::ZERO);

        let (outer_origin, outer_size, outer_radius, _) = rec.rrects[0];
        let (inner_origin, inner_size, inner_radius, _) = rec.rrects[1];
        assert_eq!(
            outer_radius, 2.0,
            "corner_radius reaches the painted outer rect"
        );
        assert_eq!(
            inner_radius, 0.0,
            "inner radius clamps at 0 once border_width exceeds corner_radius"
        );
        assert_eq!(
            inner_origin.x - outer_origin.x,
            4.0,
            "border_width insets the fill"
        );
        assert_eq!(inner_origin.y - outer_origin.y, 4.0);
        assert_eq!(
            inner_size,
            Size::new(outer_size.width - 8.0, outer_size.height - 8.0)
        );
    }

    #[test]
    fn custom_caret_width_changes_the_painted_caret_rect() {
        let mut state = AppState::default();
        let mut logic = |s: &mut AppState| {
            text_input(s.value.clone(), |s: &mut AppState, v: String| s.value = v).caret_width(6.0)
        };
        let mut root = RenderRoot::new();
        root.rebuild(&mut logic, &mut state);
        root.layout(Size::new(300.0, 200.0));
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));

        let mut rec = ChromeGeometryRecorder::default();
        root.paint(&mut rec, FrameTime::ZERO);
        let (_, caret_size, _) = *rec.rects.last().expect("caret painted");
        assert!(
            (caret_size.width - 6.0).abs() < 1e-6,
            "caret_width is reflected in the painted caret rect, not just accepted by the builder"
        );
    }

    #[test]
    fn custom_focus_ring_width_only_widens_the_border_while_focused() {
        // The focus treatment specifically: unfocused, the idle border_width
        // is unaffected; focused, focus_ring_width takes over.
        let mut state = AppState::default();
        let mut logic = |s: &mut AppState| {
            text_input(s.value.clone(), |s: &mut AppState, v: String| s.value = v)
                .focus_ring_width(5.0)
        };
        let mut root = RenderRoot::new();
        root.rebuild(&mut logic, &mut state);
        root.layout(Size::new(300.0, 200.0));

        let mut idle = ChromeGeometryRecorder::default();
        root.paint(&mut idle, FrameTime::ZERO);
        let (idle_outer, _, _, _) = idle.rrects[0];
        let (idle_inner, _, _, _) = idle.rrects[1];
        assert_eq!(
            idle_inner.x - idle_outer.x,
            BORDER_W,
            "idle border stays the default width when unfocused"
        );

        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        let mut focused = ChromeGeometryRecorder::default();
        root.paint(&mut focused, FrameTime::ZERO);
        let (focused_outer, _, _, _) = focused.rrects[0];
        let (focused_inner, _, _, _) = focused.rrects[1];
        assert_eq!(
            focused_inner.x - focused_outer.x,
            5.0,
            "the focused appearance responds to focus_ring_width"
        );
    }

    #[test]
    fn geometry_field_changes_request_the_right_change_flags() {
        let mut state = AppState::default();
        let mut root = harness(&mut state);

        // Padding changes the resolved `Size`, so it must relayout.
        let mut padded = |s: &mut AppState| {
            text_input(s.value.clone(), |s: &mut AppState, v: String| s.value = v)
                .padding(20.0, 20.0)
        };
        let flags = root.rebuild(&mut padded, &mut state);
        assert!(flags.needs_layout(), "a padding change must relayout");

        // Border width alone is paint-only geometry — it never resizes the
        // field, so it must not force a relayout on top of an unrelated
        // padding change that already did.
        let mut bordered = |s: &mut AppState| {
            text_input(s.value.clone(), |s: &mut AppState, v: String| s.value = v)
                .padding(20.0, 20.0)
                .border_width(4.0)
        };
        let flags = root.rebuild(&mut bordered, &mut state);
        assert!(flags.needs_paint());
        assert!(
            !flags.needs_layout(),
            "border_width alone does not resize the field"
        );
    }

    #[test]
    fn paint_refreshes_ime_state_after_a_controlled_clear() {
        // The mobile IME mirror relies on `ime_state()` tracking the field even
        // when an app-driven controlled change (a submit clearing the draft) is
        // applied by a *rebuild* rather than an event. The event pass alone
        // leaves the published state stale (it only refreshes on edits); the
        // focused widget republishes during paint, which runs after every
        // rebuild. This is the regression guard for that refresh.
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        let mut sink = NullScene;

        // Focus and type "hi" through the event pass — ime_state now reads "hi".
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        root.event(&mut state, &ch("h"));
        root.event(&mut state, &ch("i"));
        assert_eq!(root.ime_state().expect("focused").editing.text, "hi");

        // Simulate an app-driven controlled clear (what `on_submit` → clear draft
        // does): set the controlled value to "" and run a frame with NO event.
        state.value.clear();
        root.rebuild(&mut app_logic, &mut state);
        root.layout(Size::new(300.0, 200.0));
        root.paint(&mut sink, FrameTime::ZERO);

        let ime = root
            .ime_state()
            .expect("still focused, so still publishing");
        assert_eq!(
            ime.editing.text, "",
            "paint refreshes the shell-facing IME state to the controlled-cleared value \
             (without this, the mobile IME mirror re-pushes the stale text)"
        );
    }

    // --- Themed text color ---

    /// Records each glyph run's solid brush color (mirrors `text.rs`'s
    /// `GlyphRecorder`).
    #[derive(Default)]
    struct TextGlyphRecorder {
        colors: Vec<Color>,
    }

    impl PaintScene for TextGlyphRecorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn fill_rounded_rect(&mut self, _o: Point, _s: Size, _r: f64, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn draw_glyph_run(&mut self, run: frust_scene::GlyphRun) {
            if let peniko::Brush::Solid(color) = run.brush {
                self.colors.push(color);
            }
        }
    }

    /// Paint `root` and return the first content glyph run's brush color.
    fn painted_text_color(root: &mut RenderRoot<AppState, TextInputView<AppState>>) -> Color {
        let mut rec = TextGlyphRecorder::default();
        root.paint(&mut rec, FrameTime::ZERO);
        *rec.colors.first().expect("one glyph run painted")
    }

    // --- Placeholder family resolution ---

    /// Records font bytes from each glyph run, enabling font-resolution testing.
    #[derive(Default)]
    struct PlaceholderFontRecorder {
        font_bytes: Vec<Vec<u8>>,
    }

    impl PaintScene for PlaceholderFontRecorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn fill_rounded_rect(&mut self, _o: Point, _s: Size, _r: f64, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn draw_glyph_run(&mut self, run: frust_scene::GlyphRun) {
            // Capture the font bytes from this glyph run.
            self.font_bytes.push(run.font.font().data.as_ref().to_vec());
        }
    }

    /// Paint `root` and return the font bytes of the first glyph run's font.
    fn painted_placeholder_font_bytes(
        root: &mut RenderRoot<AppState, TextInputView<AppState>>,
    ) -> Vec<u8> {
        let mut rec = PlaceholderFontRecorder::default();
        root.paint(&mut rec, FrameTime::ZERO);
        rec.font_bytes
            .first()
            .expect("one glyph run painted")
            .clone()
    }

    #[test]
    fn unthemed_text_input_keeps_black_default() {
        // Parity: with no theme threaded in, the glyph color stays exactly the
        // TextStyle default (black) — unchanged from before this retrofit.
        let mut state = AppState {
            value: "hi".to_string(),
            ..AppState::default()
        };
        let mut root = harness(&mut state);
        assert_eq!(painted_text_color(&mut root), Color::BLACK);
    }

    #[test]
    fn themed_dark_text_input_resolves_on_surface() {
        let mut state = AppState {
            value: "hi".to_string(),
            ..AppState::default()
        };
        let mut root = harness(&mut state);

        let mut theme = Theme::neutral();
        theme.brightness = frust_theme::Brightness::Dark;
        let expected = theme.scheme().on_surface;
        assert_ne!(
            expected,
            Color::BLACK,
            "fixture sanity: dark on_surface must differ from the black fallback"
        );
        root.set_theme(Box::new(theme));
        root.layout(Size::new(300.0, 200.0));

        assert_eq!(
            painted_text_color(&mut root),
            expected,
            "a themed field's glyphs resolve to on_surface(dark), not black"
        );
    }

    #[test]
    fn explicit_text_style_wins_over_theme() {
        let custom = Color::from_rgb8(10, 20, 30);
        fn logic(state: &mut AppState) -> TextInputView<AppState> {
            text_input(state.value.clone(), |s: &mut AppState, v: String| {
                s.value = v;
            })
            .text_style(TextStyle::new(16.0, Color::from_rgb8(10, 20, 30)))
        }

        let mut state = AppState {
            value: "hi".to_string(),
            ..AppState::default()
        };
        let mut root: RenderRoot<AppState, TextInputView<AppState>> = RenderRoot::new();
        root.rebuild(&mut logic, &mut state);
        root.set_theme(Box::new(Theme::neutral()));
        root.layout(Size::new(300.0, 200.0));

        assert_eq!(
            painted_text_color(&mut root),
            custom,
            "an explicit .text_style() color wins over the themed default"
        );
    }

    #[test]
    fn rebuild_with_new_text_style_relayouts_with_new_metrics() {
        // rebuild() must reconcile a changed text_style: a larger font size
        // rebuilds the underlying TextEditor (via `apply_style`) and grows the
        // field's measured height on the next layout.
        fn logic_a(state: &mut AppState) -> TextInputView<AppState> {
            text_input(state.value.clone(), |s: &mut AppState, v: String| {
                s.value = v;
            })
            .text_style(TextStyle::new(16.0, Color::BLACK))
        }
        fn logic_b(state: &mut AppState) -> TextInputView<AppState> {
            text_input(state.value.clone(), |s: &mut AppState, v: String| {
                s.value = v;
            })
            .text_style(TextStyle::new(40.0, Color::BLACK))
        }

        let mut state = AppState::default();
        let mut root: RenderRoot<AppState, TextInputView<AppState>> = RenderRoot::new();
        root.rebuild(&mut logic_a, &mut state);
        let size_a = root.layout(Size::new(300.0, 200.0));

        root.rebuild(&mut logic_b, &mut state);
        let size_b = root.layout(Size::new(300.0, 200.0));

        assert!(
            size_b.height > size_a.height,
            "a rebuild with a larger text_style size grows the field height \
             ({size_a:?} -> {size_b:?})"
        );
    }

    #[test]
    fn theme_swap_with_no_view_change_repaints_new_glyph_color() {
        // Mirrors `text.rs`'s regression of the same name: a bare `set_theme`
        // with no view change must still re-resolve the baked color at the
        // next layout, per the `set_theme` -> `ChangeFlags::LAYOUT` contract.
        let mut state = AppState {
            value: "hi".to_string(),
            ..AppState::default()
        };
        let mut root = harness(&mut state);

        let mut theme_a = Theme::neutral();
        theme_a.brightness = frust_theme::Brightness::Light;
        let color_a = theme_a.scheme().on_surface;
        root.set_theme(Box::new(theme_a));
        root.layout(Size::new(300.0, 200.0));
        assert_eq!(painted_text_color(&mut root), color_a);

        let mut theme_b = Theme::neutral();
        theme_b.brightness = frust_theme::Brightness::Dark;
        let color_b = theme_b.scheme().on_surface;
        assert_ne!(
            color_a, color_b,
            "fixture sanity: themes must actually differ"
        );
        root.set_theme(Box::new(theme_b));
        root.layout(Size::new(300.0, 200.0));

        assert_eq!(
            painted_text_color(&mut root),
            color_b,
            "a bare theme swap (no view change) must re-resolve the themed glyph \
             color at the next layout"
        );
    }

    // --- Nested-blur regression ---
    //
    // A `TextInput` nested in a `Column` beside a `Button`. Focusing the field
    // then tapping the sibling is a *container-routed* blur: `route_event`
    // clears the field pod's recorded focus path, but the widget's `event()` is
    // never called on that dispatch. If the widget-internal `focused` flag
    // stayed set, the next `paint` would republish the (already-cleared) IME
    // surface and pump a caret-blink continuation frame — resurrecting the
    // dismissed keyboard and keeping the desktop wait-loop spinning forever.
    // Threading the pod's focus into paint (`PaintCtx::has_focus`) is what makes
    // the widget observe the blur and converge.

    #[derive(Default)]
    struct NestedState {
        value: String,
    }

    fn nested_logic(state: &mut NestedState) -> crate::FlexView<NestedState> {
        use frust_core::any;
        crate::Column(vec![
            any(text_input(
                state.value.clone(),
                |s: &mut NestedState, v: String| {
                    s.value = v;
                },
            )),
            any(crate::button("ok", |_s: &mut NestedState| {})),
        ])
    }

    #[test]
    fn nested_blur_clears_ime_and_idles_paint() {
        let mut state = NestedState::default();
        let mut root: RenderRoot<NestedState, crate::FlexView<NestedState>> = RenderRoot::new();
        root.rebuild(&mut nested_logic, &mut state);
        let mut tcx = frust_text::TextContext::new();
        root.layout_with_text(Size::new(300.0, 200.0), &mut tcx as &mut dyn Any);

        // Focus the field with a full tap (Down+Up) inside its bounds, at the
        // top of the column. The Up releases the field's pointer capture, so the
        // next Down is hit-tested afresh instead of routing back to the field.
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        root.event(&mut state, &pointer(PointerPhase::Up, 10.0, 10.0));
        assert!(root.is_focus_active(), "tap inside the field focuses it");
        assert!(
            root.ime_state().is_some(),
            "focusing the nested field publishes an IME surface"
        );

        // Tap the sibling button, below the field (a container-routed blur): the
        // field's pod focus is cleared by `route_event`, but its own `event()` is
        // never called on this dispatch. The `Up` completes the button's own
        // tap cycle (`Button` has a press-scale
        // animation that lazily launches at the next `paint` — completing the
        // gesture here, with no paint in between, cancels the retarget before
        // it ever launches, so this stays a pure blur probe rather than also
        // asserting anything about the button's own animation).
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 45.0));
        root.event(&mut state, &pointer(PointerPhase::Up, 10.0, 45.0));

        // Leg 1 — the blur event clears the shell-facing focus + IME surface.
        assert!(!root.is_focus_active(), "the sibling tap blurs the field");
        assert!(
            root.ime_state().is_none(),
            "container-routed blur clears the IME surface"
        );

        // Legs 2 & 3 — a subsequent paint must NOT resurrect the cleared IME
        // surface, and must report the tree at rest (no stale caret-blink frame).
        let mut sink = NullScene;
        let outcome = root.paint(&mut sink, FrameTime::ZERO);
        assert!(
            root.ime_state().is_none(),
            "paint must not republish the cleared IME surface (F1 resurrection)"
        );
        assert!(
            !outcome.needs_frame,
            "a blurred field paints at rest — the desktop wait-loop idles"
        );
    }

    /// Two-level nesting: the field sits inside an INNER Column, whose pod is a
    /// child of the outer Column beside the button. A blur tap on the button
    /// clears the focus link at the outer level only (the inner Column's pod) —
    /// the field's own pod flag deep in the blurred subtree legitimately stays
    /// stale, which is exactly the case `paint_child`'s ancestor-composed
    /// `has_focus` seeding must cover.
    fn deep_nested_logic(state: &mut NestedState) -> crate::FlexView<NestedState> {
        use frust_core::any;
        crate::Column(vec![
            any(crate::Column(vec![any(text_input(
                state.value.clone(),
                |s: &mut NestedState, v: String| {
                    s.value = v;
                },
            ))])),
            any(crate::button("ok", |_s: &mut NestedState| {})),
        ])
    }

    #[test]
    fn deep_nested_blur_idles_paint_despite_stale_inner_focus_flag() {
        let mut state = NestedState::default();
        let mut root: RenderRoot<NestedState, crate::FlexView<NestedState>> = RenderRoot::new();
        root.rebuild(&mut deep_nested_logic, &mut state);
        let mut tcx = frust_text::TextContext::new();
        root.layout_with_text(Size::new(300.0, 200.0), &mut tcx as &mut dyn Any);

        // Focus the field (full Down+Up tap — the Up releases capture so the
        // blur Down hit-tests afresh; see nested_blur_clears_ime_and_idles_paint).
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        root.event(&mut state, &pointer(PointerPhase::Up, 10.0, 10.0));
        assert!(root.is_focus_active());
        assert!(root.ime_state().is_some());

        // Blur via the OUTER-level sibling: route_event clears the focused link
        // at the outer Column (the inner Column's pod); the field's own pod flag
        // two levels down stays stale. The `Up` completes the button's own tap
        // cycle (see `nested_blur_clears_ime_and_idles_paint`'s identical note)
        // so this stays a pure blur probe.
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 45.0));
        root.event(&mut state, &pointer(PointerPhase::Up, 10.0, 45.0));
        assert!(!root.is_focus_active());
        assert!(root.ime_state().is_none());

        // Paint: the cleared outer link must force has_focus == false for the
        // whole subtree (ancestor-composed seeding), so the stale deep flag
        // cannot re-arm the caret blink or republish the IME surface.
        let mut sink = NullScene;
        let outcome = root.paint(&mut sink, FrameTime::ZERO);
        assert!(
            root.ime_state().is_none(),
            "deep-nested stale focus flag must not resurrect the IME surface"
        );
        assert!(
            !outcome.needs_frame,
            "deep-nested stale focus flag must not busy-loop the desktop shell"
        );
    }

    // --- Cross-branch unmount: a stale flag must not kill a live session ------
    //
    // The shipped shape is a scrollable list above a persistent composer. The
    // user taps a row's field, then taps the composer: `route_event`'s
    // blur-on-outside-tap breaks the focus link at the NEAREST COMMON ANCESTOR
    // (the outer Column's pod for the list branch), so the row field's own pod
    // flag, one level deeper, legitimately stays set. The list then recycles /
    // filters / shrinks and the generic reconciler tears that row down.
    //
    // The row owns no session — the composer does. A release here is a
    // user-visible input regression: the keyboard drops mid-typing in a field
    // nothing touched. The effective-focus chain threaded through `BuildCtx`
    // (`has_focus`/`with_focus_link`) is what tells the two apart, and both
    // directions are pinned below over the same fixture.

    #[derive(Default)]
    struct BranchState {
        row: String,
        composer: String,
        show_row: bool,
        show_composer: bool,
    }

    /// Outer Column: [ inner Column (the "list", 0-or-1 row field), composer
    /// field ]. Each field's initial text names its branch, so `ime_state`
    /// identifies which one owns the session.
    fn branches_logic(state: &mut BranchState) -> crate::FlexView<BranchState> {
        use frust_core::{AnyView, any};
        let mut rows: Vec<AnyView<BranchState>> = Vec::new();
        if state.show_row {
            rows.push(any(text_input(
                state.row.clone(),
                |s: &mut BranchState, v: String| s.row = v,
            )));
        }
        let mut outer: Vec<AnyView<BranchState>> = vec![any(crate::Column(rows))];
        if state.show_composer {
            outer.push(any(text_input(
                state.composer.clone(),
                |s: &mut BranchState, v: String| s.composer = v,
            )));
        }
        crate::Column(outer)
    }

    /// Build the two-branch tree with both fields present, focus the ROW field,
    /// then focus the COMPOSER — leaving the row's own pod flag stale below the
    /// cleared outer link. Returns the root, its text context and the state.
    fn focus_row_then_composer() -> (
        RenderRoot<BranchState, crate::FlexView<BranchState>>,
        frust_text::TextContext,
        BranchState,
    ) {
        let mut state = BranchState {
            row: "row".to_string(),
            composer: "composer".to_string(),
            show_row: true,
            show_composer: true,
        };
        let mut root: RenderRoot<BranchState, crate::FlexView<BranchState>> = RenderRoot::new();
        root.rebuild(&mut branches_logic, &mut state);
        let mut tcx = frust_text::TextContext::new();
        root.layout_with_text(Size::new(300.0, 200.0), &mut tcx as &mut dyn Any);

        // Tap the row field (full Down+Up so the Up releases its capture and the
        // next Down hit-tests afresh — see nested_blur_clears_ime_and_idles_paint).
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        root.event(&mut state, &pointer(PointerPhase::Up, 10.0, 10.0));
        assert_eq!(
            root.ime_state().map(|s| s.editing.text),
            Some("row".to_string()),
            "the row field owns the session first"
        );

        // Tap the composer: focus moves branches. The blur clears the link at the
        // outer Column only; the row field's flag inside the inner Column stays.
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 50.0));
        root.event(&mut state, &pointer(PointerPhase::Up, 10.0, 50.0));
        assert!(root.is_focus_active());
        assert_eq!(
            root.ime_state().map(|s| s.editing.text),
            Some("composer".to_string()),
            "the composer now owns the session"
        );
        (root, tcx, state)
    }

    #[test]
    fn tearing_down_a_stale_focused_branch_leaves_the_live_session_alone() {
        let (mut root, _tcx, mut state) = focus_row_then_composer();
        let ime_before = root.ime_state();
        let gen_before = root.focus_ime_generation();

        // The list shrinks: the row field — whose stale flag is still set — is
        // torn down by the generic reconciler.
        state.show_row = false;
        root.rebuild(&mut branches_logic, &mut state);

        assert!(
            root.is_focus_active(),
            "unmounting a stale-flagged row must not blur the composer the user is typing in"
        );
        assert_eq!(
            root.ime_state(),
            ime_before,
            "the live IME surface is untouched by an unrelated branch's unmount"
        );
        assert_eq!(
            root.focus_ime_generation(),
            gen_before,
            "zero spurious edges: the shell's frame gate must see no focus/IME change at all"
        );
    }

    #[test]
    fn tearing_down_the_live_focused_branch_still_releases_the_session() {
        // The twin of the test above over the same fixture: when the pod that
        // actually holds the session dies, the release must still happen — the
        // narrowed gate must not have turned into "never mark".
        let (mut root, _tcx, mut state) = focus_row_then_composer();
        let gen_before = root.focus_ime_generation();

        state.show_composer = false;
        root.rebuild(&mut branches_logic, &mut state);

        assert!(
            !root.is_focus_active(),
            "the focused composer's unmount releases the session"
        );
        assert_eq!(
            root.ime_state(),
            None,
            "the shell-facing surface dies with the widget that published it"
        );
        assert_eq!(
            root.focus_ime_generation(),
            gen_before.wrapping_add(1),
            "one orphaned live focus path is exactly one edge"
        );
    }

    // --- Wrapper type swap: the severing path `rebuild_child` owns -----------
    //
    // `Padding(if editing { text_input } else { text })` is the single-child
    // wrapper shape: the view swaps the padded child's concrete type, so the
    // focused widget is torn down inside `AnyView::rebuild` and a fresh,
    // non-focusable one takes its pod. Nothing about that reaches the root by
    // itself — no event, no publish — so without the orphan mark the keyboard
    // stays up over an idle screen, `is_focus_active()` keeps reporting a
    // session, and `Key`/`Ime` events keep routing into a widget that ignores
    // them. Both directions are pinned below over one fixture, exactly like the
    // cross-branch unmount pair above.

    #[derive(Default)]
    struct WrapState {
        row: String,
        composer: String,
        row_editing: bool,
    }

    /// Outer Column: [ Padding-wrapped slot, composer field ]. The slot holds a
    /// `text_input` while `row_editing` and a plain `text` label otherwise, so
    /// flipping the flag is an `AnyView` type swap inside the wrapper — the
    /// `rebuild_child` path, not the multi-child reconciler's.
    fn wrapped_slot_logic(state: &mut WrapState) -> crate::FlexView<WrapState> {
        use frust_core::{AnyView, any};
        // The branch is taken *inside* the wrapper, so the wrapper's own view
        // type is stable across the flip and only its child's concrete type
        // changes — the `rebuild_child` swap. (Branching outside and handing
        // `Padding` a pre-erased `AnyView` would double-erase the child and
        // hide the swap from the wrapper entirely; see the note in the
        // completion summary.)
        let slot: AnyView<WrapState> = if state.row_editing {
            any(crate::Padding(
                crate::EdgeInsets::all(0.0),
                text_input(state.row.clone(), |s: &mut WrapState, v: String| s.row = v),
            ))
        } else {
            any(crate::Padding(
                crate::EdgeInsets::all(0.0),
                crate::text(state.row.clone()),
            ))
        };
        crate::Column(vec![
            slot,
            any(text_input(
                state.composer.clone(),
                |s: &mut WrapState, v: String| s.composer = v,
            )),
        ])
    }

    /// Build the fixture and focus the **wrapped** field, leaving it (and the
    /// wrapper pod above it) on the live focus chain.
    fn focus_the_wrapped_field() -> (
        RenderRoot<WrapState, crate::FlexView<WrapState>>,
        frust_text::TextContext,
        WrapState,
    ) {
        let mut state = WrapState {
            row: "row".to_string(),
            composer: "composer".to_string(),
            row_editing: true,
        };
        let mut root: RenderRoot<WrapState, crate::FlexView<WrapState>> = RenderRoot::new();
        root.rebuild(&mut wrapped_slot_logic, &mut state);
        let mut tcx = frust_text::TextContext::new();
        root.layout_with_text(Size::new(300.0, 200.0), &mut tcx as &mut dyn Any);

        // Full Down+Up so the Up releases the capture and a later Down
        // hit-tests afresh (see the cross-branch fixture above).
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        root.event(&mut state, &pointer(PointerPhase::Up, 10.0, 10.0));
        assert_eq!(
            root.ime_state().map(|s| s.editing.text),
            Some("row".to_string()),
            "the wrapped field owns the session"
        );
        (root, tcx, state)
    }

    #[test]
    fn swapping_a_live_focused_wrapper_child_releases_the_session() {
        let (mut root, _tcx, mut state) = focus_the_wrapped_field();
        let gen_before = root.focus_ime_generation();

        // The wrapper's child type-swaps out from under the focused field.
        state.row_editing = false;
        root.rebuild(&mut wrapped_slot_logic, &mut state);

        assert!(
            !root.is_focus_active(),
            "the root's focus mirror must not outlive the type-swapped field"
        );
        assert_eq!(
            root.ime_state(),
            None,
            "the shell-facing surface dies with the widget that published it"
        );
        assert_eq!(
            root.focus_ime_generation(),
            gen_before.wrapping_add(1),
            "one severed live focus path is exactly one edge"
        );
    }

    #[test]
    fn swapping_a_stale_focused_wrapper_child_leaves_the_live_session_alone() {
        let (mut root, _tcx, mut state) = focus_the_wrapped_field();

        // Focus moves to the composer: the blur clears the link at the outer
        // Column (the wrapper's own pod), leaving the wrapped field's flag one
        // level deeper legitimately stale.
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 50.0));
        root.event(&mut state, &pointer(PointerPhase::Up, 10.0, 50.0));
        assert_eq!(
            root.ime_state().map(|s| s.editing.text),
            Some("composer".to_string()),
            "the composer now owns the session"
        );
        let ime_before = root.ime_state();
        let gen_before = root.focus_ime_generation();

        // The same swap as the twin above, now on a dead branch.
        state.row_editing = false;
        root.rebuild(&mut wrapped_slot_logic, &mut state);

        assert!(
            root.is_focus_active(),
            "swapping a stale-flagged wrapper child must not blur the composer \
             the user is typing in"
        );
        assert_eq!(
            root.ime_state(),
            ime_before,
            "the live IME surface is untouched by an unrelated branch's swap"
        );
        assert_eq!(
            root.focus_ime_generation(),
            gen_before,
            "zero spurious edges: the shell's frame gate must see no focus/IME change"
        );
    }

    /// A no-op paint sink for `needs_frame` assertions (glyph/rect output is not
    /// under test here).
    struct NullScene;
    impl PaintScene for NullScene {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
    }

    // --- Multi-line mode ---

    /// A 3-visible-line multi-line field (newline-on-Enter default).
    fn multiline_logic(state: &mut AppState) -> TextInputView<AppState> {
        text_input(state.value.clone(), |s: &mut AppState, v: String| {
            s.changes += 1;
            if !s.reject {
                s.value = v;
            }
        })
        .multiline(3)
        .on_submit(|s: &mut AppState, v: String| {
            s.submits += 1;
            s.last_submit = v;
        })
    }

    /// A multi-line field that submits on Enter (newline only on Shift+Enter).
    fn multiline_submit_logic(state: &mut AppState) -> TextInputView<AppState> {
        text_input(state.value.clone(), |s: &mut AppState, v: String| {
            s.changes += 1;
            s.value = v;
        })
        .multiline(3)
        .submit_on_enter(true)
        .on_submit(|s: &mut AppState, v: String| {
            s.submits += 1;
            s.last_submit = v;
        })
    }

    /// A tall window so the field's natural (capped) height is never clamped by
    /// the root constraints; returns the laid-out field height.
    fn relayout(root: &mut RenderRoot<AppState, TextInputView<AppState>>) -> f64 {
        root.layout(Size::new(300.0, 800.0)).height
    }

    #[test]
    fn multiline_height_grows_per_line_up_to_cap_then_stops() {
        let mut state = AppState::default();
        let mut root = RenderRoot::new();
        root.rebuild(&mut multiline_logic, &mut state);
        let h1 = relayout(&mut root); // one (empty) line

        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        // Enter inserts a newline in this newline-on-Enter field.
        root.event(&mut state, &named(NamedKey::Enter, Modifiers::default()));
        let h2 = relayout(&mut root); // two lines
        root.event(&mut state, &named(NamedKey::Enter, Modifiers::default()));
        let h3 = relayout(&mut root); // three lines (== the cap)

        assert!(h2 > h1, "a second line grows the field: {h1} -> {h2}");
        assert!(h3 > h2, "a third line grows the field: {h2} -> {h3}");

        // A fourth and fifth line exceed the 3-line cap: the box height freezes.
        root.event(&mut state, &named(NamedKey::Enter, Modifiers::default()));
        let h4 = relayout(&mut root);
        root.event(&mut state, &named(NamedKey::Enter, Modifiers::default()));
        let h5 = relayout(&mut root);
        assert_eq!(h4, h3, "height stops growing at the visible-line cap");
        assert_eq!(h5, h4, "still capped past the cap");
    }

    #[test]
    fn multiline_scroll_offset_engages_only_past_the_cap() {
        let mut state = AppState::default();
        let mut root = RenderRoot::new();
        root.rebuild(&mut multiline_logic, &mut state);
        root.layout(Size::new(300.0, 800.0));
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));

        // Within the cap (two lines) the content fits: no scroll.
        root.event(&mut state, &named(NamedKey::Enter, Modifiers::default()));
        let h = relayout(&mut root);
        assert_eq!(
            widget(&root).scroll_y(h),
            0.0,
            "content within the cap never scrolls"
        );

        // Push past the cap: the caret (on the last line) must be kept in view by
        // a positive scroll offset.
        for _ in 0..4 {
            root.event(&mut state, &named(NamedKey::Enter, Modifiers::default()));
        }
        let h = relayout(&mut root);
        assert!(
            widget(&root).scroll_y(h) > 0.0,
            "content past the cap scrolls to keep the caret in view"
        );
    }

    #[test]
    fn enter_inserts_newline_in_multiline_by_default() {
        let mut state = AppState::default();
        let mut root = RenderRoot::new();
        root.rebuild(&mut multiline_logic, &mut state);
        root.layout(Size::new(300.0, 800.0));
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));

        root.event(&mut state, &ch("a"));
        root.event(&mut state, &named(NamedKey::Enter, Modifiers::default()));
        root.event(&mut state, &ch("b"));

        assert_eq!(
            widget(&root).editor.text(),
            "a\nb",
            "Enter inserts a newline"
        );
        assert_eq!(state.submits, 0, "a newline-on-Enter field does not submit");
        assert_eq!(state.value, "a\nb", "the newline flows through on_change");
    }

    #[test]
    fn shift_enter_submits_in_a_newline_on_enter_multiline() {
        let mut state = AppState::default();
        let mut root = RenderRoot::new();
        root.rebuild(&mut multiline_logic, &mut state);
        root.layout(Size::new(300.0, 800.0));
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        root.event(&mut state, &ch("h"));
        root.event(&mut state, &ch("i"));

        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        root.event(&mut state, &named(NamedKey::Enter, shift));

        assert_eq!(state.submits, 1, "Shift+Enter inverts the default → submit");
        assert_eq!(state.last_submit, "hi");
        assert!(
            !widget(&root).editor.text().contains('\n'),
            "Shift+Enter must not insert a newline here"
        );
    }

    #[test]
    fn submit_on_enter_multiline_submits_and_shift_enter_inserts_newline() {
        let mut state = AppState::default();
        let mut root = RenderRoot::new();
        root.rebuild(&mut multiline_submit_logic, &mut state);
        root.layout(Size::new(300.0, 800.0));
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        root.event(&mut state, &ch("h"));
        root.event(&mut state, &ch("i"));

        // Plain Enter submits (explicit submit_on_enter(true)).
        root.event(&mut state, &named(NamedKey::Enter, Modifiers::default()));
        assert_eq!(
            state.submits, 1,
            "Enter submits when submit_on_enter is set"
        );
        assert!(!widget(&root).editor.text().contains('\n'));

        // Shift+Enter inverts it → insert a newline instead.
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        root.event(&mut state, &named(NamedKey::Enter, shift));
        assert_eq!(state.submits, 1, "Shift+Enter does not submit here");
        assert!(
            widget(&root).editor.text().contains('\n'),
            "Shift+Enter inserts a newline when submit_on_enter is set"
        );
    }

    #[test]
    fn ime_newline_commit_inserts_newline_in_multiline() {
        // The iOS Return path (Commit("\n")) inserts a literal newline in a
        // newline-on-Enter multi-line field, rather than submitting.
        let mut state = AppState::default();
        let mut root = RenderRoot::new();
        root.rebuild(&mut multiline_logic, &mut state);
        root.layout(Size::new(300.0, 800.0));
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        root.event(&mut state, &ch("a"));

        root.event(
            &mut state,
            &InputEvent::Ime(ImeEvent::Commit("\n".to_string())),
        );

        assert!(
            widget(&root).editor.text().contains('\n'),
            "a newline commit lands a literal newline in a multi-line field"
        );
        assert_eq!(state.submits, 0, "the newline commit does not submit");
    }

    #[test]
    fn arrow_up_down_move_caret_across_lines_in_multiline() {
        let mut state = AppState::default();
        let mut root = RenderRoot::new();
        root.rebuild(&mut multiline_logic, &mut state);
        root.layout(Size::new(300.0, 800.0));
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        root.event(&mut state, &ch("a"));
        root.event(&mut state, &ch("b"));
        root.event(&mut state, &named(NamedKey::Enter, Modifiers::default()));
        root.event(&mut state, &ch("c"));
        root.event(&mut state, &ch("d")); // caret at end of line 2 (byte 5)
        assert_eq!(widget(&root).editor.editing_state_bytes().extent, 5);

        // ArrowUp crosses onto line 1 (byte offset within [0, 2]).
        root.event(&mut state, &named(NamedKey::ArrowUp, Modifiers::default()));
        let up = widget(&root).editor.editing_state_bytes().extent;
        assert!(up <= 2, "ArrowUp moves the caret onto line 1, got {up}");

        // ArrowDown returns to line 2 (byte offset >= 3, past the newline).
        root.event(
            &mut state,
            &named(NamedKey::ArrowDown, Modifiers::default()),
        );
        let down = widget(&root).editor.editing_state_bytes().extent;
        assert!(
            down >= 3,
            "ArrowDown moves the caret back to line 2, got {down}"
        );
    }

    /// Regression: `Widget::layout`'s single-line branch must reset the
    /// editor's wrap width, or a field rebuilt from multiline into single-line
    /// keeps the stale wrapped layout — `content_height` (and, downstream, the
    /// text's vertical placement) stays wrong until something else happens to
    /// touch the wrap width again.
    #[test]
    fn multiline_to_single_line_rebuild_resets_wrap_width() {
        // Long enough to wrap across several lines at a narrow width.
        let long_text = "one two three four five six seven eight nine ten".to_string();
        let mut state = AppState {
            value: long_text.clone(),
            ..AppState::default()
        };
        let mut root = RenderRoot::new();
        root.rebuild(&mut multiline_logic, &mut state);
        let multi_height = root.layout(Size::new(120.0, 800.0)).height;
        assert!(
            widget(&root).editor.line_count() > 1,
            "seed text must actually wrap for this regression to be meaningful"
        );

        // Rebuild the *same* root into a single-line field (multiline dropped)
        // with the identical text — the value is unchanged, so `rebuild` never
        // calls `set_controlled_value`; only `max_visible_lines` flips to
        // `None`, exercising the single-line `layout` branch on a widget whose
        // editor still carries the old wrap width.
        root.rebuild(&mut app_logic, &mut state);
        let single_height = root.layout(Size::new(120.0, 800.0)).height;

        assert_eq!(
            widget(&root).editor.line_count(),
            1,
            "single-line relayout must reset the wrap width so the editor \
             reflows back onto one line"
        );
        assert!(
            single_height < multi_height,
            "single-line relayout must collapse the stale wrapped height: \
             multi={multi_height}, single={single_height}"
        );

        // Cross-check against a field built single-line from scratch with the
        // same text/width: the rebuilt-down field must match it exactly, not
        // merely be smaller.
        let mut fresh_state = AppState {
            value: long_text,
            ..AppState::default()
        };
        let mut fresh_root = RenderRoot::new();
        fresh_root.rebuild(&mut app_logic, &mut fresh_state);
        let fresh_height = fresh_root.layout(Size::new(120.0, 800.0)).height;
        assert_eq!(
            single_height, fresh_height,
            "a multiline->single-line rebuild must match a field built \
             single-line from scratch"
        );

        // Layout and caret placement agree: the origin's centering formula is
        // now measured against the corrected (single-line) content height.
        let w = widget(&root);
        assert_eq!(
            w.content_origin_y(single_height),
            w.text_top(single_height),
            "single-line mode must use the centered single-line placement"
        );
    }

    #[test]
    fn placeholder_inherits_font_family_from_style() {
        // Verify that an empty unfocused field shows a placeholder with the input's
        // configured font family, weight, and letter-spacing — not the default style.
        use frust_text::{FontFamily, FontWeight};

        let mut state = AppState::default();
        let mut root = RenderRoot::new();

        // Build with a custom font family style.
        let custom_family = FontFamily::named("Monospace");
        let custom_style = TextStyle {
            family: custom_family.clone(),
            weight: FontWeight::BOLD,
            size: 18.0,
            letter_spacing: 1.5,
            ..TextStyle::default()
        };

        root.rebuild(
            &mut |s: &mut AppState| {
                text_input(s.value.clone(), |_s: &mut AppState, _v: String| {})
                    .placeholder("Enter text")
                    .text_style(custom_style.clone())
            },
            &mut state,
        );
        root.layout(Size::new(300.0, 200.0));

        let w = widget(&root);
        // Verify the widget's configured style has the custom family.
        assert_eq!(
            w.style.family, custom_family,
            "widget style should have the custom family"
        );
        assert_eq!(
            w.style.weight,
            FontWeight::BOLD,
            "widget style should have the custom weight"
        );
        assert_eq!(
            w.style.letter_spacing, 1.5,
            "widget style should have the custom letter-spacing"
        );

        // Paint the widget (the placeholder will be rendered since the field is empty and unfocused).
        let mut sink = NullScene;
        root.paint(&mut sink, FrameTime::ZERO);

        // The test verifies that the placeholder is laid out without crashing and
        // the field's style is correctly applied. A proper pixel-level assertion
        // would require inspecting glyph runs directly (which RecordingScene doesn't
        // support), but the layout success itself proves the family was accepted.
    }

    #[test]
    fn placeholder_font_reflects_configured_style_family() {
        // Genuine shaped-output assertion: verify that an empty,
        // unfocused field's placeholder shapes with the input's configured font
        // family, not a fallback. Two TextInputs—one with default family (SystemUi),
        // one with an explicit named family—must resolve to different fonts in
        // their placeholder runs. If `ph_style` regresses to `TextStyle::new(size,
        // color)`, both would drop the family and resolve to the same default font,
        // causing this assertion to fail.
        use frust_text::{FontFamily, GenericSlot};

        // First TextInput: default style (no explicit family).
        // The placeholder will shape with the default family (SystemUi).
        let mut state_default = AppState::default();
        let mut root_default = RenderRoot::new();
        root_default.rebuild(
            &mut |s: &mut AppState| {
                text_input(s.value.clone(), |_s: &mut AppState, _v: String| {}).placeholder("test")
            },
            &mut state_default,
        );
        root_default.layout(Size::new(300.0, 200.0));
        let default_font = painted_placeholder_font_bytes(&mut root_default);

        // Second TextInput: explicit monospace family using stack_with_generic.
        // Monospace is a generic family available on all platforms; it will
        // resolve to a different system font than SystemUi on any test host.
        let monospace_style = TextStyle {
            family: FontFamily::stack_with_generic(Vec::<String>::new(), GenericSlot::Monospace),
            ..TextStyle::default()
        };
        let mut state_monospace = AppState::default();
        let mut root_monospace = RenderRoot::new();
        root_monospace.rebuild(
            &mut |s: &mut AppState| {
                text_input(s.value.clone(), |_s: &mut AppState, _v: String| {})
                    .placeholder("test")
                    .text_style(monospace_style.clone())
            },
            &mut state_monospace,
        );
        root_monospace.layout(Size::new(300.0, 200.0));
        let monospace_font = painted_placeholder_font_bytes(&mut root_monospace);

        // Assert: the two placeholders resolved to different fonts.
        // If ph_style regressed (losing the family), both would use SystemUi
        // and resolve to the same font.
        assert_ne!(
            default_font, monospace_font,
            "placeholder with Monospace family should resolve to a different font \
             than the default SystemUi family; if this fails, ph_style likely \
             regressed to not preserving the configured family"
        );
    }

    // --- App-font parity (the shell drain must reach the private context) ---
    //
    // A `TextInput` owns a private `TextContext` (module docs' Text context
    // ownership). The defect this section pins: if that private context were a
    // bare `TextContext::new()`, an app font registered through
    // `frust::register_app_fonts` — drained by the shell into the *shell-owned*
    // context, the one `LayoutCtx::text_context` threads to `Text` — would never
    // reach the field, which would silently fall back to a platform face.
    //
    // `frust-widgets` cannot call `frust::register_app_fonts` (its registry
    // lives in `frust-shell-common`, above this crate), so these tests
    // reproduce the drain's single observable effect instead:
    // `FontRegistryWatcher::drain_into` is a `TextContext::register_fonts` call
    // on the shell-owned context, and nothing else.

    /// The registered test font's bytes — the same public-domain subsetted
    /// asset `frust-text`'s own registration tests use (and which
    /// `frust-shell-common` likewise includes cross-crate rather than
    /// duplicating a font file per crate).
    const TUFFY: &[u8] = include_bytes!("../../frust-text/tests/fonts/Tuffy-Subset.ttf");

    /// The same face with its `name` table rewritten to "Helvetica". Used for
    /// the late-registration test so the *pre-registration* leg is meaningful
    /// on any host: "Helvetica" resolves to whatever the platform has (or a
    /// fallback) before registration, and to these exact bytes after, per
    /// fontique's registered-shadows-system-family rule.
    const TUFFY_AS_HELVETICA: &[u8] =
        include_bytes!("../../frust-text/tests/fonts/Tuffy-As-Helvetica.ttf");

    /// Tuffy's digit advance at 24px, in logical px — measured from this exact
    /// asset through this exact parley pin (see `docs/DEVELOPMENT.md`'s
    /// Version-Pin Policy). Hard-coded so the assertion is on the *shaped
    /// metric*, not merely on "some font was registered": the device symptom
    /// behind this bug was a wrong per-glyph advance (1-em fallback boxes)
    /// while registration itself reported success.
    const TUFFY_DIGIT_ADVANCE_24PX: f32 = 13.3125;

    /// Slack on [`TUFFY_DIGIT_ADVANCE_24PX`]: parley lays out with
    /// `quantize = true`, so a run's successive x deltas land on subpixel
    /// boundaries and one digit pair in ten reads ~0.12px short of the nominal
    /// advance. Far tighter than any real font swap (a fallback face differs by
    /// whole pixels at this size).
    const ADVANCE_TOLERANCE: f32 = 0.25;

    /// The digits string every advance assertion shapes — uniform-width in
    /// Tuffy, so one expected advance covers every glyph in the run.
    const DIGITS: &str = "0123456789";

    /// Records each painted glyph run's resolved font bytes and per-glyph
    /// advances (successive x deltas) — shaped output, not registration state.
    #[derive(Default)]
    struct ShapedRunRecorder {
        runs: Vec<(Vec<u8>, Vec<f32>)>,
    }

    impl PaintScene for ShapedRunRecorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn fill_rounded_rect(&mut self, _o: Point, _s: Size, _r: f64, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn draw_glyph_run(&mut self, run: frust_scene::GlyphRun) {
            let advances = run.glyphs.windows(2).map(|w| w[1].x - w[0].x).collect();
            self.runs
                .push((run.font.font().data.as_ref().to_vec(), advances));
        }
    }

    /// Paint `root` and return the first glyph run's `(font bytes, advances)`.
    fn painted_shaped_run(
        root: &mut RenderRoot<AppState, TextInputView<AppState>>,
    ) -> (Vec<u8>, Vec<f32>) {
        let mut rec = ShapedRunRecorder::default();
        root.paint(&mut rec, FrameTime::ZERO);
        rec.runs.first().expect("one glyph run painted").clone()
    }

    /// A 24px style in `family`.
    fn family_style(family: frust_text::FontFamily) -> TextStyle {
        TextStyle {
            family,
            ..TextStyle::new(24.0, Color::BLACK)
        }
    }

    /// A laid-out field carrying `value` (empty = the placeholder path) in
    /// `family`, built *now* — i.e. with whatever fonts are registered at call
    /// time, which is the ordering under test.
    fn field_in_family(
        state: &mut AppState,
        family: frust_text::FontFamily,
    ) -> RenderRoot<AppState, TextInputView<AppState>> {
        let style = family_style(family);
        let mut root = RenderRoot::new();
        root.rebuild(
            &mut |s: &mut AppState| {
                text_input(s.value.clone(), |_s: &mut AppState, _v: String| {})
                    .placeholder(DIGITS)
                    .text_style(style.clone())
            },
            state,
        );
        root.layout(Size::new(600.0, 200.0));
        root
    }

    /// Assert the shaped run resolved to `expected`'s exact bytes.
    ///
    /// Compared behind a `bool` rather than `assert_eq!` on purpose: an
    /// `assert_eq!` between two font files prints both in full, which is tens
    /// of megabytes of failure output per failing test.
    fn assert_same_font(font: &[u8], expected: &[u8], what: &str) {
        assert!(
            font == expected,
            "{what}: shaped against a different face than the registered app \
             font ({} bytes shaped vs {} expected)",
            font.len(),
            expected.len()
        );
    }

    /// Assert the shaped run resolved to something *other* than `other`'s bytes.
    fn assert_other_font(font: &[u8], other: &[u8], what: &str) {
        assert!(
            font != other,
            "{what}: expected a different face, got the same {} bytes",
            font.len()
        );
    }

    /// Assert every advance in `advances` is `expected` (within quantization
    /// slack — see [`ADVANCE_TOLERANCE`]).
    fn assert_uniform_advance(advances: &[f32], expected: f32, what: &str) {
        assert!(!advances.is_empty(), "{what}: no advances measured");
        for a in advances {
            assert!(
                (a - expected).abs() <= ADVANCE_TOLERANCE,
                "{what}: shaped advance {a} != the registered font's own {expected} \
                 (all advances: {advances:?})"
            );
        }
    }

    #[test]
    fn app_registered_font_shapes_the_field_content() {
        // The shell's construction-time drain, reproduced exactly.
        let mut shell_ctx = frust_text::TextContext::new();
        shell_ctx
            .register_fonts(TUFFY.to_vec())
            .expect("valid TTF bytes must register");

        // A field built *after* that drain — the real ordering: a design system
        // registers its fonts from `app!`'s setup block, before the first
        // rebuild builds any widget.
        let mut state = AppState {
            value: DIGITS.to_string(),
            ..AppState::default()
        };
        let mut root = field_in_family(&mut state, frust_text::FontFamily::named("Tuffy"));
        let (font, advances) = painted_shaped_run(&mut root);

        assert_same_font(&font, TUFFY, "content");
        assert_uniform_advance(&advances, TUFFY_DIGIT_ADVANCE_24PX, "content");

        // Negative control: an unregistered family name resolves to a fallback
        // face, whose bytes cannot be the registered asset's.
        let mut fallback_state = AppState {
            value: DIGITS.to_string(),
            ..AppState::default()
        };
        let mut fallback_root = field_in_family(
            &mut fallback_state,
            frust_text::FontFamily::named("Frust No Such Family"),
        );
        let (fallback_font, fallback_advances) = painted_shaped_run(&mut fallback_root);
        assert_other_font(&fallback_font, TUFFY, "unregistered-family control");
        assert_ne!(
            fallback_advances, advances,
            "fixture sanity: the fallback face must shape these digits to \
             different metrics, or this test could pass without the app font"
        );
    }

    #[test]
    fn app_registered_font_shapes_the_placeholder() {
        // The placeholder shapes on its own path (`text_ctx.layout` in `paint`,
        // not the editor's retained layout), so it needs its own coverage.
        let mut shell_ctx = frust_text::TextContext::new();
        shell_ctx
            .register_fonts(TUFFY.to_vec())
            .expect("valid TTF bytes must register");

        // Empty value + never focused = the placeholder is what gets painted.
        let mut state = AppState::default();
        let mut root = field_in_family(&mut state, frust_text::FontFamily::named("Tuffy"));
        let (font, advances) = painted_shaped_run(&mut root);

        assert_same_font(&font, TUFFY, "placeholder");
        assert_uniform_advance(&advances, TUFFY_DIGIT_ADVANCE_24PX, "placeholder");
    }

    #[test]
    fn font_registered_after_build_reshapes_the_field_at_the_next_layout() {
        // The shells' *per-frame* late drain: a font can register after this
        // widget's private context was built. `layout`'s `sync_app_fonts` picks
        // it up and rebuilds the editor, whose retained parley layout would
        // otherwise stay shaped against the old faces (clearing the private
        // context's shape cache alone would not cover it).
        let mut state = AppState {
            value: DIGITS.to_string(),
            ..AppState::default()
        };
        let mut root = field_in_family(&mut state, frust_text::FontFamily::named("Helvetica"));
        let (before_font, before_advances) = painted_shaped_run(&mut root);
        assert_other_font(
            &before_font,
            TUFFY_AS_HELVETICA,
            "pre-registration control (nothing has registered this face yet)",
        );

        let mut shell_ctx = frust_text::TextContext::new();
        shell_ctx
            .register_fonts(TUFFY_AS_HELVETICA.to_vec())
            .expect("valid TTF bytes must register");

        // The shell forces LAYOUT on a `drain_into` that applied something;
        // this is that relayout.
        root.layout(Size::new(600.0, 200.0));
        let (after_font, after_advances) = painted_shaped_run(&mut root);

        // A registered family shadows any system "Helvetica" (fontique 0.11),
        // so this holds on every host.
        assert_same_font(&after_font, TUFFY_AS_HELVETICA, "late-registered");
        assert_uniform_advance(&after_advances, TUFFY_DIGIT_ADVANCE_24PX, "late-registered");
        assert_ne!(
            after_advances, before_advances,
            "the late registration must actually change the shaped metrics"
        );
    }

    // --- enabled(false) ---

    /// The standard fixture field, with `enabled`/`obscured`/`read_only`
    /// dialled in.
    fn options_logic(
        enabled: bool,
        obscured: bool,
        read_only: bool,
    ) -> impl FnMut(&mut AppState) -> TextInputView<AppState> {
        move |state: &mut AppState| {
            text_input(state.value.clone(), |s: &mut AppState, v: String| {
                s.changes += 1;
                s.value = v;
            })
            .placeholder("type here")
            .enabled(enabled)
            .obscured(obscured)
            .read_only(read_only)
        }
    }

    /// Build + lay out a root over `logic`.
    fn options_root(
        logic: &mut impl FnMut(&mut AppState) -> TextInputView<AppState>,
        state: &mut AppState,
    ) -> RenderRoot<AppState, TextInputView<AppState>> {
        let mut root = RenderRoot::new();
        root.rebuild(logic, state);
        root.layout(Size::new(300.0, 200.0));
        root
    }

    #[test]
    fn disabled_field_refuses_focus_and_stays_inert() {
        let mut state = AppState::default();
        let mut logic = options_logic(false, false, false);
        let mut root = options_root(&mut logic, &mut state);

        let outcome = root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        assert!(!outcome.handled, "a disabled field consumes nothing");
        assert!(
            !root.is_focus_active(),
            "a tap must not focus a disabled field"
        );
        assert!(!widget(&root).focused);
        assert!(!widget(&root).captured, "no drag capture either");
        assert!(root.ime_state().is_none(), "no IME surface is published");

        // Keys and IME are unreachable (they route down the focus path, which
        // the tap never claimed) and edit nothing even when injected directly.
        root.event(&mut state, &ch("x"));
        root.event(
            &mut state,
            &InputEvent::Ime(ImeEvent::Commit("ni".to_string())),
        );
        assert_eq!(widget(&root).editor.text(), "");
        assert_eq!(state.changes, 0);

        // No caret is painted, and the field is at rest.
        let mut rec = CaretRecorder {
            caret_color: Some(CARET.multiply_alpha(DISABLED_CONTENT_ALPHA)),
            caret_fills: 0,
        };
        let outcome = root.paint(&mut rec, FrameTime::ZERO);
        assert_eq!(rec.caret_fills, 0, "a disabled field paints no caret");
        assert!(!outcome.needs_frame, "a disabled field never blinks");
    }

    #[test]
    fn disabling_a_focused_field_releases_focus_and_deactivates_ime() {
        let mut state = AppState::default();
        let mut enabled_logic = options_logic(true, false, false);
        let mut root = options_root(&mut enabled_logic, &mut state);
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        assert!(root.is_focus_active());
        assert!(root.ime_state().expect("focused").active);

        // Flip the flag on a rebuild while the field holds focus.
        let mut disabled_logic = options_logic(false, false, false);
        let flags = root.rebuild(&mut disabled_logic, &mut state);
        assert!(
            flags.needs_layout(),
            "the disabled dim is baked at layout, so the flip must relayout"
        );
        root.layout(Size::new(300.0, 200.0));
        assert!(
            !widget(&root).focused,
            "the widget stops considering itself focused immediately"
        );

        // Leg 1 — the next paint already refuses to act focused and hands the
        // shell an inactive IME surface (dismissing the keyboard). The root reads
        // that inactive publish as a session release, so it drops the surface
        // outright (both mobile bridges serialise `None` to the same inactive
        // wire form — see `RenderRoot::ime_state`) and clears its focus mirror
        // rather than leaving it standing over a field that stopped editing.
        let mut sink = NullScene;
        let outcome = root.paint(&mut sink, FrameTime::ZERO);
        assert!(!outcome.needs_frame, "a disabled field paints at rest");
        assert!(
            root.ime_state().is_none(),
            "the published inactive surface releases the session"
        );
        assert!(
            !root.is_focus_active(),
            "the root's focus mirror goes with it"
        );

        // Leg 2 — the first event that reaches the field releases the pod-level
        // focus path, and edits nothing on the way.
        root.event(&mut state, &ch("x"));
        assert!(
            !root.is_focus_active(),
            "the stranded focus path is released"
        );
        assert!(root.ime_state().is_none());
        assert_eq!(widget(&root).editor.text(), "");
    }

    /// A design-system-shaped theme whose `on_surface_variant` role is itself
    /// **translucent** — the shape an iOS-style design system installs (its
    /// `secondaryLabel` token really is `rgba(.., 0.60)`). Built inline here
    /// because no design language ships in this crate any more. The dim under
    /// test is an alpha *multiplier* on the resolved role, so it has to behave
    /// identically over an already-translucent one — which is exactly what a
    /// token *swap* would not do.
    fn translucent_role_theme() -> Theme {
        Theme::builder(Theme::neutral())
            .design_language(frust_theme::DesignLanguage::Cupertino)
            .map_colors_light(|c| frust_theme::ColorScheme {
                on_surface_variant: c.on_surface_variant.multiply_alpha(0.6),
                ..c
            })
            .build()
    }

    #[test]
    fn disabled_dims_content_and_outline_in_every_design_language() {
        // The dim is an alpha multiplier on the *resolved* role, so it must
        // behave identically unthemed, under the neutral baseline, and under a
        // design system whose own `on_surface_variant` is already translucent
        // (see `translucent_role_theme` — the reason a token swap would not be
        // portable).
        let translucent = translucent_role_theme();
        assert!(
            translucent.scheme().on_surface_variant.components[3] < 1.0,
            "fixture sanity: the third arm's role must really be translucent"
        );
        let languages: Vec<(&str, Option<Theme>)> = vec![
            ("unthemed", None),
            ("neutral", Some(Theme::neutral())),
            ("translucent-role", Some(translucent)),
        ];
        for (name, theme) in languages {
            let enabled = Chrome::resolve(theme.as_ref(), true);
            let disabled = Chrome::resolve(theme.as_ref(), false);
            assert_eq!(
                disabled.placeholder,
                enabled.placeholder.multiply_alpha(DISABLED_CONTENT_ALPHA),
                "{name}: disabled placeholder is the enabled role at 38%"
            );
            assert_eq!(
                disabled.border,
                enabled.border.multiply_alpha(DISABLED_CONTAINER_ALPHA),
                "{name}: disabled outline is the enabled role at 12%"
            );
            assert!(
                disabled.placeholder.components[3] < enabled.placeholder.components[3],
                "{name}: the disabled placeholder must actually be more transparent"
            );
            assert_eq!(
                disabled.bg, enabled.bg,
                "{name}: the container fill stays opaque (see Chrome::resolve)"
            );
        }
    }

    #[test]
    fn disabled_dims_the_layout_baked_glyph_color() {
        // The second resolution point: the glyph color is baked into the shaped
        // editor state at LAYOUT time, so dimming has to happen there too.
        let mut state = AppState {
            value: "hi".to_string(),
            ..AppState::default()
        };
        let mut logic = options_logic(false, false, false);
        let mut root = options_root(&mut logic, &mut state);
        root.set_theme(Box::new(Theme::neutral()));
        root.layout(Size::new(300.0, 200.0));

        let expected = Theme::neutral()
            .scheme()
            .on_surface
            .multiply_alpha(DISABLED_CONTENT_ALPHA);
        assert_eq!(
            painted_text_color(&mut root),
            expected,
            "a disabled field's glyphs are on_surface at 38%"
        );
    }

    #[test]
    fn disabled_dims_the_unthemed_fallback_glyph_color() {
        let mut state = AppState {
            value: "hi".to_string(),
            ..AppState::default()
        };
        let mut logic = options_logic(false, false, false);
        let mut root = options_root(&mut logic, &mut state);
        assert_eq!(
            painted_text_color(&mut root),
            Color::BLACK.multiply_alpha(DISABLED_CONTENT_ALPHA),
            "with no theme threaded the black fallback dims by the same rule"
        );
    }

    // --- read_only(true) ---

    #[test]
    fn read_only_refuses_focus_and_stays_inert() {
        // Same inertness contract as disabled (reused suppression hook, not a
        // parallel one): no focus, no caret, no edits reach the field.
        let mut state = AppState::default();
        let mut logic = options_logic(true, false, true);
        let mut root = options_root(&mut logic, &mut state);

        let outcome = root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        assert!(!outcome.handled, "a read-only field consumes nothing");
        assert!(
            !root.is_focus_active(),
            "a tap must not focus a read-only field"
        );
        assert!(!widget(&root).focused);
        assert!(!widget(&root).captured, "no drag capture either");
        assert!(root.ime_state().is_none(), "no IME surface is published");

        root.event(&mut state, &ch("x"));
        root.event(
            &mut state,
            &InputEvent::Ime(ImeEvent::Commit("ni".to_string())),
        );
        assert_eq!(widget(&root).editor.text(), "");
        assert_eq!(state.changes, 0);

        let mut rec = CaretRecorder {
            caret_color: Some(CARET),
            caret_fills: 0,
        };
        let outcome = root.paint(&mut rec, FrameTime::ZERO);
        assert_eq!(rec.caret_fills, 0, "a read-only field paints no caret");
        assert!(!outcome.needs_frame, "a read-only field never blinks");
    }

    #[test]
    fn making_a_focused_field_read_only_releases_focus_and_deactivates_ime() {
        let mut state = AppState::default();
        let mut live_logic = options_logic(true, false, false);
        let mut root = options_root(&mut live_logic, &mut state);
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        assert!(root.is_focus_active());
        assert!(root.ime_state().expect("focused").active);

        // Flip the flag on a rebuild while the field holds focus.
        let mut read_only_logic = options_logic(true, false, true);
        let flags = root.rebuild(&mut read_only_logic, &mut state);
        assert!(
            !flags.needs_layout(),
            "read-only never dims, so the flip is PAINT-only, unlike disabled"
        );
        root.layout(Size::new(300.0, 200.0));
        assert!(
            !widget(&root).focused,
            "the widget stops considering itself focused immediately"
        );

        // Leg 1 — the next paint already refuses to act focused and hands the
        // shell an inactive IME surface (dismissing the keyboard), which the root
        // reads as a session release: surface dropped, focus mirror cleared (see
        // the disabled twin above).
        let mut sink = NullScene;
        let outcome = root.paint(&mut sink, FrameTime::ZERO);
        assert!(!outcome.needs_frame, "a read-only field paints at rest");
        assert!(
            root.ime_state().is_none(),
            "the published inactive surface releases the session"
        );
        assert!(
            !root.is_focus_active(),
            "the root's focus mirror goes with it"
        );

        // Leg 2 — the first event that reaches the field releases the pod-level
        // focus path, and edits nothing on the way.
        root.event(&mut state, &ch("x"));
        assert!(
            !root.is_focus_active(),
            "the stranded focus path is released"
        );
        assert!(root.ime_state().is_none());
        assert_eq!(widget(&root).editor.text(), "");
    }

    #[test]
    fn read_only_paints_full_alpha_chrome_in_every_design_language() {
        // The first of the two dimming resolution points: `Chrome::resolve`,
        // observed here through the actual `paint` pass (not called directly),
        // so the assertion also proves `paint` feeds it `enabled` alone.
        let languages: Vec<(&str, Option<Theme>)> = vec![
            ("unthemed", None),
            ("neutral", Some(Theme::neutral())),
            ("translucent-role", Some(translucent_role_theme())),
        ];
        for (name, theme) in languages {
            let mut state = AppState::default();
            let mut logic = options_logic(true, false, true);
            let mut root = options_root(&mut logic, &mut state);
            if let Some(theme) = theme.clone() {
                root.set_theme(Box::new(theme));
            }
            let live_border = Chrome::resolve(theme.as_ref(), true).border;
            let rec = paint_chrome(&mut root);
            assert_eq!(
                rec.rrects[0], live_border,
                "{name}: a read-only field's idle border matches the fully-\
                 enabled resolved border — not `DISABLED_CONTAINER_ALPHA`-dimmed"
            );
        }
    }

    #[test]
    fn read_only_paints_full_alpha_layout_baked_glyph_color() {
        // The second of the two dimming resolution points:
        // `effective_style`'s LAYOUT-time bake.
        let mut state = AppState {
            value: "hi".to_string(),
            ..AppState::default()
        };
        let mut logic = options_logic(true, false, true);
        let mut root = options_root(&mut logic, &mut state);
        root.set_theme(Box::new(Theme::neutral()));
        root.layout(Size::new(300.0, 200.0));

        assert_eq!(
            painted_text_color(&mut root),
            Theme::neutral().scheme().on_surface,
            "a read-only field's glyphs stay full-alpha on_surface, not dimmed"
        );
    }

    #[test]
    fn read_only_paints_full_alpha_unthemed_fallback_glyph_color() {
        let mut state = AppState {
            value: "hi".to_string(),
            ..AppState::default()
        };
        let mut logic = options_logic(true, false, true);
        let mut root = options_root(&mut logic, &mut state);
        assert_eq!(
            painted_text_color(&mut root),
            Color::BLACK,
            "with no theme threaded, read-only stays the undimmed black fallback"
        );
    }

    #[test]
    fn switching_a_read_only_field_to_live_shows_no_alpha_change() {
        // The motivating case: a static mock rendered read-only then switched
        // interactive at a live handoff must show **no** alpha change at
        // either resolution point — the pop `enabled(false)` would have
        // produced.
        let mut state = AppState {
            value: "hi".to_string(),
            ..AppState::default()
        };
        let mut read_only_logic = options_logic(true, false, true);
        let mut root = options_root(&mut read_only_logic, &mut state);
        let before_glyph = painted_text_color(&mut root);
        let before_border = paint_chrome(&mut root).rrects[0];

        let mut live_logic = options_logic(true, false, false);
        root.rebuild(&mut live_logic, &mut state);
        root.layout(Size::new(300.0, 200.0));
        let after_glyph = painted_text_color(&mut root);
        let after_border = paint_chrome(&mut root).rrects[0];

        assert_eq!(
            before_glyph, after_glyph,
            "glyph color must not change across the read-only -> live handoff"
        );
        assert_eq!(
            before_border, after_border,
            "border color must not change across the read-only -> live handoff"
        );
        assert_eq!(
            before_glyph,
            Color::BLACK,
            "sanity: read-only starts undimmed"
        );
    }

    #[test]
    fn read_only_reports_read_only_not_disabled_semantics() {
        let mut state = AppState::default();
        let mut logic = options_logic(true, false, true);
        let root = options_root(&mut logic, &mut state);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::TextInput)
            .expect("a text field node");
        assert!(
            node.is_read_only(),
            "a read-only field says so to a11y, distinct from disabled"
        );
        assert!(
            !node.is_disabled(),
            "read-only is not the same claim as disabled"
        );
    }

    #[test]
    fn disabled_wins_semantics_over_read_only_when_both_are_set() {
        let mut state = AppState::default();
        let mut logic = options_logic(false, false, true);
        let root = options_root(&mut logic, &mut state);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::TextInput)
            .expect("a text field node");
        assert!(node.is_disabled(), "disabled is the stronger claim");
        assert!(
            !node.is_read_only(),
            "disabled and read-only are never both reported"
        );
    }

    #[test]
    fn read_only_obscured_field_stays_password_role_and_never_focuses() {
        // Orthogonality: `read_only` never touches masking or the IME
        // content-type hint, it just keeps the field from ever actually
        // focusing (so there is no active surface to publish a hint on).
        let mut state = AppState {
            value: "hunter2".to_string(),
            ..AppState::default()
        };
        let mut logic = options_logic(true, true, true);
        let mut root = options_root(&mut logic, &mut state);

        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::PasswordInput)
            .expect("read-only + obscured still contributes Role::PasswordInput");
        assert_eq!(node.value(), Some("\u{2022}".repeat(7).as_str()));
        assert!(node.is_read_only());

        let outcome = root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        assert!(!outcome.handled);
        assert!(!root.is_focus_active());
        assert!(root.ime_state().is_none());
    }

    // --- obscured(true) ---

    #[test]
    fn mask_offsets_map_1_to_1_per_char_across_a_multi_byte_grapheme() {
        // "a😀b": 1 + 4 + 1 real bytes; masked "•••" is 3 × 3 bytes.
        let text = "a\u{1F600}b";
        assert_eq!(mask_text(text), "\u{2022}\u{2022}\u{2022}");
        for (real, masked) in [(0, 0), (1, 3), (5, 6), (6, 9)] {
            assert_eq!(real_to_masked(text, real), masked, "real {real} -> masked");
            assert_eq!(
                masked_to_real(text, masked),
                real,
                "masked {masked} -> real"
            );
        }
        // Interior offsets snap back to the enclosing character's start, both
        // ways (the emoji spans real bytes 1..5 and masked bytes 3..6).
        assert_eq!(real_to_masked(text, 3), 3, "mid-emoji snaps to its start");
        assert_eq!(masked_to_real(text, 4), 1, "mid-mask snaps to its start");
        // A newline is preserved so a multi-line field keeps its line count.
        assert_eq!(mask_text("a\nb"), "\u{2022}\n\u{2022}");
    }

    #[test]
    fn obscured_paints_bullets_and_keeps_the_real_value() {
        /// Records the glyph ids of every painted run, in order.
        #[derive(Default)]
        struct GlyphIdRecorder {
            ids: Vec<u16>,
        }
        impl PaintScene for GlyphIdRecorder {
            fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
            fn fill_rounded_rect(&mut self, _o: Point, _s: Size, _r: f64, _c: Color) {}
            fn draw_text(&mut self, _o: Point, _t: &str) {}
            fn draw_glyph_run(&mut self, run: frust_scene::GlyphRun) {
                self.ids.extend(run.glyphs.iter().map(|g| g.id as u16));
            }
        }

        fn ids(root: &mut RenderRoot<AppState, TextInputView<AppState>>) -> Vec<u16> {
            let mut rec = GlyphIdRecorder::default();
            root.paint(&mut rec, FrameTime::ZERO);
            rec.ids
        }

        // An obscured field holding "abc" must paint exactly what a plain field
        // holding "•••" paints — and nothing of what "abc" paints.
        let mut secret_state = AppState {
            value: "abc".to_string(),
            ..AppState::default()
        };
        let mut secret_logic = options_logic(true, true, false);
        let mut secret = options_root(&mut secret_logic, &mut secret_state);

        let mut bullets_state = AppState {
            value: "\u{2022}\u{2022}\u{2022}".to_string(),
            ..AppState::default()
        };
        let mut plain_logic = options_logic(true, false, false);
        let mut bullets = options_root(&mut plain_logic, &mut bullets_state);

        let mut clear_state = AppState {
            value: "abc".to_string(),
            ..AppState::default()
        };
        let mut clear = options_root(&mut plain_logic, &mut clear_state);

        assert_eq!(
            ids(&mut secret),
            ids(&mut bullets),
            "an obscured field paints bullets"
        );
        assert_ne!(
            ids(&mut secret),
            ids(&mut clear),
            "…and not the real characters"
        );
        assert_eq!(
            widget(&secret).editor.text(),
            "abc",
            "the model text is untouched by masking"
        );
    }

    #[test]
    fn obscured_measures_from_the_masked_text_not_the_real_one() {
        // Masking at glyph-emission time only would leave the field measured
        // from the real text: an 'i'-heavy secret would size like 'i's while
        // painting bullets. The mirror is what gets measured, so an obscured
        // field's width matches the equivalent bullet string's.
        let mut secret_state = AppState {
            value: "iiiiiiiiii".to_string(),
            ..AppState::default()
        };
        let mut secret_logic = options_logic(true, true, false);
        let secret = options_root(&mut secret_logic, &mut secret_state);

        let mut bullets_state = AppState {
            value: "\u{2022}".repeat(10),
            ..AppState::default()
        };
        let mut plain_logic = options_logic(true, false, false);
        let bullets = options_root(&mut plain_logic, &mut bullets_state);

        let masked_w = widget(&secret).display().layout_size().width;
        let bullets_w = widget(&bullets).display().layout_size().width;
        let real_w = widget(&secret).editor.layout_size().width;
        assert_eq!(masked_w, bullets_w, "measured from the masked mirror");
        assert!(
            masked_w > real_w,
            "sanity: bullets are wider than 'i's ({masked_w} vs {real_w})"
        );
    }

    #[test]
    fn obscured_editing_matches_unobscured_across_a_multi_byte_grapheme() {
        // The masking must not disturb the editing arithmetic: run the same
        // key sequence on an obscured and a clear field and require identical
        // editing state at every step, including over a 4-byte emoji.
        let mut secret_state = AppState::default();
        let mut secret_logic = options_logic(true, true, false);
        let mut secret = options_root(&mut secret_logic, &mut secret_state);
        let mut clear_state = AppState::default();
        let mut clear_logic = options_logic(true, false, false);
        let mut clear = options_root(&mut clear_logic, &mut clear_state);

        let plain = Modifiers::default();
        let meta = Modifiers {
            meta: true,
            ..Modifiers::default()
        };
        let events = vec![
            pointer(PointerPhase::Down, 10.0, 10.0),
            ch("a"),
            ch("\u{1F600}"),
            ch("b"),
            named(NamedKey::ArrowLeft, plain),
            named(NamedKey::Backspace, plain),
            named(NamedKey::End, plain),
            named(NamedKey::Backspace, plain),
            InputEvent::Key(KeyEvent {
                key: Key::Character("a".to_string()),
                modifiers: meta,
                repeat: false,
            }),
        ];
        for (i, event) in events.iter().enumerate() {
            secret.event(&mut secret_state, event);
            clear.event(&mut clear_state, event);
            assert_eq!(
                widget(&secret).editor.editing_state_bytes(),
                widget(&clear).editor.editing_state_bytes(),
                "editing state diverged at step {i}"
            );
        }
        // The emoji was deleted as one grapheme in both, and the app saw the
        // real text throughout.
        assert_eq!(secret_state.value, "a");
        assert_eq!(secret_state.value, clear_state.value);
        assert_eq!(secret_state.changes, clear_state.changes);
    }

    #[test]
    fn obscured_caret_rect_follows_the_masked_layout() {
        // The caret must sit where the *bullets* end, not where the real text
        // would have ended.
        let mut secret_state = AppState {
            value: "iiii".to_string(),
            ..AppState::default()
        };
        let mut secret_logic = options_logic(true, true, false);
        let mut secret = options_root(&mut secret_logic, &mut secret_state);
        secret.event(&mut secret_state, &pointer(PointerPhase::Down, 290.0, 10.0));
        let masked_caret = secret
            .ime_state()
            .expect("focused")
            .caret
            .expect("a caret rect");

        let mut bullets_state = AppState {
            value: "\u{2022}".repeat(4),
            ..AppState::default()
        };
        let mut plain_logic = options_logic(true, false, false);
        let mut bullets = options_root(&mut plain_logic, &mut bullets_state);
        bullets.event(
            &mut bullets_state,
            &pointer(PointerPhase::Down, 290.0, 10.0),
        );
        let bullets_caret = bullets
            .ime_state()
            .expect("focused")
            .caret
            .expect("a caret rect");

        assert_eq!(
            masked_caret, bullets_caret,
            "the obscured caret tracks the masked glyphs"
        );
        assert_eq!(
            secret.ime_state().expect("focused").editing.text,
            "iiii",
            "the IME surface still carries the real text (see the module docs)"
        );
    }

    #[test]
    fn obscured_tap_places_the_caret_from_the_masked_hit_test() {
        // A tap between the first and second bullet must land on real byte 1,
        // resolved against the masked advances (bullets are much wider than
        // 'i's, so hit-testing the real layout would overshoot to the end).
        let mut state = AppState {
            value: "iiiiiiii".to_string(),
            ..AppState::default()
        };
        let mut logic = options_logic(true, true, false);
        let mut root = options_root(&mut logic, &mut state);
        // Mid-way through the first mask glyph -> caret before or after char 0.
        let half_bullet = widget(&root).display().layout_size().width / 16.0;
        root.event(
            &mut state,
            &pointer(PointerPhase::Down, PAD_X + half_bullet, 10.0),
        );
        let extent = widget(&root).editor.editing_state_bytes().extent;
        assert!(
            extent <= 1,
            "a tap inside the first mask glyph lands at byte 0 or 1, got {extent}"
        );
    }

    #[test]
    fn obscured_reports_password_role_with_a_masked_value() {
        let mut state = AppState {
            value: "hunter2".to_string(),
            ..AppState::default()
        };
        let mut logic = options_logic(true, true, false);
        let root = options_root(&mut logic, &mut state);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::PasswordInput)
            .expect("an obscured field contributes a Role::PasswordInput node");
        assert_eq!(
            node.value(),
            Some("\u{2022}".repeat(7).as_str()),
            "the a11y value is masked too — a client reads it verbatim"
        );

        // Enabled + clear stays a plain TextInput node carrying the real value.
        let mut plain_logic = options_logic(true, false, false);
        let plain = options_root(&mut plain_logic, &mut state);
        let update = plain.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::TextInput)
            .expect("a clear field stays Role::TextInput");
        assert_eq!(node.value(), Some("hunter2"));
        assert!(!node.is_disabled(), "an enabled field is not disabled");
    }

    #[test]
    fn disabled_reports_disabled_semantics() {
        let mut state = AppState::default();
        let mut logic = options_logic(false, false, false);
        let root = options_root(&mut logic, &mut state);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::TextInput)
            .expect("a text field node");
        assert!(node.is_disabled(), "a disabled field says so to a11y");
    }

    #[test]
    fn toggling_obscured_relayouts_and_keeps_the_value() {
        let mut state = AppState {
            value: "iiiiiiii".to_string(),
            ..AppState::default()
        };
        let mut clear_logic = options_logic(true, false, false);
        let mut root = options_root(&mut clear_logic, &mut state);
        let clear_width = widget(&root).display().layout_size().width;

        let mut secret_logic = options_logic(true, true, false);
        let flags = root.rebuild(&mut secret_logic, &mut state);
        assert!(
            flags.needs_layout(),
            "masking changes the measured text, so the flip must relayout"
        );
        root.layout(Size::new(300.0, 200.0));
        assert!(
            widget(&root).display().layout_size().width > clear_width,
            "the masked mirror is measured after the flip"
        );
        assert_eq!(widget(&root).editor.text(), "iiiiiiii");

        // …and back again.
        root.rebuild(&mut clear_logic, &mut state);
        root.layout(Size::new(300.0, 200.0));
        assert_eq!(widget(&root).display().layout_size().width, clear_width);
        assert!(widget(&root).mask_editor.is_none());
    }

    // --- content_type (widget half) ---
    //
    // These lock the widget's IME content-type mapping only — the leak this
    // closes lives at the platform seam, and no widget-level test can observe
    // whether a shell actually honours the hint. See the module docs' scope
    // boundary and `ImeContentType`'s own docs.

    #[test]
    fn obscured_publishes_password_content_type_across_focus_edit_and_refocus() {
        let mut state = AppState {
            value: "hunter2".to_string(),
            ..AppState::default()
        };
        let mut logic = options_logic(true, true, false);
        let mut root = options_root(&mut logic, &mut state);

        // Focus.
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        assert_eq!(
            root.ime_state().expect("focused").content_type,
            ImeContentType::Password,
            "an obscured field's very first published IME surface must already \
             be Password"
        );

        // Edit.
        root.event(&mut state, &ch("x"));
        assert_eq!(
            root.ime_state().expect("still focused").content_type,
            ImeContentType::Password,
            "content type must not drop on edit"
        );

        // Blur, then re-focus.
        root.event(&mut state, &named(NamedKey::Escape, Modifiers::default()));
        assert!(root.ime_state().is_none(), "blur unpublishes the surface");
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        assert_eq!(
            root.ime_state().expect("re-focused").content_type,
            ImeContentType::Password,
            "content type must be correct again on re-focus, not just the first time"
        );
    }

    #[test]
    fn unobscured_field_publishes_the_default_no_hint_content_type() {
        // No behaviour change for the common case: a plain field's published
        // surface keeps the `Normal` default it always had.
        let mut state = AppState::default();
        let mut root = harness(&mut state);

        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        assert_eq!(
            root.ime_state().expect("focused").content_type,
            ImeContentType::Normal
        );

        root.event(&mut state, &ch("h"));
        assert_eq!(
            root.ime_state().expect("still focused").content_type,
            ImeContentType::Normal
        );
    }

    #[test]
    fn obscured_content_type_is_correct_on_the_first_publication_after_focus() {
        // The ordering guarantee the security fix rests on: a field that starts
        // `Normal` and flips to `Password` a frame later has already leaked to
        // the platform IME (see the module docs). Assert there is no such
        // window by checking the *very first* `ImeState` a freshly built,
        // never-before-focused obscured field emits — a single `Down` event,
        // nothing before it.
        let mut state = AppState {
            value: "hunter2".to_string(),
            ..AppState::default()
        };
        let mut logic = options_logic(true, true, false);
        let mut root = options_root(&mut logic, &mut state);
        assert!(
            root.ime_state().is_none(),
            "an unfocused field publishes nothing yet"
        );

        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));

        let ime = root
            .ime_state()
            .expect("focusing publishes the first IME surface");
        assert_eq!(
            ime.content_type,
            ImeContentType::Password,
            "the first-ever publication for an obscured field must already \
             carry Password"
        );
        assert_eq!(
            ime.editing.text, "hunter2",
            "sanity: this is the real first publication, not a stale one"
        );
    }
}
