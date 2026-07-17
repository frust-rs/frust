//! The `TextInput` interactive widget (spec §6.4 / Phase 4B): a single-line
//! editable text field.
//!
//! [`text_input`] produces a [`TextInputView`] carrying the current `value`, a
//! `placeholder`, an `on_change` callback, and an optional `on_submit`. Like the
//! other interactive widgets it is a **controlled component** (see
//! `docs/CODE_STANDARDS.md`): it never owns the durable value. Each edit reports
//! the *requested* text through `on_change`, and the next `rebuild` reconciles
//! the app-confirmed `value` back into the underlying [`TextEditor`] (task 52) —
//! set-if-different, preserving the selection while the text is unchanged. The
//! content text's style (defaulting to 16px black) can be set with
//! [`TextInputView::text_style`], applied to the widget's [`TextEditor`] at
//! build time; it never touches the chrome color constants or padding/caret
//! sizing below.
//!
//! # Text context ownership
//!
//! Unlike the [`Text`](crate::TextView) leaf (which shapes against the shared
//! `TextContext` threaded through `LayoutCtx` during the layout pass), a
//! `TextInput` must apply edits *synchronously during the event pass*, where no
//! context is threaded. It therefore owns its own [`TextContext`] and drives the
//! editor through it — so `on_change`/the published [`ImeState`] observe the
//! fresh editing value immediately, and layout needs no threaded context (it
//! reads the editor's own refreshed metrics).
//!
//! # Focus, IME and blink
//!
//! A `Down` inside the field requests focus, places the caret, and publishes an
//! [`ImeState`] (task 51's focus/IME channel) so the shell can drive the platform
//! input method. Keyboard editing (`Key`) and IME composition/state-sync (`Ime`)
//! route down the focus path; after any edit the widget fires `on_change`, resets
//! the caret to visible, and republishes the IME surface. The caret blinks while
//! focused, its phase measured from the shared shell frame clock
//! ([`PaintCtx::frame_time`], spec §8 — no wall-clock reads in widget code) in
//! `paint` via [`PaintCtx::request_frame`] — the same animation contract the
//! scroll fling uses. An edit/focus during the (clockless) event pass flags the
//! blink for reset; the next paint records the blink epoch from `frame_time`.

use std::rc::Rc;

use forgekit_core::accesskit::Role;
use forgekit_core::{
    BoxConstraints, BuildCtx, ChangeFlags, EditingState, EventCtx, EventResult, FrameTime,
    ImeEvent, ImeState, InputEvent, Key, LayoutCtx, NamedKey, PaintCtx, PaintScene, PointerPhase,
    SemanticsCtx, View, Widget,
};
use forgekit_text::{EditOp, EditingStateBytes, TextContext, TextEditor, TextStyle, utf16_to_byte};
use forgekit_theme::Theme;
use kurbo::{Point, Rect, Size, Vec2};
use peniko::Color;

/// Corner radius of the field chrome, in logical px.
const RADIUS: f64 = 6.0;
/// Border thickness, in logical px.
const BORDER_W: f64 = 1.5;
/// Horizontal inner padding (chrome edge to text), in logical px.
const PAD_X: f64 = 8.0;
/// Vertical inner padding (chrome edge to text), in logical px.
const PAD_Y: f64 = 6.0;
/// Caret width, in logical px.
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
/// role; see task 07).
const SELECTION_ALPHA: f32 = 0.30;

/// The resolved text-field chrome colors. Themed (v1 simplification): background
/// `surface`, border `outline`, focus accent/caret `primary`, placeholder
/// `on_surface_variant`, selection `primary` at [`SELECTION_ALPHA`]. Unthemed:
/// the [`BG`]/[`BORDER`]/[`ACCENT`]/[`PLACEHOLDER`]/[`SELECTION`]/[`CARET`]
/// constants exactly, so a pre-theme app renders unchanged.
struct Chrome {
    bg: Color,
    border: Color,
    accent: Color,
    placeholder: Color,
    selection: Color,
    caret: Color,
}

impl Chrome {
    fn resolve(theme: Option<&Theme>) -> Self {
        match theme {
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
        }
    }
}

/// A view-held, typed text callback (erased on build).
type OnText<State> = Rc<dyn Fn(&mut State, String)>;

/// A declarative single-line text field. See the [module docs](self).
pub struct TextInputView<State: 'static> {
    value: String,
    placeholder: String,
    text_style: TextStyle,
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
    /// Default is unchanged (16px black). Does not affect the chrome colors
    /// or padding/caret constants.
    pub fn text_style(mut self, text_style: TextStyle) -> Self {
        self.text_style = text_style;
        self
    }
}

/// The retained widget for a [`TextInputView`].
pub struct TextInputWidget {
    /// The editing engine (task 52). Driven through the widget-owned `text_ctx`.
    editor: TextEditor,
    /// The widget's own font/layout context — see the [module docs](self).
    text_ctx: TextContext,
    style: TextStyle,
    placeholder: String,
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
    on_change: crate::ErasedArgCallback<String>,
    on_submit: Option<crate::ErasedArgCallback<String>>,
}

