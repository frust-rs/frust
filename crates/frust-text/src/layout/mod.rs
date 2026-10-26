//! [`TextLayout`]: a laid-out, line-broken, aligned block of text, and the
//! frust line layout that builds it over a [`Shaped`] paragraph.
//!
//! The pipeline, one module per stage, each working on atoms and clusters of
//! the width-independent shaping result (nothing here reshapes):
//!
//! 1. [`break`](self::r#break): greedy breaking into lines of atoms, optionally
//!    capped at a line count;
//! 2. [`truncate`]: the capped last line's native ellipsis cut, refilled
//!    from the rest of its paragraph past its soft-wrap point;
//! 3. per line, its items (maximal same-run atom spans) in logical order, and
//!    [`reorder`] into visual order (UAX #9 L2);
//! 4. [`metrics`]: line height, ascent/descent, hanging whitespace and the
//!    quantized baseline;
//! 5. [`align`]: each line's start offset and justification.
//!
//! The glyph emission over the finished [`LineTable`] lives in
//! [`crate::convert`]. A width change re-runs all five stages on the cached
//! [`Shaped`] (see [`crate::shape_cache`]).

mod align;
mod r#break;
mod metrics;
mod reorder;
mod truncate;

use std::cell::RefCell;
use std::ops::Range;
use std::sync::Arc;

use kurbo::{Affine, Point, Size};

use frust_scene::GlyphRun;

use crate::context::shape::{Shaped, ShapedCluster, ShapedRun};
use crate::style::TextStyle;
use r#break::{BreakReason, BrokenLine, OverflowWrap, break_lines};
pub(crate) use metrics::LineMetrics;
pub(crate) use truncate::ELLIPSIS;

/// A source-text byte range plus rendered width for one line of a
/// [`TextLayout`], used only by [`crate::context::TextContext::layout_bounded`]'s
/// overflow checks.
pub(crate) struct LineInfo {
    /// The line's text range in the *original* source string passed to
    /// [`crate::TextContext::layout`]/`layout_bounded` — every line takes a
    /// contiguous, non-overlapping span of whole atoms, so concatenating every
    /// line's range in order reconstructs the source. The empty line a final
    /// newline opens is `len..len`.
    pub range: Range<usize>,
    /// The line's rendered advance, excluding trailing whitespace (which
    /// hangs at the line edge and would otherwise make a legitimately
    /// fitting line look like it overflows).
    pub width: f32,
}

/// Which shaping result a [`LineItem`] reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Source {
    /// The paragraph's own text.
    Text,
    /// The separately shaped ellipsis of a truncated line.
    Ellipsis,
}

/// A maximal span of one line within one shaped run.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LineItem {
    pub(crate) source: Source,
    /// Index into the source's runs.
    pub(crate) run: usize,
    /// Index range into the source's clusters, logical order.
    pub(crate) clusters: Range<usize>,
    pub(crate) bidi_level: u8,
}

/// One laid-out line.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Line {
    /// The text atoms on the line (logical order).
    pub(crate) atoms: Range<usize>,
    /// Source byte range.
    pub(crate) text: Range<usize>,
    /// Index range into [`LineTable::items`], in visual order.
    pub(crate) items: Range<usize>,
    pub(crate) reason: BreakReason,
    pub(crate) num_spaces: usize,
    pub(crate) metrics: LineMetrics,
    /// The extra advance each stretched space gets on a justified line.
    pub(crate) justify: f32,
    /// The text clusters that get [`Self::justify`], sorted.
    pub(crate) stretched: Vec<u32>,
}

/// Every line of a laid-out paragraph.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct LineTable {
    pub(crate) lines: Vec<Line>,
    pub(crate) items: Vec<LineItem>,
    /// The widest line's width, hanging whitespace excluded.
    pub(crate) width: f32,
    /// The sum of the line heights.
    pub(crate) height: f32,
}

/// A separately shaped ellipsis and the style (span) index it was shaped in.
#[derive(Clone, Debug)]
pub(crate) struct Ellipsis {
    pub(crate) shaped: Arc<Shaped>,
    pub(crate) style_index: u16,
}

