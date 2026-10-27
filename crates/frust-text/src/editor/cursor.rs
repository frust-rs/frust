//! The editor's view of a laid-out paragraph, and the caret positions in it.
//!
//! [`EditLayout`] flattens a [`Shaped`] paragraph and its [`LineTable`] into
//! the units a caret moves over: one [`Unit`] per grapheme, placed on its
//! line in visual order with its share of the clusters it overlaps (the
//! shaper's grapheme advances), so a ligature or a multi-glyph cluster still
//! has a caret stop at every grapheme boundary inside it.
//!
//! [`Cursor`] is a byte offset with an [`Affinity`]: which side of a
//! boundary the caret belongs to where one byte offset has two visual
//! positions (a soft line break, a bidi run boundary). Motion follows frust's
//! own editor rules (derived from parley 0.11.1's editor):
//!
//! - left/right is **visual** ([`Cursor::previous_visual`]/
//!   [`Cursor::next_visual`]): the unit beside the caret on screen, so a
//!   right-to-left run is crossed right to left;
//! - word motion steps visually until a word boundary (the analysis' UAX #29
//!   word flags, which are independent of line-break opportunities: a Hangul
//!   or kana word is one word although every syllable may wrap);
//! - up/down keeps a sticky horizontal position (see the selection module);
//! - hit-testing ([`Cursor::from_point`]) finds the line by `y` and the unit
//!   by `x`, landing on the nearer edge.
//!
//! Geometry is Y-down logical pixels, the line box being its quantized
//! ascent and descent plus the leading halves (Chrome's rounding, as the line
//! layout's baseline uses).

use std::ops::Range;

use kurbo::Rect;

use crate::layout::{LineTable, Source};
use crate::shape::{Shaped, Whitespace};

/// Which side of a position the caret is attached to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Affinity {
    /// Attached to the unit after the position (in logical order).
    #[default]
    Downstream,
    /// Attached to the unit before the position.
    Upstream,
}

/// One grapheme as laid out.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Unit {
    /// Source byte range.
    pub(crate) text: Range<usize>,
    /// The left edge, layout-local (line offset and justification included).
    pub(crate) x: f32,
    pub(crate) advance: f32,
    pub(crate) rtl: bool,
    /// Index into [`EditLayout::lines`].
    pub(crate) line: usize,
    /// A word boundary precedes the unit.
    pub(crate) word_boundary: bool,
    pub(crate) space_or_nbsp: bool,
    /// A newline (CR, LF, CRLF, LS, PS): a hard line break.
    pub(crate) newline: bool,
    /// Some character of the grapheme is an emoji or pictograph.
    pub(crate) emoji: bool,
    pub(crate) font_size: f32,
}

/// One line as the editor sees it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct EditLine {
    /// Source byte range (the newline that ends it included).
    pub(crate) text: Range<usize>,
    /// Index range into [`EditLayout::units`], visual order.
    pub(crate) units: Range<usize>,
    /// Where the line's first visual unit starts.
    pub(crate) offset: f32,
    /// The line's full advance (trailing whitespace and stretch included).
    pub(crate) advance: f32,
    pub(crate) ascent: f32,
    pub(crate) descent: f32,
    /// The top of the line box.
    pub(crate) top: f64,
    /// The bottom of the line box.
    pub(crate) bottom: f64,
    /// The line ends in a newline.
    pub(crate) explicit: bool,
    /// The line ends at a soft wrap (any line but the last that does not end
    /// in a newline).
    pub(crate) soft: bool,
}

/// A laid-out paragraph as units and lines.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct EditLayout {
    /// Every line's units, line after line, each line's in visual order.
    pub(crate) units: Vec<Unit>,
    pub(crate) lines: Vec<EditLine>,
    /// Unit indices sorted by source position.
    logical: Vec<usize>,
    /// A unit's position in [`Self::logical`].
    rank: Vec<usize>,
    pub(crate) text_len: usize,
}