/// Whether `pos` (widget-local) lies within a `size`-sized field.
fn inside(pos: Point, size: Size) -> bool {
    pos.x >= 0.0 && pos.y >= 0.0 && pos.x < size.width && pos.y < size.height
}

impl TextInputWidget {
    /// The single-line text height from the editor's refreshed metrics, floored
    /// to a sensible line height for an empty field.
    fn content_height(&self) -> f64 {
        self.editor
            .layout_size()
            .height
            .max(self.style.size as f64 * 1.25)
    }

    /// The top-left of the text content within a `height`-tall field (vertically
    /// centered, never above the top padding).
    fn text_top(&self, height: f64) -> f64 {
        ((height - self.content_height()) / 2.0).max(PAD_Y)
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
        let offset = origin.to_vec2() + Vec2::new(PAD_X, self.text_top(size.height));
        let caret = self.editor.cursor_rect(CARET_W).map(|c| {
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
            (pos.x - PAD_X) as f32,
            (pos.y - self.text_top(height)) as f32,
        )
    }

    /// Handle a keyboard key event (already focus-gated by the caller).
    fn handle_key(
        &mut self,
        ctx: &mut EventCtx,
        key: &Key,
        modifiers: forgekit_core::Modifiers,
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
                        let text = self.editor.text().to_string();
                        if let Some(cb) = &mut self.on_submit {
                            cb(ctx, text);
                        }
                        ctx.request_redraw();
                    }
                    NamedKey::Escape => {
                        ctx.release_focus();
                        self.focused = false;
                        ctx.request_redraw();
                    }
                    // Single-line v1: vertical motion and Tab traversal are no-ops.
                    NamedKey::ArrowUp | NamedKey::ArrowDown | NamedKey::Tab => {
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
                // iOS Return contract (task 56 / RESEARCH §ios): the Return key on a
                // UITextInput arrives as `insertText("\n")` → `Commit("\n")`. On this
                // single-line widget a lone newline commit means SUBMIT, exactly like
                // `NamedKey::Enter` — it must never insert a literal '\n'.
                if s == "\n" || s == "\r" || s == "\r\n" {
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
        TextInputWidget {
            editor,
            text_ctx,
            style,
            placeholder: self.placeholder.clone(),
            focused: false,
            captured: false,
            blink_epoch: FrameTime::ZERO,
            blink_reset_pending: true,
            on_change: crate::erase_callback_arg(&self.on_change),
            on_submit: self
                .on_submit
                .as_ref()
                .map(crate::erase_callback_arg::<State, String>),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut TextInputWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        // Closures aren't comparable; reinstall the erased adapters unconditionally.
        element.on_change = crate::erase_callback_arg(&self.on_change);
        element.on_submit = self
            .on_submit
            .as_ref()
            .map(crate::erase_callback_arg::<State, String>);

        let mut flags = ChangeFlags::NONE;
        if prev.placeholder != self.placeholder {
            element.placeholder = self.placeholder.clone();
            flags |= ChangeFlags::PAINT;
        }
        // Controlled reconcile: adopt the app-confirmed value only when it differs
        // from what the editor currently holds, so an accepted edit leaves the
        // selection untouched and a rejected/normalized one is pulled back in.
        if self.value != element.editor.text() {
            element.set_controlled_value(&self.value);
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags
    }
}

impl Widget for TextInputWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            DEFAULT_WIDTH
        };
        let height = self.content_height() + 2.0 * PAD_Y;
        bc.constrain(Size::new(width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let origin = ctx.origin();
        let size = ctx.size();
        let chrome = Chrome::resolve(Theme::from_paint_ctx(ctx));

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
        // IME surface the blur cleared (review F1).
        let focused = ctx.has_focus();
        if self.focused && !focused {
            self.focused = false;
        }

        // Chrome: a border-colored rounded rect with an inset background fills in
        // as the frame (there is no stroke-rect primitive on `PaintScene`).
        let border_color = if focused {
            chrome.accent
        } else {
            chrome.border
        };
        scene.fill_rounded_rect(origin, size, RADIUS, border_color);
        scene.fill_rounded_rect(
            Point::new(origin.x + BORDER_W, origin.y + BORDER_W),
            Size::new(
                (size.width - 2.0 * BORDER_W).max(0.0),
                (size.height - 2.0 * BORDER_W).max(0.0),
            ),
            (RADIUS - BORDER_W).max(0.0),
            chrome.bg,
        );

        let text_origin = Point::new(origin.x + PAD_X, origin.y + self.text_top(size.height));

        if self.editor.text().is_empty() && !focused {
            // Placeholder: shaped on demand through the widget-owned context.
            if !self.placeholder.is_empty() {
                let ph_style = TextStyle::new(self.style.size, chrome.placeholder);
                let layout = self.text_ctx.layout(&self.placeholder, &ph_style, None);
                for run in layout.to_scene_runs(text_origin) {
                    scene.draw_glyph_run(run);
                }
            }
        } else {
            let off = text_origin.to_vec2();
            // Selection highlights sit behind the glyphs.
            for r in self.editor.selection_rects() {
                scene.fill_rect(
                    Point::new(r.x0 + off.x, r.y0 + off.y),
                    Size::new(r.width(), r.height()),
                    chrome.selection,
                );
            }
            for run in self.editor.to_scene_runs(text_origin) {
                scene.draw_glyph_run(run);
            }
        }

        // Caret: blink while focused. Requesting a frame keeps the desktop shell's
        // wait-loop scheduling paints so the blink animates (the mobile shells'
        // continuous loops already do). At rest (unfocused) we stop signalling.
        if focused {
            ctx.request_frame();
            // Republish the IME surface every painted frame while focused, so a
            // controlled change applied by a rebuild (a submit clearing the
            // field) refreshes the shell-facing state the event pass would
            // otherwise leave stale — the mobile IME mirror relies on this to
            // observe the clear (see `PaintCtx::publish_ime_state`).
            ctx.publish_ime_state(self.current_ime_state(origin, size));
            if self.caret_visible_at(now)
                && let Some(c) = self.editor.cursor_rect(CARET_W)
            {
                let off = text_origin.to_vec2();
                scene.fill_rect(
                    Point::new(c.x0 + off.x, c.y0 + off.y),
                    Size::new(c.width(), c.height()),
                    chrome.caret,
                );
            }
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        match event {
            InputEvent::Pointer(p) => match p.phase {
                PointerPhase::Down => {
                    if inside(p.position, ctx.size()) {
                        ctx.request_focus();
                        ctx.capture_pointer();
                        self.focused = true;
                        self.captured = true;
                        let (x, y) = self.editor_point(p.position, ctx.size().height);
                        self.apply_edit(
                            ctx,
                            EditOp::MoveToPoint {
                                x,
                                y,
                                select: false,
                            },
                        );
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
                    self.apply_edit(ctx, EditOp::MoveToPoint { x, y, select: true });
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
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // A single-line TextInput node exposing its current text as `value`.
        // accesskit tracks focus at the tree level, so a focused field records
        // itself as the pass's focus node rather than carrying a per-node flag.
        let id = ctx.push_node(Role::TextInput, |node| {
            node.set_value(self.editor.text());
        });
        if self.focused {
            ctx.set_focused(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forgekit_core::{FrameTime, KeyEvent, Modifiers, PointerButton, PointerEvent, RenderRoot};
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

        // The whole emoji code point is removed as a unit (grapheme integrity via
        // task 52), not a single byte.
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

        // Once focused, paint pumps the blink and asks for the next frame.
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        assert!(
            root.paint(&mut sink, FrameTime::ZERO).needs_frame,
            "a focused field blinks its caret"
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
        // Two-frame blink test (task 07): the caret is painted in the visible half
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

    // --- Themed chrome (task 07) ---

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
        fn draw_glyph_run(&mut self, _run: forgekit_scene::GlyphRun) {}
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
        root.set_theme(Box::new(Theme::m3_baseline()));
        let theme = Theme::m3_baseline();
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

    // --- Nested-blur regression (review F1) ---
    //
    // A `TextInput` nested in a `Column` beside a `Button`. Focusing the field
    // then tapping the sibling is a *container-routed* blur: `route_event`
    // clears the field pod's recorded focus path, but the widget's `event()` is
    // never called on that dispatch. Pre-fix, the widget-internal `focused` flag
    // stayed set, so the next `paint` republished the (already-cleared) IME
    // surface and pumped a caret-blink continuation frame — resurrecting the
    // dismissed keyboard and keeping the desktop wait-loop spinning forever.
    // The fix threads the pod's focus into paint (`PaintCtx::has_focus`) so the
    // widget observes the blur and converges.

    #[derive(Default)]
    struct NestedState {
        value: String,
    }

    fn nested_logic(state: &mut NestedState) -> crate::FlexView<NestedState> {
        use forgekit_core::any;
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
        let mut tcx = forgekit_text::TextContext::new();
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
        // never called on this dispatch.
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 45.0));

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
    /// `has_focus` seeding must cover (re-review N1).
    fn deep_nested_logic(state: &mut NestedState) -> crate::FlexView<NestedState> {
        use forgekit_core::any;
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
        let mut tcx = forgekit_text::TextContext::new();
        root.layout_with_text(Size::new(300.0, 200.0), &mut tcx as &mut dyn Any);

        // Focus the field (full Down+Up tap — the Up releases capture so the
        // blur Down hit-tests afresh; see nested_blur_clears_ime_and_idles_paint).
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        root.event(&mut state, &pointer(PointerPhase::Up, 10.0, 10.0));
        assert!(root.is_focus_active());
        assert!(root.ime_state().is_some());

        // Blur via the OUTER-level sibling: route_event clears the focused link
        // at the outer Column (the inner Column's pod); the field's own pod flag
        // two levels down stays stale.
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 45.0));
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

    /// A no-op paint sink for `needs_frame` assertions (glyph/rect output is not
    /// under test here).
    struct NullScene;
    impl PaintScene for NullScene {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
    }
}