/// The inputs of a line layout: the shaped paragraph, its span styles (by
/// style index), and an ellipsis to cut a capped last line with.
pub(crate) struct Paragraph<'a> {
    pub(crate) shaped: &'a Shaped,
    pub(crate) styles: &'a [TextStyle],
    pub(crate) ellipsis: Option<&'a Ellipsis>,
    /// What a word wider than the line does: always [`OverflowWrap::Normal`]
    /// outside tests until a style knob selects otherwise.
    pub(crate) overflow_wrap: OverflowWrap,
}

impl<'a> Paragraph<'a> {
    /// # Panics
    ///
    /// When `styles` is empty: every character's style index (and the
    /// ellipsis's) indexes it, so [`Self::style`] indexes directly and an
    /// out-of-range index is a bug that panics rather than a silent remap.
    pub(crate) fn new(
        shaped: &'a Shaped,
        styles: &'a [TextStyle],
        ellipsis: Option<&'a Ellipsis>,
    ) -> Self {
        assert!(!styles.is_empty(), "a paragraph has at least one style");
        debug_assert!(
            shaped
                .runs
                .iter()
                .all(|r| usize::from(r.style_index) < styles.len())
                && ellipsis.is_none_or(|e| usize::from(e.style_index) < styles.len()),
            "a style index is out of range of the paragraph's styles"
        );
        Self {
            shaped,
            styles,
            ellipsis,
            overflow_wrap: OverflowWrap::Normal,
        }
    }

    /// Whether the paragraph's base direction is right to left.
    pub(crate) fn is_rtl(&self) -> bool {
        self.shaped.paragraph_level % 2 == 1
    }

    /// The shaping result `source` names.
    pub(crate) fn source(&self, source: Source) -> &'a Shaped {
        match (source, self.ellipsis) {
            (Source::Ellipsis, Some(e)) => &e.shaped,
            _ => self.shaped,
        }
    }

    pub(crate) fn run(&self, item: &LineItem) -> &'a ShapedRun {
        &self.source(item.source).runs[item.run]
    }

    pub(crate) fn clusters(&self, item: &LineItem) -> &'a [ShapedCluster] {
        &self.source(item.source).clusters[item.clusters.clone()]
    }

    /// The style a character of `source` with span index `style_index` uses.
    pub(crate) fn style(&self, source: Source, style_index: u16) -> &'a TextStyle {
        let index = match (source, self.ellipsis) {
            (Source::Ellipsis, Some(e)) => e.style_index,
            _ => style_index,
        };
        &self.styles[usize::from(index)]
    }

    /// The run whose font metrics size `item`: its own, except that a run of
    /// nothing but newlines takes the adjacent run's (the one before it, else
    /// the one after) when that run is in the same style.
    ///
    /// A newline draws nothing, so the face the shaper picked for it is
    /// arbitrary — it may not be the text's own face when that face does not
    /// map U+000A. parley shapes a newline in the surrounding text's face, and
    /// so sizes an empty line by it; this keeps that.
    pub(crate) fn metrics_run(&self, item: &LineItem) -> &'a ShapedRun {
        let shaped = self.source(item.source);
        let run = &shaped.runs[item.run];
        if item.source != Source::Text || !newline_only(shaped, run) {
            return run;
        }
        let neighbour = |index: Option<usize>| {
            index
                .and_then(|i| shaped.runs.get(i))
                .filter(|r| r.style_index == run.style_index && !newline_only(shaped, r))
        };
        neighbour(item.run.checked_sub(1))
            .or_else(|| neighbour(item.run.checked_add(1)))
            .unwrap_or(run)
    }

    /// The line height `item`'s run asks for.
    pub(crate) fn line_height(&self, item: &LineItem) -> f32 {
        let run = self.metrics_run(item);
        let m = &run.metrics;
        self.style(item.source, run.style_index)
            .line_height
            .resolve(run.font_size, m.ascent, m.descent, m.leading)
    }

    /// The ellipsis's total advance (`0` with no ellipsis).
    fn ellipsis_advance(&self) -> f32 {
        self.ellipsis
            .map_or(0.0, |e| e.shaped.runs.iter().map(|r| r.advance).sum())
    }
}

