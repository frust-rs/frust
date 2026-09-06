//! Colour (COLR) glyphs: the layer stream `glifo` paints one through, held
//! until the whole glyph is known to be expressible.
//!
//! A COLR glyph does not reach the sink as a glyph at all. `glifo` brackets it
//! in a clip of the glyph's own bounding area and then replays its paint graph
//! as a sequence of `push clip` / `set paint` / `fill rect` / `pop clip`
//! commands — one per colour layer, each filling that same bounding rectangle
//! through a clip that is the layer's shape. Recombining that stream into
//! engine draws is what this module does.
//!
//! # A layer is a filled shape, not a clipped rectangle
//!
//! The engine's clip stack lowers a *rectangle* — to a scissor where it can, to
//! a coverage mask otherwise — and has no third shape. A COLR layer's clip is a
//! glyph outline, which is neither; but the rectangle that layer fills is the
//! glyph's whole bounding area, so `fill(area) under clip(outline)` and
//! `fill(outline)` cover exactly the same pixels. The outline therefore becomes
//! the drawn shape rather than a clip that would have to be rasterized twice —
//! once as a mask and once as the coverage it masks — and a colour glyph costs
//! the engine one draw per layer and no clip at all.
//!
//! Rectangular clips (the outer area bracket, and a COLR `clip box`) stay
//! rectangles: they are intersected into one running rectangle in the glyph's
//! own space, which is exact because every one of them is axis-aligned there.
//! That rectangle is folded into the layer's shape when the shape is itself a
//! rectangle, and is otherwise pushed onto the compiler's clip stack only when
//! it would actually cut the layer's outline — which, for a well-formed face
//! whose layers sit inside the area they declare, it never does. A colour glyph
//! that changes the frame's clip counters is therefore the exception, not the
//! rule.
//!
//! # All of a glyph, or none of it
//!
//! Two shapes in the stream have no exact engine spelling: a layer clipped by
//! *two* outlines at once (their intersection is not a path this module can
//! name), and a layer inside an isolated blend bracket (the display list's own
//! layers are rectangular, and a non-default blend is exactly what the
//! scheduler refuses). Either one is discovered part-way through a glyph, by
//! which time earlier layers would already have been drawn — so nothing is
//! drawn until the glyph's bracket closes. The layers accumulate here first,
//! and a glyph that turned out to be inexpressible is dropped whole, which is
//! what keeps "a colour glyph the engine cannot paint goes missing rather than
//! landing half-painted" true of the pixels and not just of the counter.

use kurbo::{Affine, BezPath, PathEl, Point, Rect, Shape};
use peniko::Brush;

/// The geometry one colour layer paints.
#[derive(Debug)]
pub(crate) enum LayerShape {
    /// A rectangle in the glyph's own space: the layer's fill area already
    /// intersected with every rectangular clip around it.
    Rect(Rect),
    /// The layer's own outline, which is the clip `glifo` filled the area
    /// through (see this module's doc).
    Path(BezPath),
}

/// One colour layer, ready to become one engine draw.
#[derive(Debug)]
pub(crate) struct ColorLayer {
    /// What the layer covers.
    pub(crate) shape: LayerShape,
    /// The rectangular clip the shape is *not* already inside, in the glyph's
    /// own space, or `None` when the shape needs no clip at all.
    pub(crate) clip: Option<Rect>,
    /// The layer's own paint — a palette colour or a COLR gradient, never the
    /// run's brush.
    pub(crate) brush: Brush,
    /// The paint transform `glifo` set for this layer, relative to the glyph's
    /// draw transform.
    pub(crate) paint_transform: Affine,
}

/// One open bracket, and what closing it has to undo.
#[derive(Debug)]
enum Bracket {
    /// A rectangular clip, holding the running rectangle it replaced.
    Rect(Option<Rect>),
    /// An outline clip, held on [`ColorGlyph::outlines`].
    Outline,
    /// A bracket this module has no spelling for, which refuses the glyph.
    Refused,
}

