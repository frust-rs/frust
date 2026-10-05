//! Device-space bounds on what a draw costs to flatten: every curve-capable
//! fill, clip mask, glyph outline and stroke reaches the strip generator
//! through this module, so compile cost and allocation stay bounded for any
//! finite path under any finite transform.
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
//! fixed-capacity stack. A piece is emitted as a curve once it lies wholly on
//! one side of the expanded viewport (the flattener then drops it or emits its
//! chord) or once it is small enough to flatten at on-screen cost. A piece
//! that reaches [`MAX_SPLIT_DEPTH`] while it would still need splitting is
//! emitted as its chord. Pieces are emitted in curve order, so the output
//! traces the same curve in the same direction, and the caller flattens it
//! with an identity transform.
//!
//! **Winding is preserved.** The split itself is exact, and the flattener's
//! later chord-or-drop of an off-viewport piece changes coverage only inside
//! that piece's control hull: the region between a curve and its chord lies
//! inside the hull, and the hull lies outside the viewport.
//!
//! **Accuracy at the depth cap.** Halving at `t = 1/2` divides the largest
//! second difference of a control polygon by at least 4, so a piece at depth
//! 48 has second differences of at most `2^-96 · D`, where `D ≤ 2√2 · E` for
//! an input curve whose device control box has larger side `E`. A cubic lies
//! within ¾ of its largest second difference of its chord (a quadratic within
//! ¼), so a depth-capped chord deviates from its piece by at most
//! `¾ · 2√2 · 2^-96 · E ≈ 2.7e-29 · E` device pixels: below the flattener's
//! 0.25-pixel tolerance for every curve whose device control box is under
//! about 9.3e27 pixels, and at any size some 10^12 times smaller than the
//! `2^-53 · E` rounding `f64` already applies to the points of such a curve,
//! so the chord never dominates the error. It is confined to the capped
//! piece's hull, and winding everywhere else is unchanged. The flattener
//! stores line end points in `f32`, so geometry whose device coordinates
//! approach `f32`'s range (about 3.4e38) stays bounded in cost but is no
//! longer rasterized faithfully.
//!
//! **Output bound.** A piece is split only while its hull reaches the
//! viewport, and a curve of degree at most 3 crosses each of a rectangle's
//! four edge lines at most 3 times, so it meets the viewport boundary at most
//! 12 times. Each crossing keeps a bounded number of pieces live per halving
//! level, so a curve yields O(12 × [`MAX_SPLIT_DEPTH`]) pieces however large
//! it is, and each reaches the flattener culled, small, or as a line. A device
//! coordinate that overflows to a non-finite value never splits: the
//! flattener drops a `NaN` path and gives a curve with an infinite coordinate
//! no subdivisions. Typical UI geometry never splits and passes through
//! untouched.
//!
//! Strokes and rounded rectangles add an expansion step ahead of the split;
//! [`generate_stroke`] and [`rounded_rect_elements`] state how each is bounded
//! and how accurate it stays. `docs/RENDER_ARCHITECTURE.md` places this pass
//! in the frame path.

use std::f64::consts::SQRT_2;

use kurbo::{
    Affine, Cap, Join, PathEl, Point, Rect, RoundedRect, RoundedRectPathIter, Shape, Stroke,
    StrokeCtx, StrokeOpts,
};
use peniko::Fill;

use vello_common::clip::PathDataRef;
use vello_common::strip_generator::{StripGenerator, StripStorage};

use super::FLATTEN_TOLERANCE;

/// Device pixels added to every side of the viewport before culling
/// decisions. It must cover the flattener's own cull rectangle, which rounds
/// to whole pixels and aligns its top edge down to a strip row: a piece judged
/// outside the expanded viewport must also be outside that rectangle, or the
/// flattener would subdivide it whole.
pub(crate) const VIEWPORT_MARGIN: f64 = 16.0;

/// Largest control-hull extent, in multiples of the expanded viewport's
/// larger side, that a viewport-straddling curve may keep unsplit. The
/// flattener's cost for a piece this size matches that of geometry drawn on
/// screen, and geometry spanning up to a few screens — scrolled content,
/// large rounded panels, glyph outlines at any legible size — stays below it
/// and passes through untouched.
pub(crate) const SPLIT_EXTENT_VIEWPORTS: f64 = 4.0;

/// Maximum number of halvings applied to one curve. A piece that reaches it
/// while still straddling the viewport and still larger than the split extent
/// is emitted as its chord; the module docs derive how close that chord stays.
pub(crate) const MAX_SPLIT_DEPTH: u8 = 48;