/// Whether every cluster of `run` is a newline.
fn newline_only(shaped: &Shaped, run: &ShapedRun) -> bool {
    shaped.clusters[run.clusters.start as usize..run.clusters.end as usize]
        .iter()
        .all(|c| c.whitespace == crate::context::shape::Whitespace::Newline)
}

/// A line cap: lay out at most `max_lines` lines, the last one ending the
/// paragraph (and cut with the paragraph's ellipsis when `ellipsize`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Cap {
    pub(crate) max_lines: usize,
    pub(crate) ellipsize: bool,
}

/// Lays `paragraph` out into lines no wider than `max_width` (unbounded when
/// `None`), aligned by its first style's [`TextStyle::align`].
pub(crate) fn lay_out(
    paragraph: &Paragraph<'_>,
    max_width: Option<f32>,
    cap: Option<Cap>,
) -> LineTable {
    let shaped = paragraph.shaped;
    let mut broken = break_lines(
        shaped,
        max_width,
        cap.map(|c| c.max_lines),
        paragraph.overflow_wrap,
    );
    // The ellipsized line's index and its kept text's exclusive end cluster.
    // The cut may extend the last line's atoms past its break (the refill);
    // the cap already dropped every line after it, so none overlap.
    let mut ellipsized: Option<(usize, u32)> = None;
    if let (Some(cap), Some(last)) = (cap, broken.last_mut()) {
        last.reason = BreakReason::None;
        if cap.ellipsize && paragraph.ellipsis.is_some() {
            let end_cluster =
                truncate::ellipsize(shaped, last, max_width, paragraph.ellipsis_advance());
            ellipsized = Some((broken.len() - 1, end_cluster));
        }
    }

    let mut table = LineTable::default();
    let mut y = 0.0f64;
    let mut previous: Option<LineMetrics> = None;
    for (index, line) in broken.iter().enumerate() {
        let start = table.items.len();
        let cut = ellipsized.and_then(|(i, end)| (i == index).then_some(end));
        push_text_items(shaped, line, cut, &mut table.items);
        let mut advance = line.advance;
        if cut.is_some()
            && let Some(ellipsis) = paragraph.ellipsis
        {
            for (run_index, run) in ellipsis.shaped.runs.iter().enumerate() {
                table.items.push(LineItem {
                    source: Source::Ellipsis,
                    run: run_index,
                    clusters: run.clusters.start as usize..run.clusters.end as usize,
                    bidi_level: shaped.paragraph_level,
                });
            }
            advance += paragraph.ellipsis_advance();
        }
        let logical = table.items[start..].to_vec();
        reorder::reorder(&mut table.items[start..], |item| item.bidi_level);
        let metrics = metrics::line_metrics(
            paragraph,
            &logical,
            &table.items[start..],
            advance,
            previous.as_ref(),
            y,
        );
        y += f64::from(metrics.line_height);
        previous = Some(metrics);
        table.lines.push(Line {
            atoms: line.atoms.clone(),
            text: text_range(shaped, line, cut),
            items: start..table.items.len(),
            reason: line.reason,
            num_spaces: line.num_spaces,
            metrics,
            justify: 0.0,
            stretched: Vec::new(),
        });
    }
    table.width = table
        .lines
        .iter()
        .map(|l| l.metrics.width())
        .fold(0.0, f32::max);
    table.height = y as f32;
    let align = paragraph
        .styles
        .first()
        .map(|s| s.align)
        .unwrap_or_default();
    let line_width = max_width.unwrap_or(table.width);
    align::align(paragraph, &mut table.lines, &table.items, align, line_width);
    table
}

