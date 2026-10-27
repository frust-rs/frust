//! Paragraph alignment: each line's start offset, and justification.
//!
//! Alignment (derived from parley 0.11.1's `align`, with overflowing lines
//! never re-aligned):
//!
//! - the alignment width is the layout's `max_width`, or the widest line's
//!   width when unbounded;
//! - a right-to-left paragraph starts every line at minus its hanging
//!   advance, so the trailing whitespace hangs off the left edge;
//! - a line with no free space keeps the start position (a right-to-left one
//!   is right-aligned by its overflow);
//! - `Start`/`End` follow the paragraph direction, `Left`/`Right` do not;
//! - `Justify` spreads the free space over the line's stretchable spaces,
//!   except on a paragraph's last line, after a newline, or with no spaces
//!   (those start-align). The spaces stretched are the first `num_spaces` in
//!   the walk over items in visual order (reversed for a right-to-left
//!   paragraph), each right-to-left item's clusters from its logical end.

use super::r#break::BreakReason;
use super::{Line, LineItem, Paragraph, Source};
use crate::style::TextAlign;

/// Sets every line's offset (and justification) for `align` across
/// `line_width`.
pub(crate) fn align(
    paragraph: &Paragraph<'_>,
    lines: &mut [Line],
    items: &[LineItem],
    align: TextAlign,
    line_width: f32,
) {
    let rtl = paragraph.is_rtl();
    for line in lines {
        let m = &mut line.metrics;
        m.offset = if rtl { -m.hanging_advance } else { 0.0 };
        line.justify = 0.0;
        line.stretched.clear();
        let free = line_width - m.advance + m.hanging_advance;
        if free <= 0.0 {
            if rtl {
                m.offset += free;
            }
            continue;
        }
        match (align, rtl) {
            (TextAlign::Left, _) | (TextAlign::Start, false) | (TextAlign::End, true) => {}
            (TextAlign::Right, _) | (TextAlign::Start, true) | (TextAlign::End, false) => {
                m.offset += free;
            }
            (TextAlign::Center, _) => m.offset += free * 0.5,
            (TextAlign::Justify, _) => {
                if matches!(line.reason, BreakReason::None | BreakReason::Explicit)
                    || line.num_spaces == 0
                {
                    if rtl {
                        m.offset += free;
                    }
                    continue;
                }
                line.justify = free / line.num_spaces as f32;
                line.stretched = stretched_spaces(paragraph, line, items, rtl);
            }
        }
    }
}

/// The text clusters a justified `line` stretches, sorted.
fn stretched_spaces(
    paragraph: &Paragraph<'_>,
    line: &Line,
    items: &[LineItem],
    rtl: bool,
) -> Vec<u32> {
    let mut out = Vec::with_capacity(line.num_spaces);
    let line_items = &items[line.items.clone()];
    let ordered: Box<dyn Iterator<Item = &LineItem>> = if rtl {
        Box::new(line_items.iter().rev())
    } else {
        Box::new(line_items.iter())
    };
    'items: for item in ordered.filter(|item| item.source == Source::Text) {
        let clusters = item.clusters.clone();
        let walk: Box<dyn Iterator<Item = usize>> = if item.bidi_level % 2 == 1 {
            Box::new(clusters.rev())
        } else {
            Box::new(clusters)
        };
        for ci in walk {
            if out.len() == line.num_spaces {
                break 'items;
            }
            if paragraph.shaped.clusters[ci].whitespace.is_space_or_nbsp() {
                out.push(u32::try_from(ci).unwrap_or(u32::MAX));
            }
        }
    }
    out.sort_unstable();
    out
}
