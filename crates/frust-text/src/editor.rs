//! [`TextEditor`]: the editing engine behind the `TextInput` widget and the
//! platform IME bridges (spec §4, §9 / Phase 4B).
//!
//! Wraps parley 0.11's [`parley::PlainEditor`] behind a renderer-agnostic API:
//! all mutation flows through [`TextEditor::apply`] (an [`EditOp`] plus a
//! borrowed [`TextContext`], from which the transient
//! [`parley::PlainEditorDriver`] borrow-triple is constructed), and all readback
//! is expressed in `kurbo`/`frust-scene` types plus the two editing-state
//! DTOs defined here — **no parley type leaks through the public API**
//! (scene-layer purity, see `docs/ARCHITECTURE.md`).
//!
//! # Two composition paths
//!
//! - **Desktop / winit** drives composition through parley's own IME machinery
//!   ([`EditOp::Compose`]/[`EditOp::FinishCompose`]/[`EditOp::ClearCompose`]):
//!   parley owns the preedit buffer and reports it via `raw_compose`.
//! - **Android / iOS** own composition against a platform-side mirror buffer and
//!   push the whole editing value ([`EditOp::ApplyEditingState`]) — a state-sync
//!   model, not op-forwarding (the Flutter-canonical contract; see
//!   `research/RESEARCH.md`). On this path parley's compose machinery is *not*
//!   used: the platform is authoritative for the composing region, so we track
//!   it in [`TextEditor::platform_composing`] purely for paint styling and echo
//!   it back out in the editing state. A value-equality short-circuit guards the
//!   IMM feedback loop (re-applying an identical state is a no-op).
//!
//! # Coordinate / unit conventions
//!
//! Selection and composing offsets are **byte** offsets into the text buffer at
//! the parley boundary and in [`EditingStateBytes`]; the platform bridges speak
//! UTF-16 code-unit offsets, so [`EditingState`] (and the [`byte_to_utf16`] /
//! [`utf16_to_byte`] helpers) do the conversion in one concentrated, tested
//! place. Geometry is Y-down logical pixels (parley 0.11's convention).
//!
//! # Single-line vs. wrapped multi-line
//!
//! The editor is built with no wrapping width, so it is single-line by default
//! (one visual line, horizontal overflow). Calling
//! [`set_wrap_width(Some(px))`](TextEditor::set_wrap_width) switches it into a
//! wrapped multi-line mode: text soft-wraps at `px`,
//! [`layout_size`](TextEditor::layout_size) reports the full wrapped height, a
//! literal newline can be inserted with [`EditOp::InsertNewline`], and the
//! [`EditOp::MoveUp`]/[`EditOp::MoveDown`] vertical caret motions (no-ops while
//! single-line, since there is only one line) become real line-to-line motion
//! through parley's line structure. Passing `None` restores single-line mode.
//! Selection/IME byte↔UTF-16 indexing is unaffected by newlines — a `'\n'` is
//! one byte / one UTF-16 unit and flows through the same conversion helpers.

use std::ops::Range;

use kurbo::{Point, Rect, Size};
use parley::{BoundingBox, PlainEditor, StyleProperty};
use peniko::Brush;

use frust_scene::GlyphRun;

use crate::context::TextContext;
use crate::style::{
    TextStyle, to_parley_family, to_parley_line_height, to_parley_style, to_parley_weight,
};

/// A single editing command applied to a [`TextEditor`].
///
/// Movement variants carry a `select` flag: `false` moves the caret
/// (collapsing any selection), `true` extends the selection to the new
/// position (parley's `move_*`/`select_*` twins).
#[derive(Clone, Debug, PartialEq)]
pub enum EditOp {
    /// Insert text, replacing the current selection.
    Insert(String),
    /// Insert a literal newline (`'\n'`), replacing the current selection.
    ///
    /// Only meaningful in wrapped multi-line mode (see
    /// [`TextEditor::set_wrap_width`]); a single-line field routes Enter to
    /// submit instead of emitting this op.
    InsertNewline,
    /// Delete the grapheme before the caret (Backspace), or the selection.
    Backdelete,
    /// Delete the grapheme after the caret (Delete/Forward-delete), or the
    /// selection.
    Delete,
    /// Move/extend one grapheme left (visual order).
    MoveLeft { select: bool },
    /// Move/extend one grapheme right (visual order).
    MoveRight { select: bool },
    /// Move/extend one line up.
    MoveUp { select: bool },
    /// Move/extend one line down.
    MoveDown { select: bool },
    /// Move/extend one word left.
    MoveWordLeft { select: bool },
    /// Move/extend one word right.
    MoveWordRight { select: bool },
    /// Move/extend to the start of the line (Home).
    Home { select: bool },
    /// Move/extend to the end of the line (End).
    End { select: bool },
    /// Select the whole buffer.
    SelectAll,
    /// Collapse the selection to its focus (drop the anchor).
    CollapseSelection,
    /// Move the caret to (or extend the selection to) a layout-local point.
    MoveToPoint { x: f32, y: f32, select: bool },
    /// Select the word under a layout-local point (double-click / long-press).
    SelectWordAtPoint { x: f32, y: f32 },
    /// Begin/update an IME preedit with `text` and an optional caret range
    /// (byte offsets *within* `text`). Desktop/winit composition path.
    Compose {
        text: String,
        cursor: Option<(usize, usize)>,
    },
    /// Commit the current IME preedit into the buffer. Desktop/winit path.
    FinishCompose,
    /// Discard the current IME preedit. Desktop/winit path.
    ClearCompose,
    /// Replace the whole editing value from a platform mirror (Android/iOS
    /// state-sync path). Byte offsets; see [`EditingStateBytes`].
    ApplyEditingState(EditingStateBytes),
}