/// The colour glyph currently being replayed into layers, if any.
///
/// One value reused for every glyph of a frame rather than allocated per
/// glyph: a colour run is a sequence of these, and the layer and bracket
/// buffers are exactly what should be reused across them.
#[derive(Debug, Default)]
pub(crate) struct ColorGlyph {
    /// Every bracket still open, in push order. Empty means no colour glyph is
    /// being replayed.
    brackets: Vec<Bracket>,
    /// The intersection of every rectangular clip currently open, in the
    /// glyph's own space; `None` when none is.
    rect: Option<Rect>,
    /// The outline clips currently open, innermost last.
    outlines: Vec<BezPath>,
    /// The layers collected so far.
    layers: Vec<ColorLayer>,
    /// Set once anything in this glyph turned out to be inexpressible.
    refused: bool,
    /// The transform the glyph's geometry is drawn under, captured when its
    /// first bracket opened.
    transform: Affine,
}

impl ColorGlyph {
    /// Whether a colour glyph is currently being replayed.
    pub(crate) fn is_open(&self) -> bool {
        !self.brackets.is_empty()
    }

    /// The transform this glyph's layers are drawn under.
    pub(crate) fn transform(&self) -> Affine {
        self.transform
    }

    /// Open a clip bracket around `path`, which is already in the glyph's own
    /// space.
    ///
    /// `transform` is the sink's current transform, which is what the glyph's
    /// geometry is drawn under; it is read only when this bracket is the
    /// glyph's first, since `glifo` sets it once per colour glyph and never
    /// again inside one.
    pub(crate) fn push_clip(&mut self, path: &BezPath, transform: Affine) {
        self.open(transform);

        match axis_aligned_rect(path) {
            Some(rect) => {
                let previous = self.rect;
                self.rect = Some(previous.map_or(rect, |open| open.intersect(rect)));
                self.brackets.push(Bracket::Rect(previous));
            }
            None => {
                self.outlines.push(path.clone());
                self.brackets.push(Bracket::Outline);
            }
        }
    }

    /// Open a bracket this module cannot honour — an isolated clip or blend
    /// layer — which refuses the glyph while keeping the brackets balanced.
    pub(crate) fn push_refused(&mut self, transform: Affine) {
        self.open(transform);
        self.refused = true;
        self.brackets.push(Bracket::Refused);
    }

    /// Close the innermost bracket, answering whether that was the glyph's
    /// last one and its layers are now ready to draw.
    ///
    /// A pop with nothing to pop answers `false`: `glifo` balances its own
    /// brackets, and an unbalanced one must not be able to flush a glyph that
    /// never started.
    pub(crate) fn pop(&mut self) -> bool {
        match self.brackets.pop() {
            Some(Bracket::Rect(previous)) => self.rect = previous,
            Some(Bracket::Outline) => {
                self.outlines.pop();
            }
            Some(Bracket::Refused) => {}
            None => return false,
        }

        self.brackets.is_empty()
    }

    /// Record the layer filling `area` with `brush` under `paint_transform`.
    ///
    /// `area` is the rectangle `glifo` fills every layer of a colour glyph
    /// with — the glyph's own bounding area, rounded out — and is intersected
    /// with the rectangular clips around it here rather than clipped later.
    pub(crate) fn fill(&mut self, area: Rect, brush: Brush, paint_transform: Affine) {
        if self.refused {
            return;
        }

        let bound = self.rect.map_or(area, |rect| rect.intersect(area));
        if bound.width() <= 0.0 || bound.height() <= 0.0 {
            // Clipped away entirely: no layer, and no reason to refuse the
            // glyph either.
            return;
        }

        let shape = match self.outlines.as_slice() {
            [] => LayerShape::Rect(bound),
            [outline] => LayerShape::Path(outline.clone()),
            // Two outline clips at once: their intersection is not a shape
            // this module can name, so the glyph goes missing whole.
            _ => {
                self.refused = true;
                return;
            }
        };

        let clip = match &shape {
            // The rectangle is the intersection already.
            LayerShape::Rect(_) => None,
            LayerShape::Path(path) => (!contains(bound, path.bounding_box())).then_some(bound),
        };

        self.layers.push(ColorLayer {
            shape,
            clip,
            brush,
            paint_transform,
        });
    }

