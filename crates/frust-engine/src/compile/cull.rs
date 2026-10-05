//! Device-space pre-pass that bounds the flattening cost of any finite path
//! under any finite transform.
//!
//! `vello_common`'s flattener culls a curve whose control points all lie on
//! one side of its cull rectangle — dropped when right of, above or below it,
//! replaced by its chord when left of it — but subdivides a curve that
//! straddles the rectangle whole, in a number of segments that grows with the
//! square root of the curve's device size. A 1e20-pixel arc crossing the
//! viewport would flatten to around 1e10 points.
//!
//! [`ViewportSplit`] runs before that flattener. It applies the transform to
//! every point and halves a curve whose control hull both reaches the
//! margin-expanded viewport and is larger than a few viewports
//! ([`SPLIT_EXTENT_VIEWPORTS`]), by exact de Casteljau subdivision on a
//! fixed-capacity stack. A piece is emitted once it lies wholly on one side of
//! the expanded viewport (the flattener then drops it or emits its chord),
//! once it is small enough to flatten at on-screen cost, or once it reaches
//! [`MAX_SPLIT_DEPTH`]. Pieces are emitted in curve order, so the output
//! traces the same curve in the same direction, and the caller flattens it
//! with an identity transform.
//!
//! **Winding is preserved.** The split itself is exact, and the flattener's
//! later chord-or-drop of an off-viewport piece changes coverage only inside
//! that piece's control hull: the region between a curve and its chord lies
//! inside the hull, and the hull lies outside the viewport.
//!
//! **Output bound.** A piece is split only while its hull reaches the
//! viewport, and a curve of degree at most 3 crosses each of a rectangle's
//! four edge lines at most 3 times, so it meets the viewport boundary at most
//! 12 times. Each crossing keeps a bounded number of pieces live per halving
//! level, so a curve yields O(12 × [`MAX_SPLIT_DEPTH`]) pieces however large
//! it is. Typical UI geometry never splits and passes through untouched.

use kurbo::{Affine, PathEl, Point, Rect};

/// Device pixels added to every side of the viewport before culling
/// decisions. It must cover the flattener's own cull rectangle, which rounds
/// to whole pixels and aligns its top edge down to a strip row: a piece judged
/// outside the expanded viewport must also be outside that rectangle, or the
/// flattener would subdivide it whole.
pub const VIEWPORT_MARGIN: f64 = 16.0;

/// Largest control-hull extent, in multiples of the expanded viewport's
/// larger side, that a viewport-straddling curve may keep unsplit. The
/// flattener's cost for a piece this size matches that of geometry drawn on
/// screen, and geometry spanning up to a few screens — scrolled content,
/// large rounded panels, glyph outlines at any legible size — stays below it
/// and passes through untouched.
pub const SPLIT_EXTENT_VIEWPORTS: f64 = 4.0;

/// Maximum number of halvings applied to one curve. At this depth a piece
/// spans about 2⁻⁴⁸ of the original curve's parameter range, so even a 1e20
/// pixel curve leaves pieces nearly straight and cheap to flatten.
pub const MAX_SPLIT_DEPTH: u8 = 48;

/// Pending pieces held at once: one deferred second half per halving level,
/// plus the piece being halved at the deepest level.
const STACK_CAPACITY: usize = MAX_SPLIT_DEPTH as usize + 1;

/// A quadratic or cubic Bézier segment awaiting a split-or-emit decision, its
/// start point explicit.
#[derive(Clone, Copy, Debug)]
struct Piece {
    /// Control points; only the first three are meaningful for a quadratic.
    pts: [Point; 4],
    cubic: bool,
    depth: u8,
}

impl Piece {
    const EMPTY: Self = Self {
        pts: [Point::ZERO; 4],
        cubic: false,
        depth: 0,
    };

    fn quad(p0: Point, p1: Point, p2: Point) -> Self {
        Self {
            pts: [p0, p1, p2, p2],
            cubic: false,
            depth: 0,
        }
    }