/// The editing value in **byte** offsets — the parley-native representation and
/// the shape [`EditOp::ApplyEditingState`] consumes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditingStateBytes {
    /// The full text buffer (including any composing preedit).
    pub text: String,
    /// Selection anchor, byte offset into `text`.
    pub base: usize,
    /// Selection focus, byte offset into `text`.
    pub extent: usize,
    /// Composing (preedit) region as a byte range, or `None` when not
    /// composing.
    pub composing: Option<Range<usize>>,
}

/// The editing value in **UTF-16** code-unit offsets — the Flutter-canonical
/// shape the Android/iOS bridges exchange (`-1` denotes "none").
///
/// Selection offsets are always valid (`base == extent` when the selection is
/// collapsed to a caret). Composing offsets are `-1` when not composing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditingState {
    /// The full text buffer (including any composing preedit).
    pub text: String,
    /// Selection anchor, UTF-16 offset into `text`.
    pub selection_base: i32,
    /// Selection focus, UTF-16 offset into `text`.
    pub selection_extent: i32,
    /// Composing region start (UTF-16), or `-1` when not composing.
    pub composing_base: i32,
    /// Composing region end (UTF-16), or `-1` when not composing.
    pub composing_extent: i32,
}

/// The editing engine wrapping a parley [`PlainEditor`].
///
/// Constructed once per focused `TextInput`; driven by [`apply`](Self::apply)
/// and read back through the geometry / editing-state accessors. The brush type
/// is fixed to [`peniko::Brush`] so glyph-run conversion is shared with
/// [`crate::TextLayout`].
pub struct TextEditor {
    editor: PlainEditor<Brush>,
    /// Platform-owned composing region (byte range), tracked for paint styling
    /// on the [`EditOp::ApplyEditingState`] path only. `None` on the desktop
    /// path, where parley's own `raw_compose` is authoritative.
    platform_composing: Option<Range<usize>>,
}

impl TextEditor {
    /// Builds an empty single-line editor styled by `style`.
    ///
    /// Every knob on `style` (family, weight, style, size, color,
    /// letter-spacing, line-height) is applied as an edit-style default,
    /// mirroring [`crate::TextContext::layout`]. No wrapping width is set —
    /// v1 is single-line.
    pub fn new(style: &TextStyle) -> Self {
        let mut editor = PlainEditor::<Brush>::new(style.size);
        // Single-line by default: no wrapping width. A widget opts into wrapped
        // multi-line editing with `set_wrap_width(Some(px))`.
        editor.set_width(None);
        let styles = editor.edit_styles();
        styles.insert(StyleProperty::FontFamily(to_parley_family(&style.family)));
        styles.insert(StyleProperty::FontWeight(to_parley_weight(style.weight)));
        styles.insert(StyleProperty::FontStyle(to_parley_style(style.style)));
        styles.insert(StyleProperty::LetterSpacing(style.letter_spacing));
        styles.insert(StyleProperty::LineHeight(to_parley_line_height(
            style.line_height,
        )));
        styles.insert(StyleProperty::Brush(Brush::Solid(style.color)));
        Self {
            editor,
            platform_composing: None,
        }
    }