    /// Refuse the glyph being replayed, without closing anything.
    pub(crate) fn refuse(&mut self) {
        self.refused = true;
    }

    /// Take the finished glyph's layers, or `None` when it was refused.
    ///
    /// Leaves this value empty either way, ready for the next glyph.
    pub(crate) fn take(&mut self) -> Option<Vec<ColorLayer>> {
        let refused = self.refused;
        self.refused = false;
        self.rect = None;
        self.outlines.clear();
        self.brackets.clear();

        let layers = std::mem::take(&mut self.layers);
        (!refused).then_some(layers)
    }

    /// Drop everything held for a glyph that will never close its brackets.
    pub(crate) fn reset(&mut self) {
        self.brackets.clear();
        self.outlines.clear();
        self.layers.clear();
        self.rect = None;
        self.refused = false;
    }

    /// Note the transform a glyph is drawn under, when this is its first
    /// bracket.
    fn open(&mut self, transform: Affine) {
        if self.brackets.is_empty() {
            self.transform = transform;
        }
    }
}

/// Whether `outer` covers all of `inner`.
fn contains(outer: Rect, inner: Rect) -> bool {
    outer.x0 <= inner.x0 && outer.y0 <= inner.y0 && outer.x1 >= inner.x1 && outer.y1 >= inner.y1
}

/// `path` as the axis-aligned rectangle it draws, or `None` when it draws
/// anything else.
///
/// Deliberately structural rather than a bounding box: a bounding box exists
/// for every path, and taking one for a glyph outline would silently widen a
/// clip into the rectangle around it. Only the four-corner, axis-aligned
/// traversal `kurbo::Rect`'s own path elements produce — which is what both a
/// COLR clip box and the bracket around a colour glyph arrive as — is
/// recognised.
fn axis_aligned_rect(path: &BezPath) -> Option<Rect> {
    let mut points: Vec<Point> = Vec::with_capacity(4);
    let mut closed = false;

    for element in path.elements() {
        if closed {
            // Anything after the close is a second subpath, which is not a
            // rectangle however the first one looked.
            return None;
        }
        match element {
            PathEl::MoveTo(point) => {
                if !points.is_empty() {
                    return None;
                }
                points.push(*point);
            }
            PathEl::LineTo(point) => {
                if points.is_empty() || points.len() > 4 {
                    return None;
                }
                points.push(*point);
            }
            PathEl::ClosePath => closed = true,
            PathEl::QuadTo(..) | PathEl::CurveTo(..) => return None,
        }
    }

    // A closed traversal may or may not restate its first corner.
    if points.len() == 5 && points.get(4) == points.first() {
        points.truncate(4);
    }
    let corners: [Point; 4] = points.try_into().ok()?;

    // Every edge — including the closing one — axis-aligned is what rules out
    // the two self-intersecting orderings of the same four corners, which fill
    // as a bow tie rather than as the rectangle they bound.
    for index in 0..corners.len() {
        let from = *corners.get(index)?;
        let to = *corners.get((index + 1) % corners.len())?;
        if from.x != to.x && from.y != to.y {
            return None;
        }
    }

    let (x0, x1) = min_max(corners.map(|corner| corner.x))?;
    let (y0, y1) = min_max(corners.map(|corner| corner.y))?;

    Some(Rect::new(x0, y0, x1, y1))
}

/// The smallest and largest of `values`, or `None` when any is not finite.
fn min_max(values: [f64; 4]) -> Option<(f64, f64)> {
    if !values.iter().all(|value| value.is_finite()) {
        return None;
    }

    let mut low = *values.first()?;
    let mut high = low;
    for value in values {
        low = low.min(value);
        high = high.max(value);
    }

    Some((low, high))
}

#[cfg(test)]
mod tests {
    use super::*;

    use peniko::color::palette::css::{BLUE, RED};