    fn cubic(p0: Point, p1: Point, p2: Point, p3: Point) -> Self {
        Self {
            pts: [p0, p1, p2, p3],
            cubic: true,
            depth: 0,
        }
    }

    fn points(&self) -> &[Point] {
        if self.cubic {
            &self.pts
        } else {
            &self.pts[..3]
        }
    }

    /// The larger side of the control-point bounding box.
    fn extent(&self) -> f64 {
        let first = self.pts[0];
        let (mut x0, mut y0, mut x1, mut y1) = (first.x, first.y, first.x, first.y);
        for p in self.points() {
            x0 = x0.min(p.x);
            y0 = y0.min(p.y);
            x1 = x1.max(p.x);
            y1 = y1.max(p.y);
        }
        (x1 - x0).max(y1 - y0)
    }

    /// Whether every control point lies strictly on one side of `bounds` —
    /// the flattener's own test for dropping a curve or emitting its chord.
    fn is_outside(&self, bounds: Rect) -> bool {
        let pts = self.points();
        pts.iter().all(|p| p.x < bounds.x0)
            || pts.iter().all(|p| p.x > bounds.x1)
            || pts.iter().all(|p| p.y < bounds.y0)
            || pts.iter().all(|p| p.y > bounds.y1)
    }

    /// Exact de Casteljau halving at `t = 0.5`. Both halves share the one
    /// computed midpoint, so the first half ends exactly where the second
    /// starts.
    fn halve(&self) -> (Self, Self) {
        let depth = self.depth.saturating_add(1);
        let [p0, p1, p2, p3] = self.pts;
        let p01 = p0.midpoint(p1);
        let p12 = p1.midpoint(p2);
        if self.cubic {
            let p23 = p2.midpoint(p3);
            let p012 = p01.midpoint(p12);
            let p123 = p12.midpoint(p23);
            let mid = p012.midpoint(p123);
            (
                Self {
                    pts: [p0, p01, p012, mid],
                    cubic: true,
                    depth,
                },
                Self {
                    pts: [mid, p123, p23, p3],
                    cubic: true,
                    depth,
                },
            )
        } else {
            let mid = p01.midpoint(p12);
            (
                Self {
                    pts: [p0, p01, mid, mid],
                    cubic: false,
                    depth,
                },
                Self {
                    pts: [mid, p12, p2, p2],
                    cubic: false,
                    depth,
                },
            )
        }
    }

    fn to_el(self) -> PathEl {
        let [_, p1, p2, p3] = self.pts;
        if self.cubic {
            PathEl::CurveTo(p1, p2, p3)
        } else {
            PathEl::QuadTo(p1, p2)
        }
    }
}

/// Allocation-free adapter yielding a path's elements in device space, with
/// every huge viewport-straddling curve split into pieces the flattener can
/// process at bounded cost.
///
/// Lines, moves and closes are transformed and passed through. A curve is
/// transformed and passed through unchanged unless its control hull both
/// reaches the expanded viewport and exceeds the split extent; such a curve
/// is replaced by consecutive sub-curves tracing the same points in the same
/// direction. The output is already in device space: flatten it with
/// [`Affine::IDENTITY`].
#[derive(Debug)]
pub struct ViewportSplit<I> {
    inner: I,
    affine: Affine,
    bounds: Rect,
    split_extent: f64,
    subpath_start: Point,
    current: Point,
    stack: [Piece; STACK_CAPACITY],
    len: usize,
}

