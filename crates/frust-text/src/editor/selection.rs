//! [`Selection`]: an anchor and a focus [`Cursor`], and what the editor's
//! selection commands do with them.
//!
//! The rules (derived from parley 0.11.1's selection): a plain move collapses the selection (a
//! left/right move of a non-collapsed selection collapses it to its visually
//! leading/trailing end), an extending move keeps the anchor; up/down keeps a
//! sticky horizontal position across repeated moves; a word selection made
//! at a point keeps extending by whole words; and the selection's geometry is
//! one rectangle per contiguous visual span per line, a selected newline
//! drawn as a quarter-em stub at its line's end.

use std::ops::Range;

use kurbo::Rect;

use super::cursor::{Affinity, Cursor, EditLayout};

/// What a selection was started from, so extending it keeps the granularity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum AnchorBase {
    /// A caret position.
    #[default]
    Unit,
    /// The word at a point: its start and end.
    Word(Cursor, Cursor),
}

/// A selection: the fixed `anchor`, the moving `focus`.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Selection {
    anchor: Cursor,
    focus: Cursor,
    anchor_base: AnchorBase,
    /// The horizontal position vertical motion keeps.
    h_pos: Option<f32>,
}

impl PartialEq for Selection {
    fn eq(&self, other: &Self) -> bool {
        self.anchor == other.anchor && self.focus == other.focus
    }
}

impl From<Cursor> for Selection {
    fn from(cursor: Cursor) -> Self {
        Self::new(cursor, cursor)
    }
}

impl Selection {
    pub(crate) fn new(anchor: Cursor, focus: Cursor) -> Self {
        Self {
            anchor,
            focus,
            anchor_base: AnchorBase::Unit,
            h_pos: None,
        }
    }

    /// A caret at byte `index`.
    pub(crate) fn from_byte_index(layout: &EditLayout, index: usize, affinity: Affinity) -> Self {
        Cursor::from_byte_index(layout, index, affinity).into()
    }

    /// A caret at the point nearest `(x, y)`.
    pub(crate) fn from_point(layout: &EditLayout, x: f32, y: f32) -> Self {
        Cursor::from_point(layout, x, y).into()
    }

    /// The word at the point nearest `(x, y)`: from the word boundary at or
    /// before the unit hit to the next one.
    pub(crate) fn word_from_point(layout: &EditLayout, x: f32, y: f32) -> Self {
        let Some((mut u, _)) = layout.hit(x, y) else {
            return Cursor::from_byte_index(layout, layout.text_len, Affinity::Upstream).into();
        };
        if !layout.units[u].word_boundary
            && let Some(previous) = layout.previous_logical_word(u)
        {
            u = previous;
        }
        let anchor = Cursor::from_unit(layout, u, !layout.units[u].rtl);
        let focus = anchor.next_logical_word(layout);
        Self {
            anchor,
            focus,
            anchor_base: AnchorBase::Word(anchor, focus),
            h_pos: None,
        }
    }

    pub(crate) fn is_collapsed(&self) -> bool {
        self.anchor.index == self.focus.index
    }

    pub(crate) fn anchor(&self) -> Cursor {
        self.anchor
    }

    pub(crate) fn focus(&self) -> Cursor {
        self.focus
    }

    /// The caret at the focus.
    #[must_use]
    pub(crate) fn collapse(&self) -> Self {
        self.focus.into()
    }

    /// This selection within `layout`'s text.
    #[must_use]
    pub(crate) fn refresh(&self, layout: &EditLayout) -> Self {
        Self {
            anchor: self.anchor.refresh(layout),
            focus: self.focus.refresh(layout),
            anchor_base: match self.anchor_base {
                AnchorBase::Unit => AnchorBase::Unit,
                AnchorBase::Word(start, end) => {
                    AnchorBase::Word(start.refresh(layout), end.refresh(layout))
                }
            },
            h_pos: self.h_pos,
        }
    }

    /// The selected byte range.
    pub(crate) fn text_range(&self) -> Range<usize> {
        let start = self.anchor.index.min(self.focus.index);
        let end = self.anchor.index.max(self.focus.index);
        start..end
    }