/// Appends `line`'s text items in logical order: one per run its atoms touch.
/// A `cut` ends the line's last item at that cluster, inside its last atom.
fn push_text_items(shaped: &Shaped, line: &BrokenLine, cut: Option<u32>, out: &mut Vec<LineItem>) {
    let mut a = line.atoms.start;
    while a < line.atoms.end {
        let run_index = shaped
            .runs
            .partition_point(|r| r.atoms.end as usize <= a)
            .min(shaped.runs.len().saturating_sub(1));
        let Some(run) = shaped.runs.get(run_index) else {
            return;
        };
        let end = line.atoms.end.min(run.atoms.end as usize).max(a + 1);
        let first = &shaped.atoms[a];
        let last = &shaped.atoms[end - 1];
        let mut last_cluster = last.clusters.end;
        if end == line.atoms.end
            && let Some(cut) = cut
        {
            last_cluster = last_cluster.min(cut);
        }
        out.push(LineItem {
            source: Source::Text,
            run: run_index,
            clusters: first.clusters.start as usize..last_cluster as usize,
            bidi_level: run.bidi_level,
        });
        a = end;
    }
}

/// `line`'s source byte range.
fn text_range(shaped: &Shaped, line: &BrokenLine, cut: Option<u32>) -> Range<usize> {
    let byte_of = |char_index: u32| {
        shaped
            .chars
            .get(char_index as usize)
            .map_or(shaped.text_len, |c| c.byte_start as usize)
    };
    match (
        shaped.atoms.get(line.atoms.start),
        line.atoms
            .end
            .checked_sub(1)
            .and_then(|i| shaped.atoms.get(i)),
    ) {
        (Some(first), Some(last)) if !line.atoms.is_empty() => {
            let end = cut
                .and_then(|c| shaped.clusters.get((c as usize).checked_sub(1)?))
                .map_or(last.chars.end, |c| c.chars.end.min(last.chars.end));
            byte_of(first.chars.start)..byte_of(end)
        }
        (Some(first), _) => {
            let at = byte_of(first.chars.start);
            at..at
        }
        _ => shaped.text_len..shaped.text_len,
    }
}

/// A finished text layout: positioned glyph runs plus a cached measured size.
///
/// Produced by [`crate::TextContext::layout`]. It borrows nothing from the
/// context, so it can outlive the layout pass and be measured or converted to
/// scene runs on demand.
pub struct TextLayout {
    text: Arc<str>,
    shaped: Arc<Shaped>,
    styles: Arc<[TextStyle]>,
    max_width: Option<f32>,
    lines: Arc<LineTable>,
    ellipsis: Option<Ellipsis>,
    size: Size,
    /// Memoized origin-independent glyph runs, built lazily on the first
    /// [`to_scene_runs`](Self::to_scene_runs) call and reused across every
    /// later call while this layout is retained. The only
    /// origin-dependent field of a [`GlyphRun`] is its `transform`, so a paint
    /// at any origin clones these base runs and re-translates rather than
    /// re-walking the line table (font handles / glyph positions) every
    /// frame. Retaining the [`TextLayout`] across frames — which the
    /// [`Text`](../../frust_widgets/struct.TextWidget.html) widget now does when
    /// content/style/width are unchanged — is what makes this reuse effective.
    base_runs: RefCell<Option<Vec<GlyphRun>>>,
}

impl TextLayout {
    /// Wraps a finished line table over `shaped` (`text` styled by `styles`,
    /// broken at `max_width`), caching its measured size. An empty `text` is
    /// shaped as a single space that sizes its one line and never draws.
    pub(crate) fn new(
        text: Arc<str>,
        shaped: Arc<Shaped>,
        styles: Arc<[TextStyle]>,
        max_width: Option<f32>,
        lines: Arc<LineTable>,
    ) -> Self {
        Self::with_ellipsis(text, shaped, styles, max_width, lines, None)
    }

    fn with_ellipsis(
        text: Arc<str>,
        shaped: Arc<Shaped>,
        styles: Arc<[TextStyle]>,
        max_width: Option<f32>,
        lines: Arc<LineTable>,
        ellipsis: Option<Ellipsis>,
    ) -> Self {
        let size = Size::new(f64::from(lines.width), f64::from(lines.height));
        Self {
            text,
            shaped,
            styles,
            max_width,
            lines,
            ellipsis,
            size,
            base_runs: RefCell::new(None),
        }
    }