    /// The transform a colour glyph's layers are drawn under in these cases —
    /// deliberately not the identity, so a value read from the wrong place is
    /// visible.
    fn transform() -> Affine {
        Affine::translate((3.0, 5.0))
    }

    /// A four-corner axis-aligned traversal, exactly as `kurbo::Rect`'s own
    /// path elements produce it.
    fn rect_path(rect: Rect) -> BezPath {
        rect.to_path(0.1)
    }

    /// A triangle: three corners, one of them reached diagonally.
    fn triangle() -> BezPath {
        let mut path = BezPath::new();
        path.move_to((0.0, 0.0));
        path.line_to((10.0, 0.0));
        path.line_to((5.0, 10.0));
        path.close_path();
        path
    }

    /// The same four corners as a rectangle, traversed so the shape crosses
    /// itself.
    fn bow_tie() -> BezPath {
        let mut path = BezPath::new();
        path.move_to((0.0, 0.0));
        path.line_to((10.0, 10.0));
        path.line_to((10.0, 0.0));
        path.line_to((0.0, 10.0));
        path.close_path();
        path
    }

    #[test]
    fn a_rectangles_own_path_is_recognised_as_that_rectangle() {
        let rect = Rect::new(1.0, 2.0, 30.0, 40.0);
        assert_eq!(axis_aligned_rect(&rect_path(rect)), Some(rect));
    }

    #[test]
    fn anything_that_is_not_an_axis_aligned_rectangle_is_not_recognised() {
        let mut curved = BezPath::new();
        curved.move_to((0.0, 0.0));
        curved.curve_to((1.0, 0.0), (2.0, 1.0), (2.0, 2.0));
        curved.close_path();

        for (label, path) in [
            ("a triangle", triangle()),
            ("a self-crossing quadrilateral", bow_tie()),
            ("a path with a curve in it", curved),
            ("an empty path", BezPath::new()),
        ] {
            assert_eq!(
                axis_aligned_rect(&path),
                None,
                "{label} is not a rectangle, and must not be widened into one"
            );
        }
    }

    #[test]
    fn a_layer_inside_rectangles_alone_is_their_intersection() {
        let mut glyph = ColorGlyph::default();
        glyph.push_clip(&rect_path(Rect::new(0.0, 0.0, 20.0, 20.0)), transform());
        glyph.push_clip(&rect_path(Rect::new(10.0, 0.0, 40.0, 12.0)), transform());
        glyph.fill(
            Rect::new(0.0, 0.0, 40.0, 40.0),
            Brush::Solid(RED),
            Affine::IDENTITY,
        );
        assert!(!glyph.pop(), "the outer bracket is still open");
        assert!(glyph.pop());

        let layers = glyph.take().expect("nothing refused the glyph");
        let [layer] = layers.as_slice() else {
            panic!("one fill is one layer, got {}", layers.len());
        };
        assert!(
            layer.clip.is_none(),
            "the rectangle became the shape itself"
        );
        match layer.shape {
            LayerShape::Rect(rect) => assert_eq!(rect, Rect::new(10.0, 0.0, 20.0, 12.0)),
            LayerShape::Path(_) => panic!("no outline clip was open"),
        }
    }

    #[test]
    fn a_layer_clipped_away_entirely_records_nothing_and_refuses_nothing() {
        let mut glyph = ColorGlyph::default();
        glyph.push_clip(&rect_path(Rect::new(0.0, 0.0, 10.0, 10.0)), transform());
        glyph.fill(
            Rect::new(20.0, 20.0, 30.0, 30.0),
            Brush::Solid(RED),
            Affine::IDENTITY,
        );
        assert!(glyph.pop());

        assert_eq!(
            glyph.take().map(|layers| layers.len()),
            Some(0),
            "an empty intersection is a layer that paints nothing, not a \
             glyph the engine cannot express"
        );
    }