/// Pending pieces held at once: one deferred second half per halving level,
/// plus the piece being halved at the deepest level.
const STACK_CAPACITY: usize = MAX_SPLIT_DEPTH as usize + 1;

/// The device-space tolerance `vello_common`'s stroker expands a stroke at,
/// before dividing it by the transform's scale to work in user space.
const STROKE_TOLERANCE: f64 = 0.25;

/// Smallest tolerance an oversized stroke is expanded at, as a fraction of
/// the larger of the stroke's reach and its largest curve piece: `2^-40`.
const STROKE_RELATIVE_TOLERANCE: f64 = 1.0 / (1_u64 << 40) as f64;

/// Smallest tolerance a rounded rectangle's corner arcs are approximated at,
/// as a fraction of its largest radius: `2^-64`.
const ARC_RELATIVE_TOLERANCE: f64 = 1.0 / (1_u128 << 64) as f64;

/// The device rectangle a split decides against, and the control-hull extent
/// past which a piece reaching that rectangle is halved.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SplitRegion {
    bounds: Rect,
    split_extent: f64,
}

impl SplitRegion {
    /// The region for geometry culled against the device-space `viewport`:
    /// the viewport expanded by [`VIEWPORT_MARGIN`] on every side, and
    /// [`SPLIT_EXTENT_VIEWPORTS`] times that rectangle's larger side.
    pub(crate) fn around(viewport: Rect) -> Self {
        Self::of(viewport.abs().inflate(VIEWPORT_MARGIN, VIEWPORT_MARGIN))
    }

    fn of(bounds: Rect) -> Self {
        Self {
            bounds,
            split_extent: SPLIT_EXTENT_VIEWPORTS * bounds.width().max(bounds.height()),
        }
    }

    /// This region with its rectangle grown by `dx` and `dy` on each side and
    /// its split extent derived again from the result.
    fn inflate(self, dx: f64, dy: f64) -> Self {
        Self::of(self.bounds.inflate(dx, dy))
    }
}

/// A quadratic or cubic Bézier segment awaiting a split-or-emit decision, its
/// start point explicit.
#[derive(Clone, Copy, Debug)]
struct Piece {
    /// Control points; only the first three are meaningful for a quadratic,
    /// whose end point is repeated in the fourth.
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

    fn end(&self) -> Point {
        self.pts[3]
    }