impl EditLayout {
    /// The units of `table` over `shaped`, for a text of `text_len` bytes
    /// (`shaped` may cover more: an empty text is shaped as one sizing space,
    /// which yields no unit).
    pub(crate) fn new(text_len: usize, shaped: &Shaped, table: &LineTable) -> Self {
        let byte_of = |char_index: u32| {
            shaped
                .chars
                .get(char_index as usize)
                .map_or(shaped.text_len, |c| c.byte_start as usize)
        };
        let mut units = Vec::new();
        let mut lines = Vec::with_capacity(table.lines.len());
        let last_line = table.lines.len().saturating_sub(1);
        for (line_index, line) in table.lines.iter().enumerate() {
            let first_unit = units.len();
            let mut pen = line.metrics.offset;
            for item in &table.items[line.items.clone()] {
                if item.source != Source::Text || item.clusters.is_empty() {
                    continue;
                }
                let run = &shaped.runs[item.run];
                let clusters = &shaped.clusters[item.clusters.clone()];
                let chars = clusters[0].chars.start..clusters[clusters.len() - 1].chars.end;
                let graphemes = shaped.graphemes
                    [run.graphemes.start as usize..run.graphemes.end as usize]
                    .iter()
                    .filter(|g| g.chars.start >= chars.start && g.chars.end <= chars.end);
                let rtl = item.bidi_level % 2 == 1;
                let ordered: Box<dyn Iterator<Item = _>> = if rtl {
                    Box::new(graphemes.rev())
                } else {
                    Box::new(graphemes)
                };
                for grapheme in ordered {
                    let text = byte_of(grapheme.chars.start)..byte_of(grapheme.chars.end);
                    let mut advance = grapheme.advance;
                    if line.justify != 0.0 {
                        let stretched = line
                            .stretched
                            .iter()
                            .filter(|&&ci| {
                                shaped.clusters.get(ci as usize).is_some_and(|c| {
                                    c.chars.start >= grapheme.chars.start
                                        && c.chars.start < grapheme.chars.end
                                })
                            })
                            .count();
                        advance += line.justify * stretched as f32;
                    }
                    if text.start < text_len {
                        let first = &shaped.chars[grapheme.chars.start as usize];
                        units.push(Unit {
                            text: text.start..text.end.min(text_len),
                            x: pen,
                            advance,
                            rtl,
                            line: line_index,
                            word_boundary: first.word_boundary,
                            space_or_nbsp: grapheme.whitespace.is_space_or_nbsp(),
                            newline: grapheme.whitespace == Whitespace::Newline,
                            emoji: shaped.chars
                                [grapheme.chars.start as usize..grapheme.chars.end as usize]
                                .iter()
                                .any(|c| c.emoji),
                            font_size: run.font_size,
                        });
                    }
                    pen += advance;
                }
            }
            let line_units = first_unit..units.len();
            let explicit = line.atoms.end > line.atoms.start
                && shaped
                    .atoms
                    .get(line.atoms.end - 1)
                    .is_some_and(|a| a.flags.mandatory_break_after);
            let m = &line.metrics;
            // The line box: quantized block coordinates (ascent and descent rounded).
            let (ascent, descent) = (m.ascent.round(), m.descent.round());
            let leading = m.line_height - (ascent + descent);
            let above = (leading * 0.5).floor();
            let below = leading.round() - above;
            let top = f64::from(m.baseline - ascent - above.max(0.0));
            let bottom = f64::from(m.baseline + descent + below.max(0.0));
            lines.push(EditLine {
                text: line.text.start.min(text_len)..line.text.end.min(text_len),
                advance: units[line_units.clone()].iter().map(|u| u.advance).sum(),
                units: line_units,
                offset: m.offset,
                ascent: m.ascent,
                descent: m.descent,
                top,
                bottom,
                explicit,
                soft: !explicit && line_index != last_line,
            });
        }
        let mut logical: Vec<usize> = (0..units.len()).collect();
        logical.sort_by_key(|&u| units[u].text.start);
        let mut rank = vec![0; units.len()];
        for (position, &u) in logical.iter().enumerate() {
            rank[u] = position;
        }
        Self {
            units,
            lines,
            logical,
            rank,
            text_len,
        }
    }

    /// The unit whose text contains byte `index`.
    pub(crate) fn unit_at(&self, index: usize) -> Option<usize> {
        if index >= self.text_len {
            return None;
        }
        let position = self
            .logical
            .partition_point(|&u| self.units[u].text.start <= index)
            .checked_sub(1)?;
        let u = self.logical[position];
        self.units[u].text.contains(&index).then_some(u)
    }

    /// The unit after `u` in logical order.
    pub(crate) fn next_logical(&self, u: usize) -> Option<usize> {
        self.logical.get(self.rank[u] + 1).copied()
    }