    #[test]
    fn an_outline_clip_becomes_the_layers_shape() {
        let mut glyph = ColorGlyph::default();
        glyph.push_clip(&rect_path(Rect::new(0.0, 0.0, 40.0, 40.0)), transform());
        glyph.push_clip(&triangle(), transform());
        glyph.fill(
            Rect::new(0.0, 0.0, 40.0, 40.0),
            Brush::Solid(BLUE),
            Affine::IDENTITY,
        );
        assert!(!glyph.pop());
        assert!(glyph.pop());

        let layers = glyph.take().expect("nothing refused the glyph");
        let [layer] = layers.as_slice() else {
            panic!("one fill is one layer, got {}", layers.len());
        };
        assert!(
            layer.clip.is_none(),
            "the rectangle around it contains it, so it costs no clip"
        );
        assert!(matches!(layer.shape, LayerShape::Path(_)));
        assert_eq!(layer.brush, Brush::Solid(BLUE), "the layer's own paint");
    }

    #[test]
    fn an_outline_that_escapes_its_rectangle_carries_it_as_a_clip() {
        let mut glyph = ColorGlyph::default();
        glyph.push_clip(&rect_path(Rect::new(0.0, 0.0, 4.0, 4.0)), transform());
        glyph.push_clip(&triangle(), transform());
        glyph.fill(
            Rect::new(0.0, 0.0, 40.0, 40.0),
            Brush::Solid(RED),
            Affine::IDENTITY,
        );
        glyph.pop();
        assert!(glyph.pop());

        let layers = glyph.take().expect("nothing refused the glyph");
        let [layer] = layers.as_slice() else {
            panic!("one fill is one layer, got {}", layers.len());
        };
        assert_eq!(layer.clip, Some(Rect::new(0.0, 0.0, 4.0, 4.0)));
    }

    #[test]
    fn two_outline_clips_at_once_refuse_the_whole_glyph() {
        let mut glyph = ColorGlyph::default();
        glyph.push_clip(&triangle(), transform());
        // The first layer is expressible and would already have been drawn if
        // layers were emitted as they arrived.
        glyph.fill(
            Rect::new(0.0, 0.0, 40.0, 40.0),
            Brush::Solid(RED),
            Affine::IDENTITY,
        );
        glyph.push_clip(&triangle(), transform());
        glyph.fill(
            Rect::new(0.0, 0.0, 40.0, 40.0),
            Brush::Solid(BLUE),
            Affine::IDENTITY,
        );
        glyph.pop();
        assert!(glyph.pop());

        assert!(
            glyph.take().is_none(),
            "an inexpressible layer takes the whole glyph with it, including \
             the layers already collected"
        );
    }

    #[test]
    fn an_isolated_bracket_refuses_the_glyph_and_still_balances() {
        let mut glyph = ColorGlyph::default();
        glyph.push_clip(&rect_path(Rect::new(0.0, 0.0, 40.0, 40.0)), transform());
        glyph.push_refused(transform());
        glyph.fill(
            Rect::new(0.0, 0.0, 40.0, 40.0),
            Brush::Solid(RED),
            Affine::IDENTITY,
        );

        assert!(!glyph.pop(), "the blend bracket closes first");
        assert!(glyph.pop(), "and the glyph's own bracket after it");
        assert!(glyph.take().is_none());
        assert!(
            !glyph.is_open(),
            "taking a refused glyph leaves nothing behind for the next one"
        );
    }

    #[test]
    fn an_unmatched_pop_closes_no_glyph() {
        let mut glyph = ColorGlyph::default();
        assert!(!glyph.pop(), "there was nothing open to close");
        assert!(!glyph.is_open());
    }

    #[test]
    fn the_transform_is_the_one_the_first_bracket_opened_under() {
        let mut glyph = ColorGlyph::default();
        glyph.push_clip(&rect_path(Rect::new(0.0, 0.0, 40.0, 40.0)), transform());
        // `glifo` sets the transform once per colour glyph, before the
        // bracket; a later bracket must not be able to move the glyph.
        glyph.push_clip(&triangle(), Affine::scale(9.0));

        assert_eq!(glyph.transform(), transform());
    }
}