    /// This layout capped at `max_lines` lines, the last one ending the
    /// paragraph and, given an `ellipsis`, natively truncated to fit with it
    /// (refilled from the text after it up to the next mandatory break, so it
    /// may end past where the uncapped layout wrapped it). Re-breaks the same
    /// [`Shaped`]; earlier lines come out identical (breaking is greedy).
    pub(crate) fn truncated(&self, max_lines: usize, ellipsis: Option<Ellipsis>) -> Self {
        let cap = Cap {
            max_lines,
            ellipsize: ellipsis.is_some(),
        };
        let lines = {
            let paragraph = Paragraph::new(&self.shaped, &self.styles, ellipsis.as_ref());
            lay_out(&paragraph, self.max_width, Some(cap))
        };
        Self::with_ellipsis(
            Arc::clone(&self.text),
            Arc::clone(&self.shaped),
            Arc::clone(&self.styles),
            self.max_width,
            Arc::new(lines),
            ellipsis,
        )
    }

    /// The shaped paragraph this layout breaks.
    pub(crate) fn shaped(&self) -> &Arc<Shaped> {
        &self.shaped
    }

    /// The measured size (width x height) of the laid-out block.
    ///
    /// Width is the longest line's advance without its hanging whitespace
    /// (which can exceed the layout's `max_width` when a line overflows);
    /// height is the sum of line heights.
    pub fn size(&self) -> Size {
        self.size
    }

    /// Converts the layout into [`frust_scene::GlyphRun`]s placed at `origin`.
    ///
    /// See [`crate::convert`] for the coordinate convention. The origin-invariant
    /// run construction (the line-table walk) is memoized on first call and
    /// reused across frames (see [`base_runs`](Self::base_runs)); each call
    /// re-applies only the cheap `origin` translation onto the cached runs'
    /// transforms.
    ///
    /// A caller supplying its own `run.brush` after this call (a gradient
    /// foreground, say) must resolve that brush's geometry relative to
    /// `origin` too — never as an already-window-space point. The render
    /// backends apply a `GlyphRun`'s `transform` to its active paint, not
    /// just its glyph outlines (vello's `Scene::draw_glyphs(..).transform(t)`
    /// composes `t` with the brush the same way it composes `t` with the
    /// glyphs; `vello_cpu`'s `set_transform` likewise precedes `set_paint`),
    /// so a brush baked with `origin` already added gets it added a second
    /// time at paint time. Every other [`frust_scene::Command`] variant's
    /// `origin`/position argument does *not* auto-translate its brush this
    /// way (see the `frust-material` gradient-button decoration's own
    /// `PaintScene` doc note) — this contract is specific to
    /// [`frust_scene::Command::GlyphRun`].
    pub fn to_scene_runs(&self, origin: Point) -> Vec<GlyphRun> {
        let mut memo = self.base_runs.borrow_mut();
        let base = memo.get_or_insert_with(|| {
            if self.text.is_empty() {
                // Only the sizing space of an empty text is shaped: nothing draws.
                return Vec::new();
            }
            let paragraph = Paragraph::new(&self.shaped, &self.styles, self.ellipsis.as_ref());
            crate::convert::lines_to_scene_runs(&paragraph, &self.text, &self.lines)
        });
        let transform = Affine::translate((origin.x, origin.y));
        base.iter()
            .map(|run| GlyphRun {
                transform,
                ..run.clone()
            })
            .collect()
    }

    /// The number of lines in this layout — at least `1` (an empty text is
    /// one empty line). `pub(crate)` — the `max_lines`/overflow walk in
    /// [`crate::context`] is the sole caller.
    pub(crate) fn line_count(&self) -> usize {
        self.lines.lines.len()
    }