    /// The unit before `u` in logical order.
    pub(crate) fn previous_logical(&self, u: usize) -> Option<usize> {
        self.logical.get(self.rank[u].checked_sub(1)?).copied()
    }

    /// The unit after `u` on screen (the next line's first after a line's
    /// last).
    pub(crate) fn next_visual(&self, u: usize) -> Option<usize> {
        (u + 1 < self.units.len()).then_some(u + 1)
    }

    /// The unit before `u` on screen.
    pub(crate) fn previous_visual(&self, u: usize) -> Option<usize> {
        u.checked_sub(1)
    }

    /// The next word-boundary unit after `u`, logically.
    pub(crate) fn next_logical_word(&self, u: usize) -> Option<usize> {
        let mut at = u;
        while let Some(next) = self.next_logical(at) {
            if self.units[next].word_boundary {
                return Some(next);
            }
            at = next;
        }
        None
    }

    /// The previous word-boundary unit before `u`, logically.
    pub(crate) fn previous_logical_word(&self, u: usize) -> Option<usize> {
        let mut at = u;
        while let Some(previous) = self.previous_logical(at) {
            if self.units[previous].word_boundary {
                return Some(previous);
            }
            at = previous;
        }
        None
    }

    /// Whether `u` is the last unit of its line on screen.
    pub(crate) fn is_end_of_line(&self, u: usize) -> bool {
        self.lines[self.units[u].line].units.end == u + 1
    }

    /// Whether `u` ends its line at a soft wrap.
    pub(crate) fn is_soft_line_break(&self, u: usize) -> bool {
        self.is_end_of_line(u) && self.lines[self.units[u].line].soft
    }

    /// Whether `u` ends its line at a newline.
    pub(crate) fn is_explicit_line_end(&self, u: usize) -> bool {
        self.is_end_of_line(u) && self.lines[self.units[u].line].explicit
    }

    /// The line whose box contains `y` (the later one on a shared edge; the
    /// first above the layout, the last below it).
    pub(crate) fn line_for_offset(&self, y: f64) -> Option<usize> {
        if self.lines.is_empty() {
            return None;
        }
        if y < 0.0 {
            return Some(0);
        }
        let found = self.lines.binary_search_by(|line| {
            if y < line.top {
                std::cmp::Ordering::Greater
            } else if y >= line.bottom {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        });
        Some(found.unwrap_or_else(|i| i.saturating_sub(1)))
    }

    /// The unit nearest `(x, y)` and whether `x` is on its left half: the line
    /// by `y`, then the unit by `x`, clamped to the line's ends. `None` on a
    /// line with no units (the empty line a final newline opens).
    pub(crate) fn hit(&self, x: f32, y: f32) -> Option<(usize, bool)> {
        let line = &self.lines[self.line_for_offset(f64::from(y))?];
        let last = line.units.end.checked_sub(1)?;
        let mut offset = line.offset;
        for u in line.units.clone() {
            let advance = self.units[u].advance;
            let edge = offset;
            offset += advance;
            if x > offset && u != last {
                continue;
            }
            return Some((u, x <= edge + advance * 0.5));
        }
        None
    }

    /// A caret rectangle `width` wide at `u`'s left edge, or its right edge
    /// when `at_end`.
    pub(crate) fn unit_rect(&self, u: usize, at_end: bool, width: f32) -> Rect {
        let unit = &self.units[u];
        let line = &self.lines[unit.line];
        let x = unit.x + if at_end { unit.advance } else { 0.0 };
        Rect::new(f64::from(x), line.top, f64::from(x + width), line.bottom)
    }

    /// A caret rectangle at the start of the last line.
    pub(crate) fn last_line_rect(&self, width: f32) -> Rect {
        match self.lines.last() {
            Some(line) => Rect::new(
                f64::from(line.offset),
                line.top,
                f64::from(line.offset + width),
                line.bottom,
            ),
            None => Rect::ZERO,
        }
    }
}

/// A position in the text: a byte offset (always a char boundary) and the
/// side it attaches to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Cursor {
    pub(crate) index: usize,
    pub(crate) affinity: Affinity,
}

/// The affinity a caret gets moving onto a unit in direction `moving_right`.
fn affinity_for_dir(rtl: bool, moving_right: bool) -> Affinity {
    if rtl == moving_right {
        Affinity::Downstream
    } else {
        Affinity::Upstream
    }
}