    /// Applies one editing command, threading the shell-owned font/layout
    /// contexts through the transient parley driver.
    ///
    /// The layout is refreshed at the end so the geometry accessors
    /// ([`cursor_rect`](Self::cursor_rect), [`selection_rects`](Self::selection_rects),
    /// [`to_scene_runs`](Self::to_scene_runs), …) observe up-to-date positions
    /// without needing a context of their own.
    pub fn apply(&mut self, op: EditOp, ctx: &mut TextContext) {
        let (font_cx, layout_cx) = ctx.driver_contexts();

        match op {
            EditOp::ApplyEditingState(state) => {
                // State-sync path: platform owns composition; parley's compose
                // machinery is intentionally bypassed (see module docs).
                //
                // Value-equality short-circuit: re-applying an identical state
                // is a no-op, guarding the IMM feedback loop.
                if self.editing_state_bytes() == state {
                    return;
                }
                if self.editor.raw_text() != state.text {
                    self.editor.set_text(&state.text);
                }
                {
                    let mut drv = self.editor.driver(font_cx, layout_cx);
                    drv.select_byte_range(state.base, state.extent);
                }
                self.platform_composing = state.composing;
            }
            other => {
                let mut drv = self.editor.driver(font_cx, layout_cx);
                match other {
                    EditOp::Insert(s) => drv.insert_or_replace_selection(&s),
                    EditOp::InsertNewline => drv.insert_or_replace_selection("\n"),
                    EditOp::Backdelete => drv.backdelete(),
                    EditOp::Delete => drv.delete(),
                    EditOp::MoveLeft { select } => {
                        sel(&mut drv, select, |d| d.move_left(), |d| d.select_left())
                    }
                    EditOp::MoveRight { select } => {
                        sel(&mut drv, select, |d| d.move_right(), |d| d.select_right())
                    }
                    EditOp::MoveUp { select } => {
                        sel(&mut drv, select, |d| d.move_up(), |d| d.select_up())
                    }
                    EditOp::MoveDown { select } => {
                        sel(&mut drv, select, |d| d.move_down(), |d| d.select_down())
                    }
                    EditOp::MoveWordLeft { select } => sel(
                        &mut drv,
                        select,
                        |d| d.move_word_left(),
                        |d| d.select_word_left(),
                    ),
                    EditOp::MoveWordRight { select } => sel(
                        &mut drv,
                        select,
                        |d| d.move_word_right(),
                        |d| d.select_word_right(),
                    ),
                    EditOp::Home { select } => sel(
                        &mut drv,
                        select,
                        |d| d.move_to_line_start(),
                        |d| d.select_to_line_start(),
                    ),
                    EditOp::End { select } => sel(
                        &mut drv,
                        select,
                        |d| d.move_to_line_end(),
                        |d| d.select_to_line_end(),
                    ),
                    EditOp::SelectAll => drv.select_all(),
                    EditOp::CollapseSelection => drv.collapse_selection(),
                    EditOp::MoveToPoint { x, y, select } => {
                        if select {
                            drv.extend_selection_to_point(x, y);
                        } else {
                            drv.move_to_point(x, y);
                        }
                    }
                    EditOp::SelectWordAtPoint { x, y } => drv.select_word_at_point(x, y),
                    EditOp::Compose { text, cursor } => drv.set_compose(&text, cursor),
                    EditOp::FinishCompose => drv.finish_compose(),
                    EditOp::ClearCompose => drv.clear_compose(),
                    // Handled in the outer arm; unreachable here.
                    EditOp::ApplyEditingState(_) => unreachable!(),
                }
            }
        }

        self.editor.refresh_layout(font_cx, layout_cx);
    }

    /// Sets (or clears) the soft-wrap width, switching between wrapped
    /// multi-line and single-line modes.
    ///
    /// `Some(px)` soft-wraps the text at `px` logical pixels and makes vertical
    /// caret motion ([`EditOp::MoveUp`]/[`EditOp::MoveDown`]) and newline
    /// insertion ([`EditOp::InsertNewline`]) meaningful; `None` restores the
    /// single-line default. The layout is refreshed immediately so
    /// [`layout_size`](Self::layout_size) and the geometry accessors observe the
    /// new wrapping without waiting for the next [`apply`](Self::apply).
    pub fn set_wrap_width(&mut self, width: Option<f32>, ctx: &mut TextContext) {
        self.editor.set_width(width);
        let (font_cx, layout_cx) = ctx.driver_contexts();
        self.editor.refresh_layout(font_cx, layout_cx);
    }

    /// The number of visual lines in the current layout (1 for a non-empty
    /// single-line field; more once wrapping or explicit newlines split it).
    ///
    /// Returns 0 before the first layout refresh.
    pub fn line_count(&self) -> usize {
        self.editor
            .try_layout()
            .map(|layout| layout.len())
            .unwrap_or(0)
    }

    /// The current buffer text, including any composing preedit.
    pub fn text(&self) -> &str {
        self.editor.raw_text()
    }

    /// The measured size of the laid-out text (Y-down logical pixels).
    ///
    /// Returns [`Size::ZERO`] before the first [`apply`](Self::apply) has
    /// refreshed the layout.
    pub fn layout_size(&self) -> Size {
        match self.editor.try_layout() {
            Some(layout) => Size::new(layout.width() as f64, layout.height() as f64),
            None => Size::ZERO,
        }
    }

    /// The current editing value in byte offsets.
    pub fn editing_state_bytes(&self) -> EditingStateBytes {
        let selection = self.editor.raw_selection();
        EditingStateBytes {
            text: self.editor.raw_text().to_string(),
            base: selection.anchor().index(),
            extent: selection.focus().index(),
            composing: self.composing_range(),
        }
    }

