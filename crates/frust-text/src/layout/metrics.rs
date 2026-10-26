//! Line metrics.
//!
//! Per line, parley 0.11.1's `finish_line` rules over the line's items:
//!
//! - **line height**: the largest of its items' run line heights
//!   ([`crate::LineHeight::resolve`] over the run's font metrics and size);
//! - **ascent/descent**: the largest of its items' run metrics, skipping
//!   all-whitespace items at the logical end; a line of nothing but
//!   whitespace takes its first visual item's, and an empty line (the one a
//!   final newline opens) the previous line's;
//! - **hanging advance**: the whitespace clusters at the line's trailing
//!   visual edge (the last visual item for a left-to-right paragraph, the
//!   first for a right-to-left one), read from that item's logical end for a
//!   left-to-right item and from its logical start for a right-to-left one;
//! - **baseline**: quantized as Chrome and parley do — ascent and descent
//!   rounded separately, the leading's floor half above, the line top rounded.

use super::{LineItem, Paragraph, Source};
use crate::context::shape::Whitespace;

/// One line's vertical and horizontal metrics, in logical pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct LineMetrics {
    /// Baseline to the top of the tallest run's alignment box.
    pub(crate) ascent: f32,
    /// Baseline to the bottom of the deepest run's alignment box (positive).
    pub(crate) descent: f32,
    /// `line_height - (ascent + descent)`; negative for a tight line height.
    pub(crate) leading: f32,
    pub(crate) line_height: f32,
    /// The baseline's `y`, from the top of the layout.
    pub(crate) baseline: f32,
    /// The pen advance of the whole line, hanging whitespace included.
    pub(crate) advance: f32,
    /// The trailing whitespace at the line's end edge, which hangs past the
    /// line box: excluded from the line's width and from alignment.
    pub(crate) hanging_advance: f32,
    /// Where the line's first visual glyph starts, set by alignment.
    pub(crate) offset: f32,
}

impl LineMetrics {
    /// The line's visible width: its advance without the hanging whitespace.
    pub(crate) fn width(&self) -> f32 {
        (self.advance - self.hanging_advance).max(0.0)
    }
}

/// The metrics of a line whose items are `logical` (and `visual`, the same
/// items reordered), whose pen advance is `advance` and whose top is at `y`.
pub(crate) fn line_metrics(
    paragraph: &Paragraph<'_>,
    logical: &[LineItem],
    visual: &[LineItem],
    advance: f32,
    previous: Option<&LineMetrics>,
    y: f64,
) -> LineMetrics {
    let mut m = LineMetrics {
        advance,
        ..LineMetrics::default()
    };
    for item in logical {
        m.line_height = m.line_height.max(paragraph.line_height(item));
    }
    let mut have = false;
    for item in logical.iter().rev() {
        if !have && all_whitespace(paragraph, item) {
            continue;
        }
        let run = paragraph.run(item);
        m.ascent = m.ascent.max(run.metrics.ascent);
        m.descent = m.descent.max(run.metrics.descent);
        have = true;
    }
    if !have {
        if let Some(first) = visual.first() {
            let run = paragraph.metrics_run(first);
            m.ascent = run.metrics.ascent;
            m.descent = run.metrics.descent;
        } else if let Some(previous) = previous {
            m.ascent = previous.ascent;
            m.descent = previous.descent;
            m.line_height = previous.line_height;
        }
    }
    m.hanging_advance = hanging_advance(paragraph, visual);
    m.leading = m.line_height - (m.ascent + m.descent);
    let (ascent, descent) = (m.ascent.round(), m.descent.round());
    let above = ((m.line_height - (ascent + descent)) * 0.5).floor();
    m.baseline = ascent + above + y.round() as f32;
    m
}

/// Whether every cluster of `item` is whitespace.
fn all_whitespace(paragraph: &Paragraph<'_>, item: &LineItem) -> bool {
    item.source == Source::Text
        && paragraph
            .clusters(item)
            .iter()
            .all(|c| c.whitespace != Whitespace::None)
}

/// The whitespace advance at the line's trailing visual edge (see the module
/// docs), continuing into the next item inward while an item is nothing but
/// whitespace (a newline the shaper put in a face of its own).
fn hanging_advance(paragraph: &Paragraph<'_>, visual: &[LineItem]) -> f32 {
    let inward: Box<dyn Iterator<Item = &LineItem>> = if paragraph.is_rtl() {
        Box::new(visual.iter())
    } else {
        Box::new(visual.iter().rev())
    };
    let mut total = 0.0;
    for item in inward {
        let (advance, whole) = hanging(paragraph, item);
        total += advance;
        if !whole {
            break;
        }
    }
    total
}

/// The whitespace advance at `item`'s trailing edge, and whether the whole
/// item is whitespace.
fn hanging(paragraph: &Paragraph<'_>, item: &LineItem) -> (f32, bool) {
    if item.source != Source::Text {
        return (0.0, false);
    }
    let clusters = paragraph.clusters(item);
    let walk: Box<dyn Iterator<Item = &crate::context::shape::ShapedCluster>> =
        if item.bidi_level % 2 == 1 {
            Box::new(clusters.iter())
        } else {
            Box::new(clusters.iter().rev())
        };
    let mut advance = 0.0;
    let mut count = 0;
    for cluster in walk.take_while(|c| c.whitespace != Whitespace::None) {
        advance += cluster.advance;
        count += 1;
    }
    (advance, count == clusters.len())
}
