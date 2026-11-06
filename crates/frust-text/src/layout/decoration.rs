//! Underline and line-through rectangles for a [`TextLayout`].
//!
//! [`TextLayout::decoration_rects`] turns each style's [`TextDecoration`] into
//! filled rectangles in layout coordinates. The renderer draws them with plain
//! rectangle fills (no scene command of their own): underlines under the glyph
//! runs, line-throughs over them.
//!
//! # Placement
//!
//! One rectangle per contiguous same-style, same-face segment of a line. Its
//! horizontal extent is the segment's pen span from the shared
//! [`crate::convert::walk_clusters`] walk, so it follows bidi order, justified
//! spacing and the ellipsis exactly as the glyphs do. Whitespace hanging at the
//! line's trailing edge is not decorated.
//!
//! The vertical position comes from the face's own metrics at the run's size
//! ([`crate::shape::RunMetrics`]): each offset is the distance from the
//! baseline up to the top of the stroke (positive above the baseline), each
//! size the stroke thickness. [`crate::TextDecoration::with_thickness`]
//! overrides the thickness and keeps the stroke's top. A face that reports a
//! zero size for a stroke gets a fallback: a thickness of `font_size / 14`,
//! an underline whose top sits one thickness below the baseline, and a
//! line-through centred `0.35 * ascent` above it.

use kurbo::{Point, Rect};
use peniko::Color;

use super::{Paragraph, Source, TextLayout};
use crate::convert::{Step, walk_clusters};
use crate::shape::RunMetrics;
use crate::style::TextStyle;

/// Which line a [`DecorationRect`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecorationKind {
    /// A line under the text, drawn beneath the glyphs.
    Underline,
    /// A line through the text, drawn over the glyphs.
    LineThrough,
}

/// One filled rectangle of a text decoration.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DecorationRect {
    /// The rectangle, translated by the origin given to
    /// [`TextLayout::decoration_rects`].
    pub rect: Rect,
    /// The fill colour.
    pub color: Color,
    /// Which line this is.
    pub kind: DecorationKind,
}

/// A cluster's pen extent and the style and face it draws with.
struct Piece {
    source: Source,
    run: usize,
    style: u16,
    start: f32,
    end: f32,
    whitespace: bool,
}

/// A contiguous run of pieces drawn alike.
struct Segment {
    source: Source,
    run: usize,
    style: u16,
    start: f32,
    end: f32,
}

impl TextLayout {
    /// The underline and line-through rectangles of this layout placed at
    /// `origin`, in line order; none when no style is decorated.
    ///
    /// Each one spans a segment of one line drawn in one style and face; see
    /// the module docs for the placement rules. Draw the
    /// [`DecorationKind::Underline`] rectangles before the glyph runs of
    /// [`Self::to_scene_runs`] and the [`DecorationKind::LineThrough`] ones
    /// after.
    pub fn decoration_rects(&self, origin: Point) -> Vec<DecorationRect> {
        if self.text.is_empty() || self.styles.iter().all(|s| s.decoration.is_none()) {
            return Vec::new();
        }
        let paragraph = Paragraph::new(&self.shaped, &self.styles, self.ellipsis.as_ref());
        let mut lines: Vec<Vec<Piece>> = Vec::new();
        lines.resize_with(self.lines.lines.len(), Vec::new);
        walk_clusters(&paragraph, &self.lines, |step| {
            if let Step::Cluster(c) = step {
                lines[c.line_index].push(Piece {
                    source: c.item.source,
                    run: c.item.run,
                    style: c.style,
                    start: c.pen,
                    end: c.end,
                    whitespace: c.cluster.whitespace != crate::shape::Whitespace::None,
                });
            }
        });

        let mut out = Vec::new();
        for (line, mut pieces) in self.lines.lines.iter().zip(lines) {
            // The whitespace hanging at the trailing visual edge is not drawn on.
            if paragraph.is_rtl() {
                let keep = pieces.iter().take_while(|p| p.whitespace).count();
                pieces.drain(..keep);
            } else {
                while pieces.last().is_some_and(|p| p.whitespace) {
                    pieces.pop();
                }
            }
            let mut segments: Vec<Segment> = Vec::new();
            let mut open: Option<Segment> = None;
            for p in &pieces {
                let decorated = !paragraph.style(p.source, p.style).decoration.is_none();
                match open.as_mut() {
                    Some(s)
                        if decorated
                            && s.source == p.source
                            && s.run == p.run
                            && s.style == p.style =>
                    {
                        s.start = s.start.min(p.start);
                        s.end = s.end.max(p.end);
                    }
                    _ => {
                        segments.extend(open.take());
                        if decorated {
                            open = Some(Segment {
                                source: p.source,
                                run: p.run,
                                style: p.style,
                                start: p.start,
                                end: p.end,
                            });
                        }
                    }
                }
            }
            segments.extend(open);

            for s in segments.iter().filter(|s| s.end > s.start) {
                let style = paragraph.style(s.source, s.style);
                let run = &paragraph.source(s.source).runs[s.run];
                push_rects(
                    &mut out,
                    style,
                    &run.metrics,
                    run.font_size,
                    origin,
                    line.metrics.baseline,
                    (s.start, s.end),
                );
            }
        }
        out
    }
}