impl Cursor {
    /// The cursor at byte `index` with `affinity`: the end of the text (and
    /// upstream) at or past its length; downstream at its start, which has
    /// nothing upstream. The caller keeps `index` on a char boundary.
    pub(crate) fn from_byte_index(layout: &EditLayout, index: usize, affinity: Affinity) -> Self {
        if index < layout.text_len {
            Self {
                index,
                affinity: if index == 0 {
                    Affinity::Downstream
                } else {
                    affinity
                },
            }
        } else {
            Self {
                index: layout.text_len,
                affinity: Affinity::Upstream,
            }
        }
    }

    /// The cursor nearest the layout-local point `(x, y)`.
    pub(crate) fn from_point(layout: &EditLayout, x: f32, y: f32) -> Self {
        let Some((u, left)) = layout.hit(x, y) else {
            return Self {
                index: layout.text_len,
                affinity: Affinity::Downstream,
            };
        };
        let unit = &layout.units[u];
        let (index, affinity) = if unit.rtl {
            if left {
                (unit.text.end, Affinity::Upstream)
            } else {
                (unit.text.start, Affinity::Downstream)
            }
        } else if left || layout.is_explicit_line_end(u) {
            // Never after a hard break: that position draws on the next line.
            (unit.text.start, Affinity::Downstream)
        } else {
            (unit.text.end, Affinity::Upstream)
        };
        Self { index, affinity }
    }

    /// The cursor at `u`'s start, attached for motion in `moving_right`.
    pub(crate) fn from_unit(layout: &EditLayout, u: usize, moving_right: bool) -> Self {
        let unit = &layout.units[u];
        Self::from_byte_index(
            layout,
            unit.text.start,
            affinity_for_dir(unit.rtl, moving_right),
        )
    }

    /// This cursor within `layout`'s text.
    #[must_use]
    pub(crate) fn refresh(self, layout: &EditLayout) -> Self {
        Self::from_byte_index(layout, self.index, self.affinity)
    }

    /// The units before and after the cursor, logically.
    pub(crate) fn logical_units(self, layout: &EditLayout) -> [Option<usize>; 2] {
        let upstream = self
            .index
            .checked_sub(1)
            .and_then(|index| layout.unit_at(index));
        [upstream, layout.unit_at(self.index)]
    }

    /// The units left and right of the cursor on screen.
    pub(crate) fn visual_units(self, layout: &EditLayout) -> [Option<usize>; 2] {
        let upstream = self
            .index
            .checked_sub(1)
            .and_then(|index| layout.unit_at(index));
        let downstream = layout.unit_at(self.index);
        let around = |u: usize, rtl_side_right: bool| -> [Option<usize>; 2] {
            if rtl_side_right {
                [layout.previous_visual(u), Some(u)]
            } else {
                [Some(u), layout.next_visual(u)]
            }
        };
        let alone = |u: usize| -> [Option<usize>; 2] {
            if layout.units[u].rtl {
                [None, Some(u)]
            } else {
                [Some(u), None]
            }
        };
        if self.affinity == Affinity::Upstream {
            match (upstream, downstream) {
                (Some(u), _) => around(u, layout.units[u].rtl),
                (None, Some(d)) => alone(d),
                (None, None) => [None, None],
            }
        } else {
            match (downstream, upstream) {
                (Some(d), _) => around(d, !layout.units[d].rtl),
                (None, Some(u)) => alone(u),
                (None, None) => [None, None],
            }
        }
    }

    /// The previous caret stop on screen.
    #[must_use]
    pub(crate) fn previous_visual(self, layout: &EditLayout) -> Self {
        let [left, right] = self.visual_units(layout);
        if let (Some(l), Some(r)) = (left, right)
            && layout.is_soft_line_break(l)
        {
            let (lu, ru) = (&layout.units[l], &layout.units[r]);
            if lu.rtl && self.affinity == Affinity::Upstream {
                let index = if ru.rtl { lu.text.start } else { lu.text.end };
                return Self::from_byte_index(layout, index, Affinity::Downstream);
            } else if !lu.rtl && self.affinity == Affinity::Downstream {
                let index = if ru.rtl { ru.text.end } else { ru.text.start };
                return Self::from_byte_index(layout, index, Affinity::Upstream);
            }
        }
        if let Some(l) = left {
            let unit = &layout.units[l];
            let index = if unit.rtl {
                unit.text.end
            } else {
                unit.text.start
            };
            return Self::from_byte_index(layout, index, affinity_for_dir(unit.rtl, false));
        }
        self
    }