    /// The current editing value in UTF-16 offsets (Flutter-canonical shape).
    pub fn editing_state_utf16(&self) -> EditingState {
        let text = self.editor.raw_text();
        let selection = self.editor.raw_selection();
        let (composing_base, composing_extent) = match self.composing_range() {
            Some(range) => (
                byte_to_utf16(text, range.start) as i32,
                byte_to_utf16(text, range.end) as i32,
            ),
            None => (-1, -1),
        };
        EditingState {
            text: text.to_string(),
            selection_base: byte_to_utf16(text, selection.anchor().index()) as i32,
            selection_extent: byte_to_utf16(text, selection.focus().index()) as i32,
            composing_base,
            composing_extent,
        }
    }

    /// The caret rectangle at the current focus, `caret_width` wide.
    ///
    /// `None` when the caret is hidden (e.g. the IME requested it).
    pub fn cursor_rect(&self, caret_width: f32) -> Option<Rect> {
        self.editor.cursor_geometry(caret_width).map(bbox_to_rect)
    }

    /// The rectangles covering the current (non-collapsed) selection.
    pub fn selection_rects(&self) -> Vec<Rect> {
        self.editor
            .selection_geometry()
            .into_iter()
            .map(|(bbox, _line)| bbox_to_rect(bbox))
            .collect()
    }

    /// The area bounding the text currently being edited — a placement hint for
    /// the platform IME candidate box.
    pub fn ime_cursor_area(&self) -> Rect {
        bbox_to_rect(self.editor.ime_cursor_area())
    }

    /// Converts the current layout into [`frust_scene::GlyphRun`]s at
    /// `origin` (reusing [`crate::TextLayout`]'s converter).
    ///
    /// Empty before the first [`apply`](Self::apply).
    pub fn to_scene_runs(&self, origin: Point) -> Vec<GlyphRun> {
        match self.editor.try_layout() {
            Some(layout) => crate::convert::layout_to_scene_runs(layout, origin),
            None => Vec::new(),
        }
    }

    /// The active composing region (byte range): parley's own preedit on the
    /// desktop path, or the platform-tracked region on the state-sync path.
    fn composing_range(&self) -> Option<Range<usize>> {
        self.editor
            .raw_compose()
            .clone()
            .or_else(|| self.platform_composing.clone())
    }
}