fn push_rects(
    out: &mut Vec<DecorationRect>,
    style: &TextStyle,
    m: &RunMetrics,
    font_size: f32,
    origin: Point,
    baseline: f32,
    (start, end): (f32, f32),
) {
    let dec = &style.decoration;
    let color = dec.color.unwrap_or(style.color);
    let fallback = font_size / 14.0;
    let mut push = |kind, top_above_baseline: f32, thickness: f32| {
        let top = f64::from(baseline - top_above_baseline) + origin.y;
        out.push(DecorationRect {
            rect: Rect::new(
                f64::from(start) + origin.x,
                top,
                f64::from(end) + origin.x,
                top + f64::from(thickness),
            ),
            color,
            kind,
        });
    };
    if dec.underline {
        let (offset, size) = if m.underline_size > 0.0 {
            (m.underline_offset, m.underline_size)
        } else {
            (-fallback, fallback)
        };
        push(
            DecorationKind::Underline,
            offset,
            dec.thickness.unwrap_or(size),
        );
    }
    if dec.line_through {
        let (offset, size) = if m.strikeout_size > 0.0 {
            (m.strikeout_offset, m.strikeout_size)
        } else {
            (m.ascent * 0.35 + fallback / 2.0, fallback)
        };
        push(
            DecorationKind::LineThrough,
            offset,
            dec.thickness.unwrap_or(size),
        );
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::convert::RunMemo;
    use crate::layout::lay_out;
    use crate::oracle::synthetic::SyntheticBackend;
    use crate::oracle::{Oracle, RenderCapabilities};
    use crate::shape::ShapeContext;
    use crate::style::{GenericSlot, TextAlign, TextDecoration};
    use peniko::Blob;

    const NOTO_SANS: &[u8] = include_bytes!("../../../../testing/fonts/NotoSans-Subset.ttf");
    const NOTO_ARABIC: &[u8] =
        include_bytes!("../../../../testing/fonts/NotoSansArabic-Subset.ttf");

    /// An oracle whose one face claims every character.
    fn oracle_of(face: &[u8]) -> Oracle {
        let blob = Blob::from(face.to_vec());
        let mut backend = SyntheticBackend::new("decoration-tests");
        for slot in [GenericSlot::SystemUi, GenericSlot::SansSerif] {
            backend.default_for(slot, &blob);
        }
        let all: String = (' '..='\u{FFFF}').collect();
        backend.cover(&blob, &all);
        Oracle::new(Box::new(backend), RenderCapabilities::default(), None)
    }

    fn laid_out(face: &[u8], text: &str, style: &TextStyle, width: Option<f32>) -> TextLayout {
        let mut oracle = oracle_of(face);
        let shaped = Arc::new(ShapeContext::default().shape(&mut oracle, text, style));
        let styles: Arc<[TextStyle]> = Arc::from([style.clone()]);
        let lines = lay_out(&Paragraph::new(&shaped, &styles, None), width, None);
        TextLayout::new(
            Arc::from(text),
            shaped,
            styles,
            width,
            Arc::new(lines),
            RunMemo::default(),
        )
    }

    fn style(decoration: TextDecoration) -> TextStyle {
        TextStyle::new(20.0, Color::BLACK).with_decoration(decoration)
    }

    fn both() -> TextDecoration {
        TextDecoration {
            underline: true,
            line_through: true,
            ..TextDecoration::default()
        }
    }

    fn of_kind(rects: &[DecorationRect], kind: DecorationKind) -> Vec<DecorationRect> {
        rects.iter().copied().filter(|r| r.kind == kind).collect()
    }

    #[test]
    fn underline_sits_below_the_baseline_and_line_through_above_it() {
        let layout = laid_out(NOTO_SANS, "Hello", &style(both()), None);
        let baseline = f64::from(layout.lines.lines[0].metrics.baseline);
        let ascent = f64::from(layout.lines.lines[0].metrics.ascent);
        let rects = layout.decoration_rects(Point::ORIGIN);
        assert_eq!(rects.len(), 2);
        let under = of_kind(&rects, DecorationKind::Underline)[0].rect;
        let through = of_kind(&rects, DecorationKind::LineThrough)[0].rect;
        assert!(under.y0 > baseline, "{under:?} vs baseline {baseline}");
        assert!(through.y1 <= baseline, "{through:?} vs baseline {baseline}");
        assert!(
            through.y0 >= baseline - ascent,
            "the line-through stays below the ascent: {through:?}"
        );
        assert!(under.y1 > under.y0 && through.y1 > through.y0);
    }

    #[test]
    fn thickness_and_offsets_come_from_the_face_metrics() {
        use read_fonts::{FontRef, TableProvider};
        let layout = laid_out(NOTO_SANS, "Hello", &style(both()), None);
        let font = FontRef::new(NOTO_SANS).unwrap();
        let upem = f64::from(font.head().unwrap().units_per_em());
        let post = font.post().unwrap();
        let os2 = font.os2().unwrap();
        let scale = 20.0 / upem;
        let baseline = f64::from(layout.lines.lines[0].metrics.baseline);
        let rects = layout.decoration_rects(Point::ORIGIN);
        let under = of_kind(&rects, DecorationKind::Underline)[0].rect;
        let through = of_kind(&rects, DecorationKind::LineThrough)[0].rect;
        let close = |a: f64, b: f64| (a - b).abs() < 0.02;
        assert!(close(
            under.height(),
            f64::from(post.underline_thickness().to_i16()) * scale
        ));
        assert!(close(
            baseline - under.y0,
            f64::from(post.underline_position().to_i16()) * scale
        ));
        assert!(close(
            through.height(),
            f64::from(os2.y_strikeout_size()) * scale
        ));
        assert!(close(
            baseline - through.y0,
            f64::from(os2.y_strikeout_position()) * scale
        ));
    }

    #[test]
    fn a_thickness_override_keeps_the_stroke_top() {
        let plain = laid_out(
            NOTO_SANS,
            "Hello",
            &style(TextDecoration::underline()),
            None,
        );
        let thick = laid_out(
            NOTO_SANS,
            "Hello",
            &style(TextDecoration::underline().with_thickness(3.0)),
            None,
        );
        let a = plain.decoration_rects(Point::ORIGIN)[0].rect;
        let b = thick.decoration_rects(Point::ORIGIN)[0].rect;
        assert_eq!(a.y0, b.y0);
        assert_eq!(b.height(), 3.0);
    }

    #[test]
    fn the_decoration_colour_overrides_the_text_colour() {
        let red = Color::from_rgb8(255, 0, 0);
        let own = laid_out(NOTO_SANS, "Hi", &style(TextDecoration::underline()), None);
        let tinted = laid_out(
            NOTO_SANS,
            "Hi",
            &style(TextDecoration::underline().with_color(red)),
            None,
        );
        assert_eq!(own.decoration_rects(Point::ORIGIN)[0].color, Color::BLACK);
        assert_eq!(tinted.decoration_rects(Point::ORIGIN)[0].color, red);
    }

    #[test]
    fn a_wrapped_paragraph_yields_one_rect_per_line() {
        let layout = laid_out(
            NOTO_SANS,
            "Hello world again and more",
            &style(TextDecoration::underline()),
            Some(70.0),
        );
        let lines = layout.line_count();
        assert!(lines > 1, "the text wraps");
        let rects = layout.decoration_rects(Point::ORIGIN);
        assert_eq!(rects.len(), lines);
        assert!(
            rects.windows(2).all(|w| w[0].rect.y0 < w[1].rect.y0),
            "one rect per line, top to bottom"
        );
    }

    #[test]
    fn trailing_spaces_are_not_underlined() {
        let bare = laid_out(
            NOTO_SANS,
            "Hello",
            &style(TextDecoration::underline()),
            None,
        );
        let padded = laid_out(
            NOTO_SANS,
            "Hello   ",
            &style(TextDecoration::underline()),
            None,
        );
        let a = bare.decoration_rects(Point::ORIGIN);
        let b = padded.decoration_rects(Point::ORIGIN);
        assert_eq!(a.len(), 1);
        assert_eq!(b.len(), 1);
        assert_eq!(a[0].rect, b[0].rect);
        // An interior space stays underlined.
        let inner = laid_out(NOTO_SANS, "a b", &style(TextDecoration::underline()), None);
        let split = laid_out(NOTO_SANS, "ab", &style(TextDecoration::underline()), None);
        assert!(
            inner.decoration_rects(Point::ORIGIN)[0].rect.width()
                > split.decoration_rects(Point::ORIGIN)[0].rect.width()
        );
    }

    #[test]
    fn an_rtl_run_is_underlined_over_its_visual_span() {
        let text = "\u{627}\u{644}\u{639}\u{631}\u{628}\u{64a}\u{629}";
        let layout = laid_out(NOTO_ARABIC, text, &style(TextDecoration::underline()), None);
        assert_eq!(layout.shaped.paragraph_level, 1);
        let rects = layout.decoration_rects(Point::new(10.0, 0.0));
        assert_eq!(rects.len(), 1);
        let rect = rects[0].rect;
        let runs = layout.to_scene_runs(Point::new(10.0, 0.0));
        let width = layout.size().width;
        assert!((rect.x0 - 10.0).abs() < 0.01, "{rect:?}");
        assert!((rect.width() - width).abs() < 0.01, "{rect:?} vs {width}");
        assert!(
            runs.iter()
                .flat_map(|r| r.glyphs.iter())
                .all(
                    |g| f64::from(g.x) + 10.0 >= rect.x0 - 0.01 && f64::from(g.x) + 10.0 <= rect.x1
                )
        );
    }

    #[test]
    fn a_justified_line_underlines_its_stretched_spaces() {
        let justified = style(TextDecoration::underline()).with_align(TextAlign::Justify);
        let layout = laid_out(
            NOTO_SANS,
            "aa bb cc dd ee ff gg hh",
            &justified,
            Some(120.0),
        );
        assert!(layout.line_count() > 1);
        let rects = layout.decoration_rects(Point::ORIGIN);
        let first = rects[0].rect;
        assert!(
            (first.width() - 120.0).abs() < 0.5,
            "the first line is stretched to the width: {first:?}"
        );
    }

    #[test]
    fn undecorated_text_yields_nothing() {
        let layout = laid_out(NOTO_SANS, "Hello", &style(TextDecoration::none()), None);
        assert!(layout.decoration_rects(Point::ORIGIN).is_empty());
    }

    #[test]
    fn the_origin_translates_every_rect() {
        let layout = laid_out(NOTO_SANS, "Hello", &style(both()), None);
        let at_zero = layout.decoration_rects(Point::ORIGIN);
        let moved = layout.decoration_rects(Point::new(5.0, 7.0));
        for (a, b) in at_zero.iter().zip(&moved) {
            assert_eq!(a.rect + kurbo::Vec2::new(5.0, 7.0), b.rect);
        }
    }
}