    /// The source text range and rendered (trailing-whitespace-excluded)
    /// width of line `index`, or `None` if out of bounds. `pub(crate)` — see
    /// [`Self::line_count`].
    pub(crate) fn line_info(&self, index: usize) -> Option<LineInfo> {
        let line = self.lines.lines.get(index)?;
        let len = self.text.len();
        Some(LineInfo {
            range: line.text.start.min(len)..line.text.end.min(len),
            width: line.metrics.width(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::shape::ShapeContext;
    use crate::oracle::synthetic::SyntheticBackend;
    use crate::oracle::{Oracle, RenderCapabilities};
    use crate::style::{GenericSlot, LineHeight, TextAlign};
    use peniko::{Blob, Color};

    const NOTO_SANS: &[u8] = include_bytes!("../../../../testing/fonts/NotoSans-Subset.ttf");

    /// An oracle whose one face (the Latin subset) claims every character, so
    /// any script shapes (as `.notdef` outside the subset) with real bidi
    /// levels.
    fn cover_all_oracle() -> Oracle {
        let blob = Blob::from(NOTO_SANS.to_vec());
        let mut backend = SyntheticBackend::new("layout-tests");
        for slot in [GenericSlot::SystemUi, GenericSlot::SansSerif] {
            backend.default_for(slot, &blob);
        }
        let all: String = (' '..='\u{FFFF}').collect();
        backend.cover(&blob, &all);
        Oracle::new(Box::new(backend), RenderCapabilities::default(), None)
    }

    fn style(size: f32) -> TextStyle {
        TextStyle::new(size, Color::BLACK)
    }

    fn laid_out(text: &str, style: &TextStyle, max_width: Option<f32>) -> TextLayout {
        let mut oracle = cover_all_oracle();
        let shaped = Arc::new(ShapeContext::default().shape(&mut oracle, text, style));
        let styles: Arc<[TextStyle]> = Arc::from([style.clone()]);
        let lines = lay_out(&Paragraph::new(&shaped, &styles, None), max_width, None);
        TextLayout::new(Arc::from(text), shaped, styles, max_width, Arc::new(lines))
    }

    /// The first character of every cluster, in the order the glyph emitter
    /// walks them (visual order).
    fn visual_chars(layout: &TextLayout, line: usize) -> String {
        let shaped = &layout.shaped;
        let line = &layout.lines.lines[line];
        let mut out = String::new();
        for item in &layout.lines.items[line.items.clone()] {
            let walk: Vec<usize> = if item.bidi_level % 2 == 1 {
                item.clusters.clone().rev().collect()
            } else {
                item.clusters.clone().collect()
            };
            for ci in walk {
                let byte = shaped.chars[shaped.clusters[ci].chars.start as usize].byte_start;
                out.extend(layout.text[byte as usize..].chars().next());
            }
        }
        out
    }

    #[test]
    fn mixed_bidi_reorders_into_visual_order() {
        let layout = laid_out("abc \u{5d0}\u{5d1}\u{5d2} def", &style(16.0), None);
        assert_eq!(
            layout.shaped.paragraph_level, 0,
            "the first strong is Latin"
        );
        assert_eq!(
            visual_chars(&layout, 0),
            "abc \u{5d2}\u{5d1}\u{5d0} def",
            "the Hebrew run is drawn right to left between the Latin runs"
        );
        let runs = layout.to_scene_runs(Point::ORIGIN);
        let xs: Vec<f32> = runs
            .iter()
            .flat_map(|r| r.glyphs.iter().map(|g| g.x))
            .collect();
        assert!(
            xs.windows(2).all(|w| w[0] < w[1]),
            "glyphs advance left to right in emission order: {xs:?}"
        );
    }

    #[test]
    fn an_rtl_paragraph_reverses_the_whole_line() {
        let layout = laid_out("\u{5d0}\u{5d1} abc \u{5d2}", &style(16.0), None);
        assert_eq!(layout.shaped.paragraph_level, 1);
        assert_eq!(visual_chars(&layout, 0), "\u{5d2} abc \u{5d1}\u{5d0}");
    }

    #[test]
    fn the_metrics_follow_the_line_height_mode() {
        let natural = laid_out("Hello\nHello", &style(20.0), None);
        let line = natural.lines.lines[0].metrics;
        assert!(line.ascent > 0.0 && line.descent > 0.0);
        assert!((line.line_height - (line.ascent + line.descent + line.leading)).abs() < 1e-4);
        let absolute = laid_out(
            "Hello\nHello",
            &TextStyle {
                line_height: LineHeight::Absolute(50.0),
                ..style(20.0)
            },
            None,
        );
        assert_eq!(absolute.size().height, 100.0);
        let lines = &absolute.lines.lines;
        assert_eq!(lines[1].metrics.baseline - lines[0].metrics.baseline, 50.0);
        let relative = laid_out(
            "Hello",
            &TextStyle {
                line_height: LineHeight::FontSizeRelative(2.0),
                ..style(20.0)
            },
            None,
        );
        assert_eq!(relative.size().height, 40.0);
    }

    #[test]
    fn a_final_newline_opens_an_empty_line_with_the_previous_metrics() {
        let layout = laid_out("Hello\n", &style(20.0), None);
        let lines = &layout.lines.lines;
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1].text, 6..6);
        assert_eq!(lines[1].metrics.line_height, lines[0].metrics.line_height);
        let empty = laid_out("", &style(20.0), Some(10.0));
        assert_eq!(empty.line_count(), 1);
        assert_eq!(
            empty.size(),
            Size::ZERO,
            "a bare empty paragraph has no metrics"
        );
    }

    #[test]
    fn justify_fills_every_line_but_the_last() {
        let text = "Hello hello Hello hello Hello hello";
        let s = TextStyle {
            align: TextAlign::Justify,
            ..style(18.0)
        };
        let layout = laid_out(text, &s, Some(130.0));
        let lines = &layout.lines.lines;
        assert!(lines.len() > 1);
        for line in &lines[..lines.len() - 1] {
            let stretched = line.metrics.width() + line.justify * line.num_spaces as f32;
            assert!((stretched - 130.0).abs() < 1e-3, "{stretched}");
            assert_eq!(line.stretched.len(), line.num_spaces);
        }
        assert_eq!(lines.last().map(|l| l.justify), Some(0.0));
    }

    #[test]
    fn a_long_word_overflows_unless_the_paragraph_wraps_anywhere() {
        let s = style(18.0);
        let mut oracle = cover_all_oracle();
        let shaped = ShapeContext::default().shape(&mut oracle, "Hellooooooo Hello", &s);
        let styles = [s.clone()];
        let mut paragraph = Paragraph::new(&shaped, &styles, None);
        let normal = lay_out(&paragraph, Some(40.0), None);
        assert_eq!(
            normal.lines.len(),
            2,
            "the long word overflows its own line"
        );
        assert!(normal.width > 40.0);
        paragraph.overflow_wrap = OverflowWrap::Anywhere;
        let anywhere = lay_out(&paragraph, Some(40.0), None);
        assert!(anywhere.lines.len() > 2);
        assert!(anywhere.lines[0].reason == BreakReason::Emergency);
        assert!(anywhere.width <= 40.0);
    }

    #[test]
    fn a_capped_ellipsis_line_ends_with_the_ellipsis_glyph() {
        let s = style(18.0);
        let full = laid_out("Hello hello Hello hello", &s, Some(60.0));
        let mut oracle = cover_all_oracle();
        let ellipsis = Ellipsis {
            shaped: Arc::new(ShapeContext::default().shape(&mut oracle, "\u{2026}", &s)),
            style_index: 0,
        };
        let cut = full.truncated(1, Some(ellipsis));
        assert_eq!(cut.line_count(), 1);
        let line = &cut.lines.lines[0];
        assert_eq!(line.reason, BreakReason::None);
        let last = &cut.lines.items[line.items.end - 1];
        assert_eq!(last.source, Source::Ellipsis);
        assert!(line.metrics.width() <= 60.0);
    }
}