    /// This piece with every control point mapped through `affine`. An affine
    /// map commutes with de Casteljau subdivision, so the mapped piece is the
    /// same parameter range of the mapped curve.
    fn mapped(&self, affine: Affine) -> Self {
        Self {
            pts: self.pts.map(|p| affine * p),
            ..*self
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

/// The coordinate space a split works and emits in.
#[derive(Clone, Copy, Debug)]
enum Space {
    /// Every element is mapped to device space as it is read, split there and
    /// emitted there. A piece wholly outside the region stays a curve for the
    /// flattener to drop or chord.
    Device(Affine),
    /// Elements are split and emitted in user space; the transform maps a
    /// piece to device space only to decide its fate. Nothing downstream culls,
    /// so a piece wholly outside the region is emitted as its chord.
    User(Affine),
}

/// What happens to one piece.
enum Verdict {
    Curve,
    Chord,
    Split,
}

/// Allocation-free adapter yielding a path's elements with every huge
/// viewport-straddling curve split into pieces the flattener can process at
/// bounded cost.
///
/// Lines, moves and closes pass through. A curve passes through unchanged
/// unless its control hull both reaches the expanded viewport and exceeds the
/// split extent; such a curve is replaced by consecutive sub-curves tracing the
/// same points in the same direction, any of which that reaches
/// [`MAX_SPLIT_DEPTH`] still needing a split being replaced by its chord.
/// [`ViewportSplit::new`] emits device space — flatten its output with
/// [`Affine::IDENTITY`].
#[derive(Clone, Debug)]
pub(crate) struct ViewportSplit<I> {
    inner: I,
    space: Space,
    region: SplitRegion,
    subpath_start: Point,
    current: Point,
    stack: [Piece; STACK_CAPACITY],
    len: usize,
}

impl<I: Iterator<Item = PathEl>> ViewportSplit<I> {
    /// Wraps a user-space `path` drawn under `affine` and culled against the
    /// device-space `viewport` — the same rectangle the flattener culls
    /// against, which this adapter expands by [`VIEWPORT_MARGIN`]. The output
    /// is in device space.
    pub(crate) fn new(
        path: impl IntoIterator<Item = PathEl, IntoIter = I>,
        affine: Affine,
        viewport: Rect,
    ) -> Self {
        Self::with(
            path.into_iter(),
            Space::Device(affine),
            SplitRegion::around(viewport),
        )
    }

    /// Wraps a stroke's user-space centerline drawn under `transform`, split
    /// against `region` and emitted in user space, with every piece wholly
    /// outside `region` replaced by its chord.
    fn centerline(path: I, transform: Affine, region: SplitRegion) -> Self {
        Self::with(path, Space::User(transform), region)
    }

    fn with(inner: I, space: Space, region: SplitRegion) -> Self {
        Self {
            inner,
            space,
            region,
            subpath_start: Point::ZERO,
            current: Point::ZERO,
            stack: [Piece::EMPTY; STACK_CAPACITY],
            len: 0,
        }
    }

    /// Decides `piece`'s fate from its device-space image. A piece that
    /// would need a split at the depth cap — or with no stack room, which the
    /// depth cap keeps from ever happening — becomes its chord.
    fn verdict(&self, piece: &Piece) -> Verdict {
        let device = match self.space {
            Space::Device(_) => *piece,
            Space::User(transform) => piece.mapped(transform),
        };
        if device.is_outside(self.region.bounds) {
            return match self.space {
                Space::Device(_) => Verdict::Curve,
                Space::User(_) => Verdict::Chord,
            };
        }
        let oversized = device.extent() > self.region.split_extent;
        if !oversized {
            Verdict::Curve
        } else if piece.depth < MAX_SPLIT_DEPTH && self.len + 2 <= STACK_CAPACITY {
            Verdict::Split
        } else {
            Verdict::Chord
        }
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
                match self.verdict(&piece) {
                    Verdict::Split => {
                        let (first, second) = piece.halve();
                        self.push(second);
                        self.push(first);
                        continue;
                    }
                    Verdict::Curve => return Some(piece.to_el()),
                    Verdict::Chord => return Some(PathEl::LineTo(piece.end())),
                }
            }

            let el = self.inner.next()?;
            let el = match self.space {
                Space::Device(affine) => affine * el,
                Space::User(_) => el,
            };
            let piece = match el {
                PathEl::MoveTo(p) => {
                    self.subpath_start = p;
                    self.current = p;
                    return Some(el);
                }
                PathEl::LineTo(p) => {
                    self.current = p;
                    return Some(el);
                }
                PathEl::ClosePath => {
                    self.current = self.subpath_start;
                    return Some(el);
                }
                PathEl::QuadTo(p1, p2) => Piece::quad(self.current, p1, p2),
                PathEl::CurveTo(p1, p2, p3) => Piece::cubic(self.current, p1, p2, p3),
            };
            self.current = piece.end();
            match self.verdict(&piece) {
                Verdict::Split => self.push(piece),
                Verdict::Curve => return Some(el),
                Verdict::Chord => return Some(PathEl::LineTo(piece.end())),
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.len + self.inner.size_hint().0, None)
    }
}

/// The device rectangle the strip generator culls a path against: the active
/// mask's bounds when a draw is generated under one, the generator's viewport
/// otherwise.
pub(crate) fn cull_viewport(generator: &StripGenerator, clip: Option<&PathDataRef<'_>>) -> Rect {
    match clip {
        Some(clip) => Rect::new(
            f64::from(clip.bbox.x0),
            f64::from(clip.bbox.y0),
            f64::from(clip.bbox.x1),
            f64::from(clip.bbox.y1),
        ),
        None => Rect::new(
            0.0,
            0.0,
            f64::from(generator.width()),
            f64::from(generator.height()),
        ),
    }
}

/// Generate the strips for `path` filled non-zero under `transform`, through
/// the [`ViewportSplit`] pre-pass.
///
/// Every fill that can carry a curve reaches the generator this way, so no
/// finite path under a finite transform can make the flattener subdivide
/// without bound. The pre-pass hands back device-space elements, which the
/// generator then flattens under the identity; a path with no curve large
/// enough to split comes out as the same points the generator would have
/// computed from `transform` itself.
pub(crate) fn generate_fill(
    generator: &mut StripGenerator,
    path: impl IntoIterator<Item = PathEl>,
    transform: Affine,
    storage: &mut StripStorage,
    clip: Option<PathDataRef<'_>>,
) {
    let viewport = cull_viewport(generator, clip.as_ref());
    generator.generate_filled_path(
        ViewportSplit::new(path, transform, viewport),
        Fill::NonZero,
        Affine::IDENTITY,
        None,
        storage,
        clip,
    );
}

/// Generate the strips for `path` stroked with `stroke` under `transform`.
/// `stroke` carries no dash pattern: a caller lowers dashes into sub-paths
/// first, and the sub-path count that lowering produces is its own.
///
/// A stroke whose [`stroke_device_extent`] is within the cull viewport's split
/// extent goes to the generator's own stroker unchanged: its expansion
/// flattens at on-screen cost already. A larger one is expanded here, in user
/// space, and the expansion is filled through [`generate_fill`]:
///
/// 1. **The centerline is clipped.** It goes through the same split, in user
///    space, against the expanded viewport inflated further by the stroke's
///    device reach on each axis (the user-space reach mapped through the
///    transform's linear part). A piece wholly beyond one edge of that
///    rectangle becomes its chord: the stroke of anything inside that hull —
///    chord, joins and caps alike — stays beyond the matching edge of the
///    expanded viewport, so no coverage inside it changes. What reaches the
///    expander near the viewport is a bounded number of curve pieces no larger
///    than the inflated rectangle's split extent, plus lines.
/// 2. **The expansion tolerance is floored.** kurbo expands at the tolerance
///    `vello_common`'s stroker would use, `0.25 / max(|a|, |d|, 1)` user units,
///    raised to at least `2^-40` times the larger of the stroke's user-space
///    reach and its largest remaining curve piece. Each element's expansion is
///    then a scale-free problem of bounded relative size, so its output has a
///    fixed ceiling at any magnitude. Under a similarity transform the floor
///    stays below the 0.25-pixel tolerance — the outline is as exact as
///    vello's own — until the stroke's device reach passes about 3e10 pixels,
///    since a kept piece spans at most four inflated viewports, each about
///    twice the reach across. Beyond that the outline error is at most `2^-40`
///    of the larger of the device reach and the kept piece's device size.
///
/// The cost of the whole branch is therefore bounded by the number of input
/// elements, for every finite input, and the clip changes nothing a frame
/// shows; the depth-capped chords it can emit carry the accuracy the module
/// docs derive for fills.
pub(crate) fn generate_stroke<I>(
    generator: &mut StripGenerator,
    path: I,
    stroke: &Stroke,
    transform: Affine,
    storage: &mut StripStorage,
    clip: Option<PathDataRef<'_>>,
) where
    I: IntoIterator<Item = PathEl>,
    I::IntoIter: Clone,
{
    let path = path.into_iter();
    let region = SplitRegion::around(cull_viewport(generator, clip.as_ref()));

    if stroke_device_extent(path.clone(), stroke, transform) <= region.split_extent {
        generator.generate_stroked_path(path, stroke, transform, None, storage, clip);
        return;
    }

    let mut expanded = StrokeCtx::default();
    expand_oversized_stroke(path, stroke, transform, region, &mut expanded);
    generate_fill(
        generator,
        expanded.output().iter(),
        transform,
        storage,
        clip,
    );
}

/// Expands a stroke too large for `vello_common`'s stroker into `expanded`,
/// in user space, by the clipped-centerline and floored-tolerance scheme
/// [`generate_stroke`] describes. `region` is the cull viewport's own.
fn expand_oversized_stroke<I>(
    path: I,
    stroke: &Stroke,
    transform: Affine,
    region: SplitRegion,
    expanded: &mut StrokeCtx,
) where
    I: Iterator<Item = PathEl> + Clone,
{
    let reach = stroke_reach(stroke);
    let [a, b, c, d, _, _] = transform.as_coeffs();
    let region = region.inflate(reach * (a.abs() + c.abs()), reach * (b.abs() + d.abs()));
    let centerline = ViewportSplit::centerline(path, transform, region);
    let tolerance = expansion_tolerance(centerline.clone(), reach, transform);
    kurbo::stroke_with(
        centerline,
        stroke,
        &StrokeOpts::default(),
        tolerance,
        expanded,
    );
}

/// The user-space tolerance an oversized stroke's clipped `centerline` is
/// expanded at: `vello_common`'s own, floored at [`STROKE_RELATIVE_TOLERANCE`]
/// of the larger of `reach` and the largest curve piece's extent. A size that
/// overflows yields the largest finite tolerance rather than an infinite one.
fn expansion_tolerance(
    centerline: impl Iterator<Item = PathEl>,
    reach: f64,
    transform: Affine,
) -> f64 {
    let mut size = reach;
    let mut start = Point::ZERO;
    let mut current = Point::ZERO;
    for el in centerline {
        let piece = match el {
            PathEl::MoveTo(p) => {
                start = p;
                current = p;
                continue;
            }
            PathEl::LineTo(p) => {
                current = p;
                continue;
            }
            PathEl::ClosePath => {
                current = start;
                continue;
            }
            PathEl::QuadTo(p1, p2) => Piece::quad(current, p1, p2),
            PathEl::CurveTo(p1, p2, p3) => Piece::cubic(current, p1, p2, p3),
        };
        current = piece.end();
        size = size.max(piece.extent());
    }

    let [a, _, _, d, _, _] = transform.as_coeffs();
    let tolerance =
        (STROKE_TOLERANCE / a.abs().max(d.abs()).max(1.0)).max(STROKE_RELATIVE_TOLERANCE * size);
    if tolerance.is_finite() {
        tolerance
    } else {
        f64::MAX
    }
}

/// How far a stroke's outline can stand from its centerline, in user units:
/// half the width, times the miter limit when joins are mitred (a miter tip
/// stands at most that many half-widths from its vertex) and times √2 when a
/// cap is square (a square cap's far corners).
fn stroke_reach(stroke: &Stroke) -> f64 {
    let mut factor: f64 = 1.0;
    if matches!(stroke.join, Join::Miter) {
        factor = factor.max(stroke.miter_limit);
    }
    if matches!(stroke.start_cap, Cap::Square) || matches!(stroke.end_cap, Cap::Square) {
        factor = factor.max(SQRT_2);
    }
    0.5 * stroke.width.abs() * factor
}

/// The larger side of the device-space box bounding everything a stroke of
/// `path` can cover: its control box widened by [`stroke_reach`] on every
/// side, under `transform`. Zero for a path with no points.
fn stroke_device_extent(
    path: impl Iterator<Item = PathEl>,
    stroke: &Stroke,
    transform: Affine,
) -> f64 {
    let mut control: Option<Rect> = None;
    let mut include = |p: Point| {
        control = Some(control.map_or(Rect::from_points(p, p), |r| r.union_pt(p)));
    };
    for el in path {
        match el {
            PathEl::MoveTo(p) | PathEl::LineTo(p) => include(p),
            PathEl::QuadTo(p1, p2) => {
                include(p1);
                include(p2);
            }
            PathEl::CurveTo(p1, p2, p3) => {
                include(p1);
                include(p2);
                include(p3);
            }
            PathEl::ClosePath => {}
        }
    }
    let Some(control) = control else {
        return 0.0;
    };
    let reach = stroke_reach(stroke);
    let device = transform.transform_rect_bbox(control.inflate(reach, reach));
    device.width().max(device.height())
}

/// The elements of `shape`, its corner arcs approximated at
/// [`FLATTEN_TOLERANCE`] raised to at least [`ARC_RELATIVE_TOLERANCE`] of its
/// largest radius.
///
/// kurbo approximates an arc with a number of cubics growing as the sixth
/// root of radius over tolerance, so an unfloored radius of 1e300 would yield
/// around 1e50 elements before any split ran. The floor caps that at about
/// 1656 per full turn — at most 414 per corner. Up to a radius of
/// `FLATTEN_TOLERANCE / ARC_RELATIVE_TOLERANCE` (about 1.8e18 user units) the
/// floor is inactive and the elements are exactly the unfloored ones; past it
/// the arc error is `2^-64` of the radius, below the `2^-53` relative rounding
/// `f64` already applies to the coordinates of such an arc.
pub(crate) fn rounded_rect_elements(shape: &RoundedRect) -> RoundedRectPathIter {
    let radii = shape.radii();
    let largest = radii
        .top_left
        .max(radii.top_right)
        .max(radii.bottom_right)
        .max(radii.bottom_left);
    shape.path_elements(FLATTEN_TOLERANCE.max(ARC_RELATIVE_TOLERANCE * largest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    use kurbo::{
        BezPath, Circle, CubicBez, ParamCurve, ParamCurveNearest, PathSeg, QuadBez,
        RoundedRectRadii,
    };
    use vello_common::fearless_simd::Level;
    use vello_common::strip_generator::GenerationMode;

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
        let region = SplitRegion::around(VIEWPORT);
        for (path, affine) in huge_cases(1e9) {
            let out: BezPath = BezPath::from_vec(split(&path, affine));
            for seg in out.segments() {
                let piece = match seg {
                    PathSeg::Line(_) => continue,
                    PathSeg::Quad(q) => Piece::quad(q.p0, q.p1, q.p2),
                    PathSeg::Cubic(c) => Piece::cubic(c.p0, c.p1, c.p2, c.p3),
                };
                assert!(
                    piece.is_outside(region.bounds) || piece.extent() <= region.split_extent,
                    "piece {piece:?} straddles the viewport at extent {}",
                    piece.extent()
                );
            }
        }
    }

    /// A lone straddling curve far past the depth cap's reach — 1e50 and
    /// 1e300 pixels need about 154 and 984 halvings to reach the split extent
    /// — splits into a bounded number of elements, and every piece that
    /// reached the cap while still straddling the viewport is a line: no
    /// emitted curve both reaches the expanded viewport and exceeds the split
    /// extent.
    #[test]
    fn depth_capped_pieces_are_emitted_as_chords() {
        let region = SplitRegion::around(VIEWPORT);
        for size in [1e50, 1e300] {
            let quad = QuadBez::new((-size, size), (800.0, 600.0 - size), (size, size));
            let cubic = CubicBez::new(
                (-size, 0.0),
                (size, 3.0 * size),
                (3200.0 / 3.0 - size, 600.0 - 3.0 * size),
                (size, 600.0),
            );
            for curve in [PathSeg::Quad(quad), PathSeg::Cubic(cubic)] {
                let path = BezPath::from_path_segments(std::iter::once(curve));
                let out = split(&path, Affine::IDENTITY);
                assert!(
                    out.len() < 2000,
                    "a {size:e}-pixel {curve:?} yields {} elements",
                    out.len()
                );
                let cost = straddling_flatten_cost(&out);
                assert!(
                    cost < 2000,
                    "a {size:e}-pixel curve flattens to {cost} lines"
                );

                let chords = out
                    .iter()
                    .filter(|el| matches!(el, PathEl::LineTo(_)))
                    .count();
                assert!(
                    chords > 0,
                    "a {size:e}-pixel curve must reach the depth cap and emit chords"
                );
                for (_, piece) in with_pieces(&out) {
                    let Some(piece) = piece else { continue };
                    assert!(
                        piece.is_outside(region.bounds) || piece.extent() <= region.split_extent,
                        "a {size:e}-pixel curve emitted a straddling curve piece of extent {}",
                        piece.extent()
                    );
                }
                let end = out.last().and_then(|el| el.end_point());
                assert_eq!(end.map(point_bits), Some(point_bits(curve.end())));
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
        let bounds = SplitRegion::around(VIEWPORT).bounds;
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
        let bounds = SplitRegion::around(VIEWPORT).bounds;
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

    // -----------------------------------------------------------------------
    // Strokes
    // -----------------------------------------------------------------------

    /// Strip-generator viewport for the stroke tests, matching the property
    /// suite's frame.
    const STROKE_VIEWPORT: (u16, u16) = (64, 64);

    /// Wall-time ceiling for one oversized stroke, expansion and strips
    /// together: a bounded expansion takes milliseconds even unoptimized.
    const STROKE_TIME_LIMIT: Duration = Duration::from_millis(250);

    /// Largest outline an oversized stroke in these tests may expand to.
    const STROKE_OUTLINE_LIMIT: usize = 20_000;

    fn stroke_styles() -> [(&'static str, Stroke); 3] {
        [
            (
                "round",
                Stroke::new(1.0)
                    .with_join(Join::Round)
                    .with_caps(Cap::Round),
            ),
            (
                "miter",
                Stroke::new(1.0)
                    .with_join(Join::Miter)
                    .with_miter_limit(10.0)
                    .with_caps(Cap::Butt),
            ),
            (
                "square",
                Stroke::new(1.0)
                    .with_join(Join::Bevel)
                    .with_caps(Cap::Square),
            ),
        ]
    }

    /// Centerlines crossing the stroke viewport at magnitude `size`: a
    /// straight line, a polyline with a right-angle corner inside the
    /// viewport, and an S-shaped cubic through its centre.
    fn stroke_centerlines(size: f64) -> [(&'static str, BezPath); 3] {
        let mut line = BezPath::new();
        line.move_to((-size, 32.0));
        line.line_to((size, 32.0));

        let mut corner = BezPath::new();
        corner.move_to((-size, 32.0 - size));
        corner.line_to((32.0, 32.0));
        corner.line_to((size, 32.0 - size));

        let mut curve = BezPath::new();
        curve.move_to((-size, 0.0));
        curve.curve_to(
            (size, 3.0 * size),
            (256.0 / 3.0 - size, 64.0 - 3.0 * size),
            (size, 64.0),
        );
        [("line", line), ("corner", corner), ("curve", curve)]
    }

    fn stroke_region() -> SplitRegion {
        SplitRegion::around(Rect::new(
            0.0,
            0.0,
            f64::from(STROKE_VIEWPORT.0),
            f64::from(STROKE_VIEWPORT.1),
        ))
    }

    #[test]
    fn stroke_reach_covers_miter_tips_and_square_corners() {
        let round = Stroke::new(4.0);
        assert_eq!(stroke_reach(&round), 2.0);
        let miter = Stroke::new(4.0)
            .with_join(Join::Miter)
            .with_miter_limit(10.0);
        assert_eq!(stroke_reach(&miter), 20.0);
        let square = Stroke::new(4.0)
            .with_join(Join::Bevel)
            .with_caps(Cap::Square);
        assert_eq!(stroke_reach(&square), 2.0 * SQRT_2);

        let mut path = BezPath::new();
        path.move_to((0.0, 0.0));
        path.line_to((100.0, 0.0));
        let extent = |stroke: &Stroke| stroke_device_extent(path.iter(), stroke, Affine::IDENTITY);
        assert_eq!(extent(&round), 104.0);
        assert_eq!(extent(&miter), 140.0);
        assert!((extent(&square) - (100.0 + 4.0 * SQRT_2)).abs() < 1e-9);
    }

    /// Every style over every centerline, thin and as wide as the geometry
    /// itself, expands to a bounded outline in bounded time and generates its
    /// strips in bounded time, at magnitudes up to 1e300.
    #[test]
    fn oversized_strokes_expand_and_generate_at_bounded_cost() {
        let region = stroke_region();
        let mut generator =
            StripGenerator::new(STROKE_VIEWPORT.0, STROKE_VIEWPORT.1, Level::baseline());
        for size in [1e20, 1e40, 1e100, 1e300] {
            for (style, base) in stroke_styles() {
                for width in [2.0, size] {
                    let stroke = Stroke {
                        width,
                        ..base.clone()
                    };
                    for (shape, path) in stroke_centerlines(size) {
                        let case = format!("{style} {shape} at {size:e}, width {width:e}");
                        assert!(
                            stroke_device_extent(path.iter(), &stroke, Affine::IDENTITY)
                                > region.split_extent,
                            "{case} must take the oversized branch"
                        );

                        let started = Instant::now();
                        let mut expanded = StrokeCtx::default();
                        expand_oversized_stroke(
                            path.iter(),
                            &stroke,
                            Affine::IDENTITY,
                            region,
                            &mut expanded,
                        );
                        let outline = expanded.output().elements().len();
                        let mut storage = StripStorage::new(GenerationMode::Append);
                        generate_stroke(
                            &mut generator,
                            path.iter(),
                            &stroke,
                            Affine::IDENTITY,
                            &mut storage,
                            None,
                        );
                        let elapsed = started.elapsed();

                        assert!(
                            outline <= STROKE_OUTLINE_LIMIT,
                            "{case} expanded to {outline} elements"
                        );
                        assert!(elapsed < STROKE_TIME_LIMIT, "{case} took {elapsed:?}");
                    }
                }
            }
        }
    }

    /// A thin stroke along a huge parabola whose apex crosses the viewport
    /// keeps the band it covers there: inside within the half width of the
    /// apex row, outside beyond it, sampled clear of the 0.25-pixel tolerance
    /// band at the outline. The apex lies inside the curve, not at an end
    /// point, so the band is only right if the expansion is accurate there.
    #[test]
    fn a_thin_stroke_of_a_huge_curve_keeps_its_band_inside_the_viewport() {
        let region = stroke_region();
        // `y = x²` over `-1..1`: its apex sits exactly on the user-space
        // origin, and across the viewport it departs from the device row
        // y = 32 by at most `32² / scale` pixels.
        let mut parabola = BezPath::new();
        parabola.move_to((-1.0, 1.0));
        parabola.quad_to((0.0, -1.0), (1.0, 1.0));
        for scale in [1e9, 1e20, 1e25] {
            let transform = Affine::translate((32.0, 32.0)) * Affine::scale(scale);
            let half_width = 1.5;
            for (style, base) in stroke_styles() {
                let stroke = Stroke {
                    width: 2.0 * half_width / scale,
                    ..base.clone()
                };
                let mut expanded = StrokeCtx::default();
                expand_oversized_stroke(parabola.iter(), &stroke, transform, region, &mut expanded);
                let outline = transform * expanded.output().clone();
                for ix in 0..16 {
                    for iy in 0..=64 {
                        let p = Point::new(ix as f64 * 4.0 + 1.37, iy as f64 * 0.5 + 16.11);
                        let offset = (p.y - 32.0).abs();
                        if (offset - half_width).abs() < 0.3 {
                            continue;
                        }
                        let inside = far_safe_winding(&outline, p) != 0;
                        assert_eq!(
                            inside,
                            offset < half_width,
                            "{style} stroke at scale {scale:e}, at {p:?}"
                        );
                    }
                }
            }
        }
    }

    /// The winding number of `path` around `p`, computed without
    /// re-evaluating a segment from its parameters: kurbo's own winding
    /// re-derives a line's end point as `p0 + (p1 - p0) * 1.0`, which a
    /// 1e20-pixel line rounds away from its true end. A curve wholly beside
    /// `p` counts as its chord, as in the flattener; one whose hull holds `p`
    /// is flattened finely.
    fn far_safe_winding(path: &BezPath, p: Point) -> i32 {
        let crossing = |a: Point, b: Point| -> i32 {
            let (low, high, sign) = if a.y < b.y {
                (a, b, -1)
            } else if a.y > b.y {
                (b, a, 1)
            } else {
                return 0;
            };
            if p.y < low.y || p.y >= high.y || p.x < a.x.min(b.x) {
                return 0;
            }
            if p.x >= a.x.max(b.x) {
                return sign;
            }
            let x = low.x + (p.y - low.y) / (high.y - low.y) * (high.x - low.x);
            if x <= p.x { sign } else { 0 }
        };
        let mut winding = 0;
        for seg in path.segments() {
            let piece = match seg {
                PathSeg::Line(line) => {
                    winding += crossing(line.p0, line.p1);
                    continue;
                }
                PathSeg::Quad(q) => Piece::quad(q.p0, q.p1, q.p2),
                PathSeg::Cubic(c) => Piece::cubic(c.p0, c.p1, c.p2, c.p3),
            };
            let pts = piece.points();
            if pts.iter().all(|q| q.x < p.x)
                || pts.iter().all(|q| q.x > p.x)
                || pts.iter().all(|q| q.y < p.y)
                || pts.iter().all(|q| q.y > p.y)
            {
                winding += crossing(piece.pts[0], piece.end());
                continue;
            }
            let mut last = piece.pts[0];
            kurbo::flatten([PathEl::MoveTo(last), piece.to_el()], 1e-4, |el| {
                if let PathEl::LineTo(next) = el {
                    winding += crossing(last, next);
                    last = next;
                }
            });
        }
        winding
    }

    // -----------------------------------------------------------------------
    // Rounded rectangles
    // -----------------------------------------------------------------------

    #[test]
    fn rounded_rect_arcs_are_bit_identical_below_the_floor_and_bounded_above_it() {
        for radius in [0.5, 12.0, 1e6, 1e15] {
            let shape = RoundedRect::from_rect(
                Rect::new(-radius, 0.0, radius, 2.0 * radius),
                RoundedRectRadii::from_single_radius(radius),
            );
            let expected: Vec<PathEl> = shape.path_elements(FLATTEN_TOLERANCE).collect();
            let actual: Vec<PathEl> = rounded_rect_elements(&shape).collect();
            assert_eq!(actual.len(), expected.len(), "radius {radius:e}");
            for (a, e) in actual.iter().zip(&expected) {
                assert!(same_kind(*a, *e), "{a:?} vs {e:?} at radius {radius:e}");
                assert_eq!(
                    el_bits(*a),
                    el_bits(*e),
                    "{a:?} vs {e:?} at radius {radius:e}"
                );
            }
        }
        for radius in [1e20, 1e40, 1e100, 1e300] {
            let shape = RoundedRect::from_rect(
                Rect::new(-radius, 0.0, radius, 2.0 * radius),
                RoundedRectRadii::from_single_radius(radius),
            );
            let count = rounded_rect_elements(&shape).count();
            assert!(
                count <= 4 * 414 + 10,
                "radius {radius:e} yields {count} elements"
            );
        }
    }
}