    /// The focus moved to `focus`, the anchor kept when `extend`.
    fn maybe_extend(&self, focus: Cursor, extend: bool) -> Self {
        if extend {
            Self::new(self.anchor, focus)
        } else {
            focus.into()
        }
    }

    /// One unit right on screen (a non-collapsed selection collapses to its
    /// right end unless extending).
    #[must_use]
    pub(crate) fn next_visual(&self, layout: &EditLayout, extend: bool) -> Self {
        if !self.is_collapsed() && !extend {
            let anchor = self.anchor.geometry(layout, 0.0);
            let focus = self.focus.geometry(layout, 0.0);
            if (anchor.y0, anchor.x0) > (focus.y0, focus.x0) {
                self.anchor.into()
            } else {
                self.focus.into()
            }
        } else {
            self.maybe_extend(self.focus.next_visual(layout), extend)
        }
    }

    /// One unit left on screen (a non-collapsed selection collapses to its
    /// left end unless extending).
    #[must_use]
    pub(crate) fn previous_visual(&self, layout: &EditLayout, extend: bool) -> Self {
        if !self.is_collapsed() && !extend {
            let anchor = self.anchor.geometry(layout, 0.0);
            let focus = self.focus.geometry(layout, 0.0);
            if (anchor.y0, anchor.x0) < (focus.y0, focus.x0) {
                self.anchor.into()
            } else {
                self.focus.into()
            }
        } else {
            self.maybe_extend(self.focus.previous_visual(layout), extend)
        }
    }

    /// One word right on screen.
    #[must_use]
    pub(crate) fn next_visual_word(&self, layout: &EditLayout, extend: bool) -> Self {
        self.maybe_extend(self.focus.next_visual_word(layout), extend)
    }

    /// One word left on screen.
    #[must_use]
    pub(crate) fn previous_visual_word(&self, layout: &EditLayout, extend: bool) -> Self {
        self.maybe_extend(self.focus.previous_visual_word(layout), extend)
    }

    /// The focus moved `delta` lines (negative: up), keeping the horizontal
    /// position; past the first line it goes to the line's start, past the
    /// last to its end.
    #[must_use]
    pub(crate) fn move_lines(&self, layout: &EditLayout, delta: isize, extend: bool) -> Self {
        if delta == 0 {
            return *self;
        }
        let line_limit = layout.lines.len().saturating_sub(1);
        let line_index = self.focus.line(layout).unwrap_or(line_limit);
        let new_line_index = line_index.saturating_add_signed(delta);
        if delta < 0 && line_index.checked_add_signed(delta).is_none() {
            return self
                .move_to_line(layout, 0, extend)
                .line_start(layout, extend);
        } else if delta > 0 && new_line_index > line_limit {
            return self
                .move_to_line(layout, line_limit, extend)
                .line_end(layout, extend);
        }
        self.move_to_line(layout, new_line_index, extend)
    }

    fn move_to_line(&self, layout: &EditLayout, line_index: usize, extend: bool) -> Self {
        let Some(line) = layout.lines.get(line_index) else {
            return *self;
        };
        let h_pos = self
            .h_pos
            .unwrap_or_else(|| self.focus.geometry(layout, 0.0).x0 as f32);
        let y = line.bottom as f32 - line.ascent * 0.5;
        let focus = Cursor::from_point(layout, h_pos, y);
        Self {
            anchor: if extend { self.anchor } else { focus },
            focus,
            anchor_base: AnchorBase::Unit,
            h_pos: Some(h_pos),
        }
    }

    /// The focus moved to the start of its line.
    #[must_use]
    pub(crate) fn line_start(&self, layout: &EditLayout, extend: bool) -> Self {
        match self.focus.line(layout) {
            Some(line) => self.maybe_extend(
                Cursor::from_byte_index(
                    layout,
                    layout.lines[line].text.start,
                    Affinity::Downstream,
                ),
                extend,
            ),
            None => *self,
        }
    }