    /// The next caret stop on screen.
    #[must_use]
    pub(crate) fn next_visual(self, layout: &EditLayout) -> Self {
        let [left, right] = self.visual_units(layout);
        let to_right = |r: usize| {
            let unit = &layout.units[r];
            let index = if unit.rtl {
                unit.text.start
            } else {
                unit.text.end
            };
            Self::from_byte_index(layout, index, affinity_for_dir(unit.rtl, true))
        };
        if let (Some(l), Some(r)) = (left, right) {
            if layout.is_soft_line_break(l) {
                let (lu, ru) = (&layout.units[l], &layout.units[r]);
                let index = if ru.rtl { ru.text.end } else { ru.text.start };
                if lu.rtl && self.affinity == Affinity::Downstream {
                    return Self::from_byte_index(layout, index, Affinity::Upstream);
                } else if !lu.rtl && self.affinity == Affinity::Upstream {
                    return Self::from_byte_index(layout, index, Affinity::Downstream);
                }
            }
            return to_right(r);
        }
        if let Some(r) = right {
            return to_right(r);
        }
        self
    }

    /// The next word boundary on screen.
    #[must_use]
    pub(crate) fn next_visual_word(self, layout: &EditLayout) -> Self {
        let mut cur = self;
        loop {
            let next = cur.next_visual(layout);
            if next == cur {
                break;
            }
            cur = next;
            let [Some(l), Some(r)] = cur.visual_units(layout) else {
                break;
            };
            let (lu, ru) = (&layout.units[l], &layout.units[r]);
            if lu.rtl {
                if lu.word_boundary && !lu.space_or_nbsp {
                    break;
                }
            } else if ru.word_boundary && !lu.space_or_nbsp {
                break;
            }
        }
        cur
    }

    /// The previous word boundary on screen.
    #[must_use]
    pub(crate) fn previous_visual_word(self, layout: &EditLayout) -> Self {
        let mut cur = self;
        loop {
            let next = cur.previous_visual(layout);
            if next == cur {
                break;
            }
            cur = next;
            let [Some(l), Some(r)] = cur.visual_units(layout) else {
                break;
            };
            let (lu, ru) = (&layout.units[l], &layout.units[r]);
            if lu.rtl {
                if lu.word_boundary && (lu.space_or_nbsp || (ru.word_boundary && !ru.space_or_nbsp))
                {
                    break;
                }
            } else if ru.word_boundary && !ru.space_or_nbsp {
                break;
            }
        }
        cur
    }

    /// The next word boundary in logical order (the text end past the last).
    #[must_use]
    pub(crate) fn next_logical_word(self, layout: &EditLayout) -> Self {
        let [left, right] = self.logical_units(layout);
        let Some(u) = right.or(left) else {
            return self;
        };
        match layout.next_logical_word(u) {
            Some(next) => Self::from_unit(layout, next, true),
            None => Self::from_byte_index(layout, usize::MAX, Affinity::Downstream),
        }
    }

    /// The caret rectangle, `width` wide.
    pub(crate) fn geometry(self, layout: &EditLayout, width: f32) -> Rect {
        match self.visual_units(layout) {
            [Some(l), Some(r)] => {
                if layout.is_end_of_line(l) {
                    if layout.is_soft_line_break(l) {
                        let rtl = layout.units[l].rtl;
                        if rtl && self.affinity == Affinity::Downstream
                            || !rtl && self.affinity == Affinity::Upstream
                        {
                            layout.unit_rect(l, true, width)
                        } else {
                            layout.unit_rect(r, false, width)
                        }
                    } else {
                        layout.unit_rect(r, false, width)
                    }
                } else {
                    layout.unit_rect(l, true, width)
                }
            }
            [Some(l), None] if layout.units[l].newline => layout.last_line_rect(width),
            [Some(l), _] => layout.unit_rect(l, true, width),
            [_, Some(r)] => layout.unit_rect(r, false, width),
            _ => layout.last_line_rect(width),
        }
    }

    /// The line the caret draws on.
    pub(crate) fn line(self, layout: &EditLayout) -> Option<usize> {
        layout.line_for_offset(self.geometry(layout, 0.0).y0)
    }
}