/// Applies `mv` or `ext` to `drv` depending on `select`.
fn sel<'a>(
    drv: &mut parley::PlainEditorDriver<'a, Brush>,
    select: bool,
    mv: impl FnOnce(&mut parley::PlainEditorDriver<'a, Brush>),
    ext: impl FnOnce(&mut parley::PlainEditorDriver<'a, Brush>),
) {
    if select {
        ext(drv);
    } else {
        mv(drv);
    }
}

/// Converts a parley [`BoundingBox`] (Y-down logical pixels) into a kurbo
/// [`Rect`].
fn bbox_to_rect(bbox: BoundingBox) -> Rect {
    Rect::new(bbox.x0, bbox.y0, bbox.x1, bbox.y1)
}

/// Converts a byte offset into `text` to a UTF-16 code-unit offset.
///
/// Out-of-range offsets clamp to `text.len()`; an offset landing inside a
/// multi-byte UTF-8 sequence snaps back to the enclosing char boundary.
pub fn byte_to_utf16(text: &str, byte_idx: usize) -> usize {
    let mut boundary = byte_idx.min(text.len());
    while boundary > 0 && !text.is_char_boundary(boundary) {
        boundary -= 1;
    }
    text[..boundary].chars().map(char::len_utf16).sum()
}

/// Converts a UTF-16 code-unit offset into `text` to a byte offset.
///
/// Out-of-range offsets clamp to `text.len()`; an offset landing inside a
/// surrogate pair snaps back to the start of the enclosing char.
pub fn utf16_to_byte(text: &str, utf16_idx: usize) -> usize {
    let mut units = 0usize;
    for (byte_off, ch) in text.char_indices() {
        if utf16_idx <= units {
            return byte_off;
        }
        let next = units + ch.len_utf16();
        if utf16_idx < next {
            // Landing inside a surrogate pair: clamp to this char's start.
            return byte_off;
        }
        units = next;
    }
    text.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::{FontStyle, FontWeight};
    use peniko::Color;

    fn style() -> TextStyle {
        TextStyle::new(24.0, Color::BLACK)
    }

    fn editor() -> (TextEditor, TextContext) {
        (TextEditor::new(&style()), TextContext::new())
    }

    /// Emoji with an astral-plane code point (grinning face) — a UTF-16
    /// surrogate pair, 4 UTF-8 bytes.
    const EMOJI: &str = "\u{1F600}";
    /// A ZWJ "family" sequence (man+ZWJ+woman+ZWJ+girl+ZWJ+boy).
    const FAMILY: &str = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466}";

    #[test]
    fn insert_appends_text_and_advances_caret() {
        let (mut ed, mut cx) = editor();
        ed.apply(EditOp::Insert("Hi".into()), &mut cx);
        assert_eq!(ed.text(), "Hi");
        let st = ed.editing_state_bytes();
        assert_eq!(st.base, 2);
        assert_eq!(st.extent, 2);
    }

    #[test]
    fn backdelete_over_emoji_removes_whole_code_point() {
        let (mut ed, mut cx) = editor();
        ed.apply(EditOp::Insert(format!("a{EMOJI}")), &mut cx);
        assert_eq!(ed.text(), format!("a{EMOJI}"));
        ed.apply(EditOp::Backdelete, &mut cx);
        // The 4-byte emoji is removed as a unit, not a single byte.
        assert_eq!(ed.text(), "a");
    }

    #[test]
    fn backdelete_over_zwj_sequence_removes_a_whole_code_point() {
        let (mut ed, mut cx) = editor();
        ed.apply(EditOp::Insert(format!("x{FAMILY}")), &mut cx);
        let before = ed.text().len();
        ed.apply(EditOp::Backdelete, &mut cx);
        let after = ed.text();
        // parley deletes by code point, not byte: the trailing 4-byte emoji
        // is removed as a unit and the result stays valid UTF-8. (Full
        // ZWJ-grapheme-cluster awareness is a widget/bridge-layer concern per
        // research/RESEARCH.md, not parley's guarantee.)
        assert_eq!(before - after.len(), 4, "one 4-byte code point removed");
        assert!(after.starts_with('x'));
    }

    #[test]
    fn forward_delete_removes_following_grapheme() {
        let (mut ed, mut cx) = editor();
        ed.apply(EditOp::Insert("abc".into()), &mut cx);
        ed.apply(EditOp::Home { select: false }, &mut cx);
        ed.apply(EditOp::Delete, &mut cx);
        assert_eq!(ed.text(), "bc");
    }

    #[test]
    fn move_left_right_adjust_caret_without_selecting() {
        let (mut ed, mut cx) = editor();
        ed.apply(EditOp::Insert("abc".into()), &mut cx);
        ed.apply(EditOp::MoveLeft { select: false }, &mut cx);
        assert_eq!(ed.editing_state_bytes().extent, 2);
        let st = ed.editing_state_bytes();
        assert_eq!(st.base, st.extent, "plain move must collapse the selection");
        ed.apply(EditOp::MoveRight { select: false }, &mut cx);
        assert_eq!(ed.editing_state_bytes().extent, 3);
    }

    #[test]
    fn move_left_with_select_extends_selection() {
        let (mut ed, mut cx) = editor();
        ed.apply(EditOp::Insert("abc".into()), &mut cx);
        ed.apply(EditOp::MoveLeft { select: true }, &mut cx);
        let st = ed.editing_state_bytes();
        assert_ne!(st.base, st.extent, "select variant must extend selection");
        assert_eq!(st.base, 3);
        assert_eq!(st.extent, 2);
    }

    #[test]
    fn word_move_jumps_over_a_word() {
        let (mut ed, mut cx) = editor();
        ed.apply(EditOp::Insert("hello world".into()), &mut cx);
        ed.apply(EditOp::Home { select: false }, &mut cx);
        assert_eq!(ed.editing_state_bytes().extent, 0);
        ed.apply(EditOp::MoveWordRight { select: false }, &mut cx);
        let after = ed.editing_state_bytes().extent;
        assert!(
            after > 0 && after <= 6,
            "word-right should land near the space, got {after}"
        );
    }

    #[test]
    fn home_and_end_move_to_line_bounds() {
        let (mut ed, mut cx) = editor();
        ed.apply(EditOp::Insert("hello".into()), &mut cx);
        ed.apply(EditOp::Home { select: false }, &mut cx);
        assert_eq!(ed.editing_state_bytes().extent, 0);
        ed.apply(EditOp::End { select: false }, &mut cx);
        assert_eq!(ed.editing_state_bytes().extent, 5);
    }

    #[test]
    fn select_all_then_collapse() {
        let (mut ed, mut cx) = editor();
        ed.apply(EditOp::Insert("hello".into()), &mut cx);
        ed.apply(EditOp::SelectAll, &mut cx);
        let st = ed.editing_state_bytes();
        assert_eq!(st.base.min(st.extent), 0);
        assert_eq!(st.base.max(st.extent), 5);
        ed.apply(EditOp::CollapseSelection, &mut cx);
        let st = ed.editing_state_bytes();
        assert_eq!(st.base, st.extent, "collapse must drop the anchor");
    }

    #[test]
    fn insert_replaces_selection() {
        let (mut ed, mut cx) = editor();
        ed.apply(EditOp::Insert("hello".into()), &mut cx);
        ed.apply(EditOp::SelectAll, &mut cx);
        ed.apply(EditOp::Insert("bye".into()), &mut cx);
        assert_eq!(ed.text(), "bye");
    }

    #[test]
    fn compose_lifecycle_commit() {
        let (mut ed, mut cx) = editor();
        ed.apply(
            EditOp::Compose {
                text: "ni".into(),
                cursor: Some((2, 2)),
            },
            &mut cx,
        );
        // Composing region is exposed in the editing state.
        let st = ed.editing_state_bytes();
        assert_eq!(st.composing, Some(0..2), "preedit range should be reported");
        assert_eq!(ed.text(), "ni");
        ed.apply(EditOp::FinishCompose, &mut cx);
        let st = ed.editing_state_bytes();
        assert_eq!(st.composing, None, "commit clears the composing region");
        assert_eq!(ed.text(), "ni");
    }

    #[test]
    fn compose_lifecycle_clear_reverts() {
        let (mut ed, mut cx) = editor();
        ed.apply(EditOp::Insert("a".into()), &mut cx);
        ed.apply(
            EditOp::Compose {
                text: "xyz".into(),
                cursor: None,
            },
            &mut cx,
        );
        assert_eq!(ed.text(), "axyz");
        ed.apply(EditOp::ClearCompose, &mut cx);
        // Clearing the preedit reverts to the pre-compose buffer.
        assert_eq!(ed.text(), "a");
        assert_eq!(ed.editing_state_bytes().composing, None);
    }

    #[test]
    fn apply_editing_state_sets_text_selection_and_composing() {
        let (mut ed, mut cx) = editor();
        let state = EditingStateBytes {
            text: "hello".into(),
            base: 1,
            extent: 3,
            composing: Some(0..5),
        };
        ed.apply(EditOp::ApplyEditingState(state.clone()), &mut cx);
        assert_eq!(ed.text(), "hello");
        assert_eq!(ed.editing_state_bytes(), state);
    }

    #[test]
    fn apply_editing_state_is_idempotent_and_short_circuits() {
        let (mut ed, mut cx) = editor();
        let state = EditingStateBytes {
            text: "world".into(),
            base: 2,
            extent: 2,
            composing: None,
        };
        ed.apply(EditOp::ApplyEditingState(state.clone()), &mut cx);
        let first = ed.editing_state_bytes();
        // Re-applying the identical state must be a no-op (feedback-loop guard).
        ed.apply(EditOp::ApplyEditingState(state.clone()), &mut cx);
        assert_eq!(ed.editing_state_bytes(), first);
        assert_eq!(ed.editing_state_bytes(), state);
    }

    #[test]
    fn apply_editing_state_selection_only_change_keeps_text() {
        let (mut ed, mut cx) = editor();
        ed.apply(
            EditOp::ApplyEditingState(EditingStateBytes {
                text: "abcd".into(),
                base: 0,
                extent: 0,
                composing: None,
            }),
            &mut cx,
        );
        ed.apply(
            EditOp::ApplyEditingState(EditingStateBytes {
                text: "abcd".into(),
                base: 1,
                extent: 3,
                composing: None,
            }),
            &mut cx,
        );
        let st = ed.editing_state_bytes();
        assert_eq!(st.text, "abcd");
        assert_eq!((st.base, st.extent), (1, 3));
    }

    #[test]
    fn editing_state_utf16_converts_offsets_for_emoji() {
        let (mut ed, mut cx) = editor();
        // Buffer: "a😀" — 'a' = 1 byte / 1 unit, emoji = 4 bytes / 2 units.
        ed.apply(
            EditOp::ApplyEditingState(EditingStateBytes {
                text: format!("a{EMOJI}"),
                base: 5,
                extent: 5,
                composing: Some(1..5),
            }),
            &mut cx,
        );
        let st = ed.editing_state_utf16();
        assert_eq!(st.selection_base, 3, "1 + 2 UTF-16 units");
        assert_eq!(st.selection_extent, 3);
        assert_eq!(st.composing_base, 1);
        assert_eq!(st.composing_extent, 3);
    }

    #[test]
    fn editing_state_utf16_reports_minus_one_when_not_composing() {
        let (mut ed, mut cx) = editor();
        ed.apply(EditOp::Insert("hi".into()), &mut cx);
        let st = ed.editing_state_utf16();
        assert_eq!(st.composing_base, -1);
        assert_eq!(st.composing_extent, -1);
    }

    #[test]
    fn cursor_and_selection_geometry_are_sane() {
        let (mut ed, mut cx) = editor();
        ed.apply(EditOp::Insert("hello".into()), &mut cx);

        let caret = ed
            .cursor_rect(2.0)
            .expect("caret should exist after typing");
        assert!(
            caret.width() > 0.0,
            "caret width should reflect the requested size"
        );
        assert!(caret.height() > 0.0, "caret should have positive height");

        ed.apply(EditOp::SelectAll, &mut cx);
        let rects = ed.selection_rects();
        assert!(
            !rects.is_empty(),
            "a non-empty selection should yield rects"
        );
        for r in &rects {
            assert!(
                r.width() > 0.0 && r.height() > 0.0,
                "selection rect must be non-degenerate: {r:?}"
            );
            assert!(r.x1 >= r.x0 && r.y1 >= r.y0, "rect must be ordered: {r:?}");
        }

        let ime = ed.ime_cursor_area();
        assert!(
            ime.x1 >= ime.x0 && ime.y1 >= ime.y0,
            "ime area must be ordered"
        );
    }

    #[test]
    fn to_scene_runs_translates_by_origin_and_is_empty_when_fresh() {
        let (mut ed, mut cx) = editor();
        // Fresh editor: no layout refreshed yet.
        assert!(ed.to_scene_runs(Point::ORIGIN).is_empty());

        ed.apply(EditOp::Insert("Ag".into()), &mut cx);
        let runs = ed.to_scene_runs(Point::new(10.0, 20.0));
        assert!(!runs.is_empty(), "typed text should produce glyph runs");
        for run in &runs {
            assert_eq!(run.transform, kurbo::Affine::translate((10.0, 20.0)));
        }
    }

    // --- conversion helper matrix ---

    #[test]
    fn byte_to_utf16_matrix() {
        // ASCII
        assert_eq!(byte_to_utf16("abc", 0), 0);
        assert_eq!(byte_to_utf16("abc", 2), 2);
        // é (U+00E9): 2 bytes, 1 UTF-16 unit
        assert_eq!(byte_to_utf16("é", 0), 0);
        assert_eq!(byte_to_utf16("é", 2), 1);
        // Mid-char byte snaps back to the boundary.
        assert_eq!(byte_to_utf16("é", 1), 0);
        // Emoji: 4 bytes, 2 UTF-16 units.
        assert_eq!(byte_to_utf16(EMOJI, 4), 2);
        assert_eq!(byte_to_utf16(EMOJI, 2), 0, "mid-surrogate byte snaps back");
        // ZWJ family: 25 bytes, 11 UTF-16 units (4 emoji × 2 + 3 ZWJ × 1).
        assert_eq!(byte_to_utf16(FAMILY, FAMILY.len()), 11);
        // Overflow clamps to the end.
        assert_eq!(byte_to_utf16("abc", 99), 3);
    }

    #[test]
    fn utf16_to_byte_matrix() {
        // ASCII
        assert_eq!(utf16_to_byte("abc", 0), 0);
        assert_eq!(utf16_to_byte("abc", 2), 2);
        // é
        assert_eq!(utf16_to_byte("é", 1), 2);
        // Emoji
        assert_eq!(utf16_to_byte(EMOJI, 2), 4);
        assert_eq!(
            utf16_to_byte(EMOJI, 1),
            0,
            "mid-surrogate snaps to char start"
        );
        // ZWJ family: all 11 units -> full byte length.
        assert_eq!(utf16_to_byte(FAMILY, 11), FAMILY.len());
        // Overflow clamps to the end.
        assert_eq!(utf16_to_byte("abc", 99), 3);
    }

    #[test]
    fn conversion_helpers_round_trip_on_boundaries() {
        for text in ["", "abc", "é", EMOJI, FAMILY, "aé😀b"] {
            let mut byte = 0;
            for ch in text.chars() {
                let units = byte_to_utf16(text, byte);
                assert_eq!(
                    utf16_to_byte(text, units),
                    byte,
                    "round-trip failed at byte {byte} in {text:?}"
                );
                byte += ch.len_utf8();
            }
            // End boundary.
            let units = byte_to_utf16(text, text.len());
            assert_eq!(utf16_to_byte(text, units), text.len());
        }
    }

    // --- editor/context style parity (task: TextStyle expansion) ---

    #[test]
    fn editor_letter_spacing_widens_layout_like_context_layout() {
        // TextEditor::new's edit-style defaults must apply the same
        // letter-spacing TextContext::layout does (see frust_text::tests
        // in lib.rs for the equivalent context-side assertion).
        let narrow_style = TextStyle::new(24.0, Color::BLACK);
        let (mut narrow_ed, mut narrow_cx) = (TextEditor::new(&narrow_style), TextContext::new());
        narrow_ed.apply(EditOp::Insert("Hello".into()), &mut narrow_cx);
        let narrow_width = narrow_ed.layout_size().width;

        let mut wide_style = TextStyle::new(24.0, Color::BLACK);
        wide_style.letter_spacing = 20.0;
        let (mut wide_ed, mut wide_cx) = (TextEditor::new(&wide_style), TextContext::new());
        wide_ed.apply(EditOp::Insert("Hello".into()), &mut wide_cx);
        let wide_width = wide_ed.layout_size().width;

        assert!(
            wide_width > narrow_width,
            "the editor's own layout should widen with extra letter-spacing \
             the same way TextContext::layout's does: {narrow_width} vs {wide_width}"
        );
    }

    // --- wrapped multi-line mode (task: textinput-multiline) ---

    #[test]
    fn single_line_mode_keeps_one_line_and_grows_horizontally() {
        // With no wrap width, a long string stays on a single visual line and
        // the layout width reflects the whole run.
        let (mut ed, mut cx) = editor();
        ed.apply(
            EditOp::Insert("the quick brown fox jumps over the lazy dog".into()),
            &mut cx,
        );
        assert_eq!(ed.line_count(), 1, "single-line mode never wraps");
        let one_line_h = ed.layout_size().height;
        assert!(one_line_h > 0.0);
        assert!(
            ed.layout_size().width > 100.0,
            "single line grows horizontally"
        );
    }

    #[test]
    fn wrap_width_produces_multiple_lines_and_grows_vertically() {
        let (mut ed, mut cx) = editor();
        ed.apply(
            EditOp::Insert("the quick brown fox jumps over the lazy dog".into()),
            &mut cx,
        );
        let single_h = ed.layout_size().height;

        // Constrain to a narrow width: the same text must wrap onto >1 line and
        // the reported layout height must grow accordingly.
        ed.set_wrap_width(Some(60.0), &mut cx);
        assert!(
            ed.line_count() > 1,
            "narrow wrap width splits the run into multiple lines, got {}",
            ed.line_count()
        );
        assert!(
            ed.layout_size().height > single_h,
            "wrapped multi-line layout is taller than the single line: {single_h} -> {}",
            ed.layout_size().height
        );

        // Clearing the wrap width collapses back to a single line.
        ed.set_wrap_width(None, &mut cx);
        assert_eq!(ed.line_count(), 1, "None restores single-line mode");
    }

    #[test]
    fn insert_newline_splits_into_two_lines() {
        let (mut ed, mut cx) = editor();
        // A wrap width is what makes vertical layout meaningful; wide enough that
        // only the explicit newline splits the text.
        ed.set_wrap_width(Some(400.0), &mut cx);
        ed.apply(EditOp::Insert("ab".into()), &mut cx);
        ed.apply(EditOp::InsertNewline, &mut cx);
        ed.apply(EditOp::Insert("cd".into()), &mut cx);
        assert_eq!(ed.text(), "ab\ncd");
        assert_eq!(ed.line_count(), 2, "the newline creates a second line");
        // Caret sits after the last char (byte offset 5: 'a''b''\n''c''d').
        assert_eq!(ed.editing_state_bytes().extent, 5);
    }

    #[test]
    fn move_up_down_navigate_across_a_newline() {
        let (mut ed, mut cx) = editor();
        ed.set_wrap_width(Some(400.0), &mut cx);
        ed.apply(EditOp::Insert("abc".into()), &mut cx);
        ed.apply(EditOp::InsertNewline, &mut cx);
        ed.apply(EditOp::Insert("xyz".into()), &mut cx);
        // Caret on line 2, at its end (byte 7).
        let bottom = ed.editing_state_bytes().extent;
        assert_eq!(bottom, 7);

        // MoveUp lands on line 1 (byte offset within [0, 3]).
        ed.apply(EditOp::MoveUp { select: false }, &mut cx);
        let up = ed.editing_state_bytes().extent;
        assert!(up <= 3, "move-up crosses the newline onto line 1, got {up}");

        // MoveDown returns to line 2 (byte offset >= 4, past the newline).
        ed.apply(EditOp::MoveDown { select: false }, &mut cx);
        let down = ed.editing_state_bytes().extent;
        assert!(
            down >= 4,
            "move-down crosses the newline back onto line 2, got {down}"
        );
    }

    #[test]
    fn utf16_selection_indices_are_correct_across_a_newline() {
        let (mut ed, mut cx) = editor();
        ed.set_wrap_width(Some(400.0), &mut cx);
        // Line 1 "a😀" (1 byte + 4 bytes / 1 + 2 UTF-16 units), newline (1/1),
        // line 2 "b" — select the whole buffer.
        ed.apply(
            EditOp::ApplyEditingState(EditingStateBytes {
                text: format!("a{EMOJI}\nb"),
                base: 0,
                extent: format!("a{EMOJI}\nb").len(),
                composing: None,
            }),
            &mut cx,
        );
        assert_eq!(ed.line_count(), 2, "the newline still splits the layout");
        let st = ed.editing_state_utf16();
        // 1 (a) + 2 (emoji) + 1 (\n) + 1 (b) = 5 UTF-16 units.
        assert_eq!(st.selection_base, 0);
        assert_eq!(
            st.selection_extent, 5,
            "the newline counts as exactly one UTF-16 unit"
        );
    }

    #[test]
    fn editor_applies_weight_and_style_edit_defaults_without_panicking() {
        let mut style = TextStyle::new(24.0, Color::BLACK);
        style.weight = FontWeight::BOLD;
        style.style = FontStyle::Italic;
        let (mut ed, mut cx) = (TextEditor::new(&style), TextContext::new());
        ed.apply(EditOp::Insert("Hi".into()), &mut cx);
        assert!(
            ed.layout_size().width > 0.0,
            "bold+italic edit-style defaults should still shape text"
        );
    }
}