impl<I: Iterator<Item = PathEl>> ViewportSplit<I> {
    /// Wraps a user-space `path` drawn under `affine` and culled against the
    /// device-space `viewport` — the same rectangle the flattener culls
    /// against, which this adapter expands by [`VIEWPORT_MARGIN`].
    pub fn new(
        path: impl IntoIterator<Item = PathEl, IntoIter = I>,
        affine: Affine,
        viewport: Rect,
    ) -> Self {
        let bounds = viewport.abs().inflate(VIEWPORT_MARGIN, VIEWPORT_MARGIN);
        Self {
            inner: path.into_iter(),
            affine,
            bounds,
            split_extent: SPLIT_EXTENT_VIEWPORTS * bounds.width().max(bounds.height()),
            subpath_start: Point::ZERO,
            current: Point::ZERO,
            stack: [Piece::EMPTY; STACK_CAPACITY],
            len: 0,
        }
    }

    /// Whether `piece` must be halved before it is emitted. A full stack
    /// emits the piece whole rather than overflow; the depth cap keeps the
    /// stack from ever filling.
    fn needs_split(&self, piece: &Piece) -> bool {
        piece.depth < MAX_SPLIT_DEPTH
            && self.len + 2 <= STACK_CAPACITY
            && piece.extent() > self.split_extent
            && !piece.is_outside(self.bounds)
    }

    fn push(&mut self, piece: Piece) {
        if let Some(slot) = self.stack.get_mut(self.len) {
            *slot = piece;
            self.len += 1;
        }
    }

    fn pop(&mut self) -> Option<Piece> {
        self.len = self.len.checked_sub(1)?;
        self.stack.get(self.len).copied()
    }
}

impl<I: Iterator<Item = PathEl>> Iterator for ViewportSplit<I> {
    type Item = PathEl;