    /// The focus moved to the end of its line: before its newline, or after
    /// its last unit at a soft wrap.
    #[must_use]
    pub(crate) fn line_end(&self, layout: &EditLayout, extend: bool) -> Self {
        let Some(line) = self.focus.line(layout).map(|l| &layout.lines[l]) else {
            return *self;
        };
        let (index, affinity) = line
            .explicit
            .then(|| {
                let newline = layout.unit_at(line.text.end.checked_sub(1)?)?;
                Some((layout.units[newline].text.start, Affinity::Downstream))
            })
            .flatten()
            .unwrap_or((line.text.end, Affinity::Upstream));
        self.maybe_extend(Cursor::from_byte_index(layout, index, affinity), extend)
    }

    /// The whole text.
    pub(crate) fn all(layout: &EditLayout) -> Self {
        Self::from_byte_index(layout, 0, Affinity::Downstream).move_lines(layout, isize::MAX, true)
    }

    /// The focus extended to the point nearest `(x, y)`, by whole words when
    /// the selection started as a word.
    #[must_use]
    pub(crate) fn extend_to_point(&self, layout: &EditLayout, x: f32, y: f32) -> Self {
        match self.anchor_base {
            AnchorBase::Unit => Self::new(self.anchor, Cursor::from_point(layout, x, y)),
            AnchorBase::Word(start, end) => {
                let target = Self::word_from_point(layout, x, y);
                let [anchor, focus] = extend_selection(target, [start, end]);
                Self {
                    anchor,
                    focus,
                    anchor_base: self.anchor_base,
                    h_pos: None,
                }
            }
        }
    }

    /// Calls `f` with each rectangle covering the selection, line by line.
    pub(crate) fn geometry_with(&self, layout: &EditLayout, mut f: impl FnMut(Rect)) {
        // A selected newline: a stub a quarter of the line's ascent + descent
        // wide, so it does not change with the line height.
        const NEWLINE_WIDTH_RATIO: f64 = 0.25;
        if self.is_collapsed() {
            return;
        }
        let (mut start, mut end) = (self.anchor, self.focus);
        if start.index > end.index {
            std::mem::swap(&mut start, &mut end);
        }
        let text_range = start.index..end.index;
        let first_line = start.line(layout).unwrap_or(0);
        let last_line = end.line(layout).unwrap_or(layout.lines.len() + 1);
        for line_index in first_line..=last_line {
            let Some(line) = layout.lines.get(line_index) else {
                continue;
            };
            let (top, bottom) = (line.top, line.bottom);
            let newline = if line.explicit {
                (f64::from(line.ascent) + f64::from(line.descent)) * NEWLINE_WIDTH_RATIO
            } else {
                0.0
            };
            if line_index == first_line || line_index == last_line {
                let mut start_x = f64::from(line.offset);
                let mut cur_x = start_x;
                let mut selected = 0;
                for unit in &layout.units[line.units.clone()] {
                    let advance = f64::from(unit.advance);
                    if text_range.contains(&unit.text.start) {
                        selected += 1;
                        cur_x += advance;
                    } else {
                        if cur_x != start_x {
                            f(Rect::new(start_x, top, cur_x, bottom));
                        }
                        cur_x += advance;
                        start_x = cur_x;
                    }
                }
                let mut end_x = cur_x;
                if line_index != last_line || (selected != 0 && line.advance == 0.0) {
                    end_x += newline;
                }
                if end_x != start_x {
                    f(Rect::new(start_x, top, end_x, bottom));
                }
            } else {
                let x = f64::from(line.offset);
                let width = f64::from(line.advance);
                f(Rect::new(x, top, x + width + newline, bottom));
            }
        }
    }
}

/// The anchor and focus of a word-granular extension from `anchor_base` to
/// `target`: the union of both, oriented toward the target.
fn extend_selection(target: Selection, anchor_base: [Cursor; 2]) -> [Cursor; 2] {
    let rightward = target.anchor.index >= anchor_base[0].index;
    let cursors = [target.anchor, target.focus, anchor_base[0], anchor_base[1]];
    let mut min = cursors[0];
    let mut max = cursors[0];
    for c in cursors {
        if c.index < min.index {
            min = c;
        }
        if c.index > max.index {
            max = c;
        }
    }
    if rightward { [min, max] } else { [max, min] }
}