    fn next(&mut self) -> Option<PathEl> {
        loop {
            if let Some(piece) = self.pop() {
                if self.needs_split(&piece) {
                    let (first, second) = piece.halve();
                    self.push(second);
                    self.push(first);
                    continue;
                }
                return Some(piece.to_el());
            }

            let el = self.affine * self.inner.next()?;
            match el {
                PathEl::MoveTo(p) => {
                    self.subpath_start = p;
                    self.current = p;
                }
                PathEl::LineTo(p) => self.current = p,
                PathEl::QuadTo(p1, p2) => {
                    let piece = Piece::quad(self.current, p1, p2);
                    self.current = p2;
                    if self.needs_split(&piece) {
                        self.push(piece);
                        continue;
                    }
                }
                PathEl::CurveTo(p1, p2, p3) => {
                    let piece = Piece::cubic(self.current, p1, p2, p3);
                    self.current = p3;
                    if self.needs_split(&piece) {
                        self.push(piece);
                        continue;
                    }
                }
                PathEl::ClosePath => self.current = self.subpath_start,
            }
            return Some(el);
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.len + self.inner.size_hint().0, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::{
        BezPath, Circle, CubicBez, ParamCurve, ParamCurveNearest, PathSeg, QuadBez, Shape,
    };

    const VIEWPORT: Rect = Rect::new(0.0, 0.0, 800.0, 600.0);

    fn split(path: &BezPath, affine: Affine) -> Vec<PathEl> {
        ViewportSplit::new(path.iter(), affine, VIEWPORT).collect()
    }

    fn point_bits(p: Point) -> (u64, u64) {
        (p.x.to_bits(), p.y.to_bits())
    }

    fn el_bits(el: PathEl) -> Vec<(u64, u64)> {
        match el {
            PathEl::MoveTo(p) | PathEl::LineTo(p) => vec![point_bits(p)],
            PathEl::QuadTo(p1, p2) => vec![point_bits(p1), point_bits(p2)],
            PathEl::CurveTo(p1, p2, p3) => vec![point_bits(p1), point_bits(p2), point_bits(p3)],
            PathEl::ClosePath => Vec::new(),
        }
    }

    fn same_kind(a: PathEl, b: PathEl) -> bool {
        std::mem::discriminant(&a) == std::mem::discriminant(&b)
    }

    /// A quadratic through the viewport centre at `t = 0.5` whose endpoints
    /// sit `size` pixels away on either side.
    fn huge_quad(size: f64) -> BezPath {
        let mut path = BezPath::new();
        path.move_to((-size, size));
        path.quad_to((800.0, 600.0 - size), (size, size));
        path.close_path();
        path
    }

    /// An S-shaped cubic through the viewport centre at `t = 0.5` whose
    /// control points sit up to `3 * size` pixels away.
    fn huge_cubic(size: f64) -> BezPath {
        let mut path = BezPath::new();
        path.move_to((-size, 0.0));
        path.curve_to(
            (size, 3.0 * size),
            (3200.0 / 3.0 - size, 600.0 - 3.0 * size),
            (size, 600.0),
        );
        path.line_to((size, 2.0 * size));
        path.close_path();
        path
    }

    /// A unit circle under a transform that scales it to radius `size` and
    /// places the top of its approximating path at the viewport centre.
    fn huge_circle(size: f64) -> (BezPath, Affine) {
        let path = Circle::new((0.0, 0.0), 1.0).to_path(0.1);
        let shape = Affine::scale(size) * Affine::rotate(0.3);
        let bounds = (shape * path.clone()).bounding_box();
        let affine = Affine::translate((400.0 - bounds.center().x, 300.0 - bounds.y0)) * shape;
        (path, affine)
    }

    /// A unit circle stretched into an ellipse `size` wide, its sharp
    /// leftmost end placed at the viewport centre and turned to cross the
    /// viewport diagonally.
    fn huge_ellipse(size: f64) -> (BezPath, Affine) {
        let path = Circle::new((0.0, 0.0), 1.0).to_path(0.1);
        let shape = Affine::scale_non_uniform(size, size * 0.01);
        let bounds = (shape * path.clone()).bounding_box();
        let affine = Affine::translate((400.0, 300.0))
            * Affine::rotate(0.7)
            * Affine::translate((-bounds.x0, -bounds.center().y))
            * shape;
        (path, affine)
    }

    fn huge_cases(size: f64) -> Vec<(BezPath, Affine)> {
        vec![
            (huge_quad(size), Affine::IDENTITY),
            (huge_cubic(size), Affine::IDENTITY),
            huge_circle(size),
            huge_ellipse(size),
        ]
    }

    #[test]
    fn small_curves_pass_through_bit_identical() {
        let mut path = BezPath::new();
        path.move_to((10.0, 20.0));
        path.line_to((300.0, 40.5));
        path.quad_to((420.25, -80.0), (780.0, 590.0));
        path.curve_to((-200.0, 700.0), (900.0, -100.0), (123.456, 654.321));
        path.close_path();
        path.move_to((-50.0, -50.0));
        path.curve_to((1200.0, 30.0), (1500.0, 1200.0), (400.0, 300.0));
        path.quad_to((0.0, 0.0), (1.0, 1.0));
        path.close_path();
        // Huge, but wholly right of the viewport: the flattener drops it.
        path.move_to((5000.0, 0.0));
        path.curve_to((1e20, -1e20), (1e20, 1e20), (5000.0, 600.0));
        path.close_path();
        // Huge, but wholly left of the viewport: the flattener takes its chord.
        path.move_to((-5000.0, 0.0));
        path.quad_to((-1e20, 300.0), (-5000.0, 600.0));

        let transforms = [
            Affine::IDENTITY,
            Affine::translate((13.5, -7.25)) * Affine::scale(1.75),
            Affine::scale_non_uniform(0.5, 2.0) * Affine::translate((-3.0, 11.0)),
        ];
        for affine in transforms {
            let expected: Vec<PathEl> = path.iter().map(|el| affine * el).collect();
            let actual = split(&path, affine);
            assert_eq!(actual.len(), expected.len(), "under {affine:?}");
            for (a, e) in actual.iter().zip(&expected) {
                assert!(same_kind(*a, *e), "{a:?} vs {e:?} under {affine:?}");
                assert_eq!(el_bits(*a), el_bits(*e), "{a:?} vs {e:?} under {affine:?}");
            }
        }
    }

    #[test]
    fn huge_straddling_curves_split_into_a_bounded_number_of_pieces() {
        for size in [1e9, 1e15, 1e20] {
            for (path, affine) in huge_cases(size) {
                let input_len = path.elements().len();
                let out = split(&path, affine);
                assert!(
                    out.len() > input_len,
                    "a {size:e}-pixel straddling path must split, got {} elements",
                    out.len()
                );
                assert!(
                    out.len() < 2000,
                    "a {size:e}-pixel path yields {} elements",
                    out.len()
                );
                let cost = straddling_flatten_cost(&out);
                assert!(
                    cost < 2000,
                    "a {size:e}-pixel path flattens to {cost} lines"
                );
            }
        }
    }

    #[test]
    fn every_piece_is_outside_small_or_depth_capped() {
        // 1e9 pixels needs about 19 halvings, well under the depth cap, so
        // every emitted piece meets one of the two non-cap conditions.
        let bounds = VIEWPORT.inflate(VIEWPORT_MARGIN, VIEWPORT_MARGIN);
        let split_extent = SPLIT_EXTENT_VIEWPORTS * bounds.width().max(bounds.height());
        for (path, affine) in huge_cases(1e9) {
            let out: BezPath = BezPath::from_vec(split(&path, affine));
            for seg in out.segments() {
                let piece = match seg {
                    PathSeg::Line(_) => continue,
                    PathSeg::Quad(q) => Piece::quad(q.p0, q.p1, q.p2),
                    PathSeg::Cubic(c) => Piece::cubic(c.p0, c.p1, c.p2, c.p3),
                };
                assert!(
                    piece.is_outside(bounds) || piece.extent() <= split_extent,
                    "piece {piece:?} straddles the viewport at extent {}",
                    piece.extent()
                );
            }
        }
    }

    /// Pairs each element of a device-space path with the curve piece it
    /// draws, its start point made explicit.
    fn with_pieces(path: &[PathEl]) -> Vec<(PathEl, Option<Piece>)> {
        let mut current = Point::ZERO;
        let mut start = Point::ZERO;
        path.iter()
            .map(|&el| {
                let piece = match el {
                    PathEl::QuadTo(p1, p2) => Some(Piece::quad(current, p1, p2)),
                    PathEl::CurveTo(p1, p2, p3) => Some(Piece::cubic(current, p1, p2, p3)),
                    _ => None,
                };
                current = match el {
                    PathEl::MoveTo(p) => {
                        start = p;
                        p
                    }
                    PathEl::LineTo(p) | PathEl::QuadTo(_, p) | PathEl::CurveTo(_, _, p) => p,
                    PathEl::ClosePath => start,
                };
                (el, piece)
            })
            .collect()
    }

    /// Replaces every piece lying wholly on one side of the expanded viewport
    /// with its chord, as the flattener does for a piece left of it.
    fn chorded(path: &[PathEl]) -> BezPath {
        let bounds = VIEWPORT.inflate(VIEWPORT_MARGIN, VIEWPORT_MARGIN);
        let mut out = BezPath::new();
        for (el, piece) in with_pieces(path) {
            match piece {
                Some(piece) if piece.is_outside(bounds) => out.line_to(piece.pts[3]),
                _ => out.push(el),
            }
        }
        out
    }

    /// Line segments a flattener at the strip pipeline's 0.25-pixel tolerance
    /// produces for the pieces it cannot cull: those reaching the expanded
    /// viewport.
    fn straddling_flatten_cost(path: &[PathEl]) -> usize {
        let bounds = VIEWPORT.inflate(VIEWPORT_MARGIN, VIEWPORT_MARGIN);
        let mut lines = 0;
        for (el, piece) in with_pieces(path) {
            match piece {
                Some(piece) if !piece.is_outside(bounds) => {
                    let els = [PathEl::MoveTo(piece.pts[0]), el];
                    kurbo::flatten(els, 0.25, |line| {
                        lines += usize::from(matches!(line, PathEl::LineTo(_)));
                    });
                }
                _ => lines += 1,
            }
        }
        lines
    }

    #[test]
    fn winding_inside_the_viewport_is_preserved() {
        for size in [1e5, 1e7, 1e9] {
            for (case, (path, affine)) in huge_cases(size).into_iter().enumerate() {
                let original = affine * path.clone();
                let out = split(&path, affine);
                let pieces = BezPath::from_vec(out.clone());
                let chords = chorded(&out);
                let mut samples = 0;
                let mut nonzero = 0;
                for ix in 0..=40 {
                    for iy in 0..=30 {
                        let p = Point::new(ix as f64 * 20.0 + 0.37, iy as f64 * 20.0 + 0.61);
                        let expected = original.winding(p);
                        assert_eq!(pieces.winding(p), expected, "split, {size:e}, at {p:?}");
                        assert_eq!(chords.winding(p), expected, "chorded, {size:e}, at {p:?}");
                        samples += 1;
                        nonzero += i32::from(expected != 0);
                    }
                }
                assert!(
                    nonzero > 0 && nonzero < samples,
                    "case {case} at {size:e}: the curve must divide the sampled viewport \
                     ({nonzero}/{samples} inside)"
                );
            }
        }
    }

    /// Checks that `out` traces `curve` from start to end: each piece matches
    /// the original curve over the parameter range from the previous piece's
    /// end to its own.
    fn assert_traces(curve: PathSeg, out: &[PathEl]) {
        let (first, rest) = out.split_first().expect("non-empty output");
        assert_eq!(
            point_bits(first.end_point().expect("move")),
            point_bits(curve.start())
        );
        let tolerance = 1e-9 * curve.bounding_box().size().max_side();
        let close = |a: Point, b: Point| (a - b).hypot() <= tolerance;
        let mut t0 = 0.0;
        let mut start = curve.start();
        for &el in rest {
            let (piece, end) = match el {
                PathEl::QuadTo(p1, p2) => (PathSeg::Quad(QuadBez::new(start, p1, p2)), p2),
                PathEl::CurveTo(p1, p2, p3) => {
                    (PathSeg::Cubic(CubicBez::new(start, p1, p2, p3)), p3)
                }
                other => panic!("unexpected {other:?}"),
            };
            let t1 = curve.nearest(end, 1e-12).t;
            assert!(
                t1 > t0,
                "piece ending at {end:?} runs backwards ({t0} -> {t1})"
            );
            let reference = curve.subsegment(t0..t1);
            for t in [0.25, 0.5, 0.75] {
                assert!(
                    close(piece.eval(t), reference.eval(t)),
                    "piece {piece:?} departs from the curve over {t0}..{t1}"
                );
            }
            t0 = t1;
            start = end;
        }
        assert_eq!(point_bits(start), point_bits(curve.end()));
    }

    #[test]
    fn pieces_run_from_start_to_end_in_curve_order() {
        let size = 1e6;
        let quad = QuadBez::new((-size, size), (800.0, 600.0 - size), (size, size));
        let cubic = CubicBez::new(
            (-size, 0.0),
            (size, 3.0 * size),
            (3200.0 / 3.0 - size, 600.0 - 3.0 * size),
            (size, 600.0),
        );
        let reversed = CubicBez::new(cubic.p3, cubic.p2, cubic.p1, cubic.p0);
        for curve in [
            PathSeg::Quad(quad),
            PathSeg::Cubic(cubic),
            PathSeg::Cubic(reversed),
        ] {
            let path = BezPath::from_path_segments(std::iter::once(curve));
            let out = split(&path, Affine::IDENTITY);
            assert!(out.len() > 2, "{curve:?} must split");
            assert_traces(curve, &out);
        }
    }
}
