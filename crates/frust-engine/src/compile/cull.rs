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
//! Strokes and rounded rectangles add an expansion step ahead of the split,
//! and dashed strokes a clip of the centerline ahead of the dash lowering;
//! [`generate_stroke`], [`generate_dashed_stroke`] and [`rounded_rect_elements`]
//! state how each is bounded and how accurate it stays.
//! `docs/RENDER_ARCHITECTURE.md` places this pass in the frame path.

use std::f64::consts::SQRT_2;

use kurbo::{
    Affine, BezPath, Cap, CubicBez, Join, ParamCurveArclen, PathEl, Point, QuadBez, Rect,
    RoundedRect, RoundedRectPathIter, Shape, Stroke, StrokeCtx, StrokeOpts,
};
use peniko::Fill;

use frust_scene::DashPattern;

use vello_common::clip::PathDataRef;
use vello_common::strip_generator::{StripGenerator, StripStorage};

use super::{FLATTEN_TOLERANCE, dash_path, well_formed};

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

/// Smallest device-space tolerance an oversized stroke is expanded at, as a
/// fraction of the larger of the stroke's device reach and its largest kept
/// curve piece's device extent: `2^-40`.
const STROKE_RELATIVE_TOLERANCE: f64 = 1.0 / (1_u64 << 40) as f64;

/// Largest stroke, in multiples of the user-space tolerance `vello_common`'s
/// stroker would expand it at, that is left to that stroker: `2^32`. A stroke
/// within the split extent at a scale of one or more spans under `2^17`
/// tolerances even on an 8K display, and below a scale of one the tolerance
/// is a quarter unit, so a zoomed-out 4000-unit page spans `2^14`: only a
/// stroke many orders of magnitude larger in user space than anything it
/// shows reaches the ceiling.
const STROKER_EXTENT_CEILING: f64 = (1_u64 << 32) as f64;

/// Smallest tolerance a rounded rectangle's corner arcs are approximated at,
/// as a fraction of its largest radius: `2^-64`.
const ARC_RELATIVE_TOLERANCE: f64 = 1.0 / (1_u128 << 64) as f64;

/// Smallest device-space dash period, in pixels, an oversized dashed stroke is
/// still broken into dashes at; below it the stroke is drawn solid. Half a
/// pixel: a cycle that fits in half a pixel rasterizes as an even partial
/// coverage, not as visible dashes, and a real UI period — a few logical
/// pixels at a device scale of one or more — sits several times above it.
pub(crate) const MIN_DEVICE_DASH_PERIOD: f64 = 0.5;

/// Most dashes one path element — a line, quad or cubic, or a kept span of
/// one in the capped lowering — is broken into, estimated as its
/// control-polygon length over the pattern period. The bound is per element,
/// so a path's total is at most its element count times this budget; a run of
/// many on-screen elements stays dashed, and only an element that is itself
/// absurdly long for its period is stroked solid. A real UI pattern sits far
/// below it: a 4K-wide line at a 2-pixel period is under 2 000 dashes, and a
/// span the split kept is at most a few split extents long.
pub(crate) const MAX_DASHES_PER_ELEMENT: f64 = 65_536.0;

/// The accuracy, as a fraction of a piece's control-polygon length, an
/// oversized dashed stroke measures arc length at to carry its dash phase.
/// kurbo's own error estimate for its first quadrature never exceeds
/// `3.5e-3` of that length, so the measure never subdivides at any magnitude.
const DASH_ARCLEN_RELATIVE_ACCURACY: f64 = 1.0 / 256.0;

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
/// A stroke whose device extent is within the cull viewport's split
/// extent, and whose user-space extent is within [`STROKER_EXTENT_CEILING`]
/// times the tolerance the generator's own stroker expands it at
/// ([`stroker_tolerance`]), goes to that stroker unchanged: its expansion
/// flattens at on-screen cost already. Any other is expanded here, in user
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
/// 2. **The expansion tolerance is set in device space.** It is 0.25 pixel,
///    raised to at least `2^-40` of the larger of the stroke's device reach —
///    its user-space reach times the transform's largest stretch `σ` — and
///    the largest remaining curve piece's device extent, and kurbo expands at
///    that divided by `σ`, in user units. No direction stretches by more than
///    `σ`, so under every affine transform the outline lies within that many
///    device pixels of the exact one: within 0.25 pixel — vello's own
///    accuracy under a similarity transform — until the device reach passes
///    about 3e10 pixels, since a kept piece spans at most four inflated
///    viewports, each about twice the reach across, and within `2^-40` of the
///    larger of the device reach and the kept piece's device extent beyond
///    that. The user-space tolerance is never below `2^-40` of the user-space
///    reach, and kurbo fits each element's offsets and arcs with a bounded
///    number of curves at any tolerance, so the outline has a fixed ceiling
///    per element at any magnitude.
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

    if fits_the_stroker(path.clone(), stroke, transform, region) {
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

/// Whether a stroke of `path` goes to `vello_common`'s own stroker: its
/// device extent is within the split extent of `region`, the cull
/// viewport's own, and the larger side of its user-space box — the control
/// box widened by [`stroke_reach`] — is within [`STROKER_EXTENT_CEILING`]
/// times [`stroker_tolerance`]. A path with no points fits.
fn fits_the_stroker(
    path: impl Iterator<Item = PathEl>,
    stroke: &Stroke,
    transform: Affine,
    region: SplitRegion,
) -> bool {
    let Some(bounds) = stroke_bounds(path, stroke) else {
        return true;
    };
    device_extent(bounds, transform) <= region.split_extent
        && bounds.width().max(bounds.height())
            <= STROKER_EXTENT_CEILING * stroker_tolerance(transform)
}

/// The user-space tolerance `vello_common`'s stroker expands a stroke drawn
/// under `transform` at: `0.25 / max(|a|, |d|, 1)`.
fn stroker_tolerance(transform: Affine) -> f64 {
    let [a, _, _, d, _, _] = transform.as_coeffs();
    STROKE_TOLERANCE / a.abs().max(d.abs()).max(1.0)
}

/// Expands a stroke too large for `vello_common`'s stroker into `expanded`,
/// in user space, by the clipped-centerline and device-space tolerance scheme
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
    expand_stroke_at(path, stroke, transform, region, STROKE_TOLERANCE, expanded);
}

/// [`expand_oversized_stroke`] with `base` pixels in place of
/// [`STROKE_TOLERANCE`] as the device-space tolerance the floor raises.
fn expand_stroke_at<I>(
    path: I,
    stroke: &Stroke,
    transform: Affine,
    region: SplitRegion,
    base: f64,
    expanded: &mut StrokeCtx,
) where
    I: Iterator<Item = PathEl> + Clone,
{
    let reach = stroke_reach(stroke);
    let [a, b, c, d, _, _] = transform.as_coeffs();
    let region = region.inflate(reach * (a.abs() + c.abs()), reach * (b.abs() + d.abs()));
    let centerline = ViewportSplit::centerline(path, transform, region);
    let tolerance = expansion_tolerance(centerline.clone(), reach, transform, base);
    kurbo::stroke_with(
        centerline,
        stroke,
        &StrokeOpts::default(),
        tolerance,
        expanded,
    );
}

/// The user-space tolerance an oversized stroke's clipped `centerline` is
/// expanded at: `base` device pixels, floored at [`STROKE_RELATIVE_TOLERANCE`]
/// of the larger of the device reach — `reach` times the [`largest_stretch`]
/// of `transform` — and the largest curve piece's device extent, divided by
/// that stretch. A tolerance that overflows, or a transform that stretches
/// nothing, yields the largest finite tolerance rather than an infinite one.
fn expansion_tolerance(
    centerline: impl Iterator<Item = PathEl>,
    reach: f64,
    transform: Affine,
    base: f64,
) -> f64 {
    let [a, b, c, d, _, _] = transform.as_coeffs();
    let linear = Affine::new([a, b, c, d, 0.0, 0.0]);
    let stretch = largest_stretch(transform);
    let mut size = reach * stretch;
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
        size = size.max(piece.mapped(linear).extent());
    }

    let tolerance = base.max(STROKE_RELATIVE_TOLERANCE * size) / stretch;
    if tolerance.is_finite() && tolerance > 0.0 {
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
/// side, under `transform`. Zero for a path with no points. The gates decide
/// on [`fits_the_stroker`]; the tests measure a stroke's screen size with this.
#[cfg(test)]
fn stroke_device_extent(
    path: impl Iterator<Item = PathEl>,
    stroke: &Stroke,
    transform: Affine,
) -> f64 {
    stroke_bounds(path, stroke).map_or(0.0, |bounds| device_extent(bounds, transform))
}

/// The larger side of the device-space box bounding user-space `bounds`
/// under `transform`.
fn device_extent(bounds: Rect, transform: Affine) -> f64 {
    let device = transform.transform_rect_bbox(bounds);
    device.width().max(device.height())
}

/// The user-space box bounding everything a stroke of `path` can cover: its
/// control box widened by [`stroke_reach`] on every side. `None` for a path
/// with no points.
fn stroke_bounds(path: impl Iterator<Item = PathEl>, stroke: &Stroke) -> Option<Rect> {
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
    let reach = stroke_reach(stroke);
    control.map(|control| control.inflate(reach, reach))
}

/// Generate the strips for `path` stroked with `stroke` and broken into
/// dashes by an effective `dash`, under `transform`.
///
/// A stroke that [`fits_the_stroker`] — of the undashed path — is dashed by
/// [`dash_path`] over its whole length and handed to [`generate_stroke`],
/// exactly as every other consumer of the display list lowers it. A larger one
/// is lowered by [`dash_visible_stretch`], whose cost is bounded by the number
/// of input elements for every finite input, and its result goes to
/// [`generate_stroke`] in turn.
pub(crate) fn generate_dashed_stroke(
    generator: &mut StripGenerator,
    path: &BezPath,
    dash: DashPattern,
    stroke: &Stroke,
    transform: Affine,
    storage: &mut StripStorage,
    clip: Option<PathDataRef<'_>>,
) {
    let region = SplitRegion::around(cull_viewport(generator, clip.as_ref()));
    let dashed = dash_within(path, dash, stroke, transform, region);
    generate_stroke(generator, dashed.iter(), stroke, transform, storage, clip);
}

/// `path` broken into the dashes of `dash`: by [`dash_path`] only when its
/// stroke [`fits_the_stroker`] in `region`, the pattern's device period is at
/// least [`MIN_DEVICE_DASH_PERIOD`] and no element exceeds
/// [`MAX_DASHES_PER_ELEMENT`] periods, and by [`dash_visible_stretch`]
/// otherwise. Extents alone never decide: [`dash_path`] iterates every period
/// of an element, so a fine period over a long on-screen line is refused.
fn dash_within(
    path: &BezPath,
    dash: DashPattern,
    stroke: &Stroke,
    transform: Affine,
    region: SplitRegion,
) -> BezPath {
    let period = dash.on + dash.off;
    let device_period = period * largest_stretch(transform);
    if fits_the_stroker(path.iter(), stroke, transform, region)
        && device_period >= MIN_DEVICE_DASH_PERIOD
        && within_dash_budget(path, period)
    {
        dash_path(path, dash)
    } else {
        dash_visible_stretch(path, dash, stroke, transform, region)
    }
}

/// Whether every element of `path` is at most [`MAX_DASHES_PER_ELEMENT`]
/// `period`s of control polygon; a non-finite estimate fails.
fn within_dash_budget(path: &BezPath, period: f64) -> bool {
    let limit = MAX_DASHES_PER_ELEMENT * period;
    let mut start = Point::ZERO;
    let mut current = Point::ZERO;
    for el in path.iter() {
        let span = match el {
            PathEl::MoveTo(p) => {
                start = p;
                current = p;
                continue;
            }
            PathEl::ClosePath => Span::line(current, start),
            PathEl::LineTo(p) => Span::line(current, p),
            PathEl::QuadTo(p1, p2) => Span::curve(Piece::quad(current, p1, p2)),
            PathEl::CurveTo(p1, p2, p3) => Span::curve(Piece::cubic(current, p1, p2, p3)),
        };
        let length = span.polygon_length();
        if length.is_nan() || length > limit {
            return false;
        }
        current = span.piece.end();
    }
    true
}

/// Dashes only the stretch of an oversized stroke's centerline that can reach
/// the expanded viewport, carrying the dash phase across the rest.
///
/// 1. **The centerline is clipped** against the expanded viewport inflated by
///    the stroke's device reach — the rectangle [`generate_stroke`]'s
///    oversized branch clips against. A curve is halved as in
///    [`ViewportSplit`], a piece still oversized at [`MAX_SPLIT_DEPTH`]
///    becoming its chord; a line, chord or not, is trimmed by bisection with
///    no depth cap. A piece wholly beyond one edge is skipped: no stroke of it,
///    cap or join included, reaches the expanded viewport. Consecutive kept
///    pieces form a run, dashed as its own subpath; a dash a skip cuts gains a
///    cap at the cut, which lies beyond the expanded viewport as well.
/// 2. **The phase is carried.** A run starts at the dash offset its subpath's
///    travelled arc length gives it, each piece measured at
///    [`DASH_ARCLEN_RELATIVE_ACCURACY`] of its control-polygon length — by
///    kurbo's own estimate, far tighter for a smooth piece, exact for a line —
///    so a run is out of phase by at most that fraction of the polygon length
///    before it. The error moves where dashes fall, never whether the stroke
///    draws or how wide. A travelled length that overflows starts the run at
///    the pattern's own phase.
/// 3. **Dashing is capped.** A run is stroked solid, losing the pattern's
///    duty cycle, when the pattern's device period is below
///    [`MIN_DEVICE_DASH_PERIOD`] or any span kept in it exceeds
///    [`MAX_DASHES_PER_ELEMENT`] periods.
///
/// A subpath nothing was skipped from is one run and keeps its closing
/// segment, so it dashes as [`dash_path`] dashes it. A closed subpath with a
/// skip is dashed open at its start: where kurbo joins its last dash to its
/// first, the two meet there with a cap each, the same coverage under the
/// round caps and joins every display-list stroke has.
fn dash_visible_stretch(
    path: &BezPath,
    dash: DashPattern,
    stroke: &Stroke,
    transform: Affine,
    region: SplitRegion,
) -> BezPath {
    let reach = stroke_reach(stroke);
    let [a, b, c, d, _, _] = transform.as_coeffs();
    let region = region.inflate(reach * (a.abs() + c.abs()), reach * (b.abs() + d.abs()));
    let mut lowering = DashLowering::new(dash, transform, region);

    let source = well_formed(path.iter());
    let mut start = Point::ZERO;
    let mut current = Point::ZERO;
    for el in source.iter() {
        let span = match el {
            PathEl::MoveTo(p) => {
                lowering.end_subpath(false);
                start = p;
                current = p;
                continue;
            }
            PathEl::ClosePath => {
                if current != start {
                    lowering.segment(Span::line(current, start));
                }
                current = start;
                lowering.end_subpath(true);
                continue;
            }
            PathEl::LineTo(p) => Span::line(current, p),
            PathEl::QuadTo(p1, p2) => Span::curve(Piece::quad(current, p1, p2)),
            PathEl::CurveTo(p1, p2, p3) => Span::curve(Piece::cubic(current, p1, p2, p3)),
        };
        current = span.piece.end();
        lowering.segment(span);
    }
    lowering.end_subpath(false);
    well_formed(lowering.out.iter())
}

/// The largest factor the linear part of `transform` stretches any direction
/// by: its largest singular value, in closed form.
fn largest_stretch(transform: Affine) -> f64 {
    let [a, b, c, d, _, _] = transform.as_coeffs();
    0.5 * ((a + d).hypot(b - c) + (a - d).hypot(b + c))
}

/// One element of a dashed centerline awaiting a keep-or-skip decision: a
/// curve piece, or a line held as the quadratic with its control point at the
/// midpoint, whose halving is the line's own.
#[derive(Clone, Copy, Debug)]
struct Span {
    piece: Piece,
    line: bool,
}

impl Span {
    const EMPTY: Self = Self {
        piece: Piece::EMPTY,
        line: false,
    };

    fn line(p0: Point, p1: Point) -> Self {
        Self {
            piece: Piece::quad(p0, p0.midpoint(p1), p1),
            line: true,
        }
    }

    fn curve(piece: Piece) -> Self {
        Self { piece, line: false }
    }

    fn halve(self) -> (Self, Self) {
        let (first, second) = self.piece.halve();
        (
            Self {
                piece: first,
                ..self
            },
            Self {
                piece: second,
                ..self
            },
        )
    }

    fn to_el(self) -> PathEl {
        if self.line {
            PathEl::LineTo(self.piece.end())
        } else {
            self.piece.to_el()
        }
    }

    /// The length of the control polygon, an upper bound on the arc length.
    fn polygon_length(&self) -> f64 {
        if self.line {
            return (self.piece.end() - self.piece.pts[0]).hypot();
        }
        self.piece
            .points()
            .windows(2)
            .map(|pair| (pair[1] - pair[0]).hypot())
            .sum()
    }

    /// The arc length at [`DASH_ARCLEN_RELATIVE_ACCURACY`] of the polygon
    /// length; a degenerate or overflowing polygon measures as its own length.
    fn arclen(&self) -> f64 {
        let polygon = self.polygon_length();
        if self.line || polygon == 0.0 || !polygon.is_finite() {
            return polygon;
        }
        let accuracy = DASH_ARCLEN_RELATIVE_ACCURACY * polygon;
        let [p0, p1, p2, p3] = self.piece.pts;
        if self.piece.cubic {
            CubicBez::new(p0, p1, p2, p3).arclen(accuracy)
        } else {
            QuadBez::new(p0, p1, p2).arclen(accuracy)
        }
    }
}

/// Most halvings one line is trimmed by: enough to take any finite `f64`
/// length down to the smallest subnormal, so the bound is never what stops a
/// trim short of the split extent.
const LINE_TRIM_STEPS: usize = 2100;

/// What happens to one piece of a dashed centerline.
#[derive(Clone, Copy, Debug)]
enum Fate {
    Skip,
    Keep,
    Split,
}

/// The running state of [`dash_visible_stretch`]: the run being gathered,
/// the arc length travelled along the current subpath, and the output.
struct DashLowering {
    pattern: [f64; 2],
    phase: f64,
    period: f64,
    /// Whether the pattern's device period is below [`MIN_DEVICE_DASH_PERIOD`].
    sub_pixel: bool,
    transform: Affine,
    region: SplitRegion,
    run: BezPath,
    /// Whether a span kept in the current run exceeds the dash budget.
    over_budget: bool,
    run_offset: f64,
    travelled: f64,
    skipped: bool,
    out: BezPath,
}

impl DashLowering {
    fn new(dash: DashPattern, transform: Affine, region: SplitRegion) -> Self {
        let period = dash.on + dash.off;
        let device_period = period * largest_stretch(transform);
        Self {
            pattern: [dash.on, dash.off],
            phase: dash.phase,
            period,
            sub_pixel: device_period.is_nan() || device_period < MIN_DEVICE_DASH_PERIOD,
            transform,
            region,
            run: BezPath::new(),
            over_budget: false,
            run_offset: 0.0,
            travelled: 0.0,
            skipped: false,
            out: BezPath::new(),
        }
    }

    /// Whether `span` is skipped, kept or halved further.
    fn fate(&self, span: &Span) -> Fate {
        let device = span.piece.mapped(self.transform);
        if device.is_outside(self.region.bounds) {
            Fate::Skip
        } else if device.extent() > self.region.split_extent {
            Fate::Split
        } else {
            // A non-finite extent never splits, as in [`ViewportSplit`].
            Fate::Keep
        }
    }

    /// Skips or keeps `span` whole; a piece that still needs a split is kept.
    fn settle(&mut self, span: &Span, fate: Fate) {
        match fate {
            Fate::Skip => self.skip(span),
            Fate::Keep | Fate::Split => self.keep(span),
        }
    }

    /// Splits one element against the region and keeps or skips each piece,
    /// in curve order. A curve is halved on a fixed-capacity stack; a piece
    /// that would still need a split at [`MAX_SPLIT_DEPTH`] is replaced by its
    /// chord, as [`ViewportSplit`] replaces it and to the accuracy the module
    /// docs derive. A line, chord or not, goes to [`Self::line`].
    fn segment(&mut self, span: Span) {
        if span.line {
            self.line(span.piece.pts[0], span.piece.end());
            return;
        }
        let mut stack = [Span::EMPTY; STACK_CAPACITY];
        let mut len = 0;
        let mut next = Some(span);
        while let Some(span) = next.take() {
            let fate = self.fate(&span);
            if !matches!(fate, Fate::Split) {
                self.settle(&span, fate);
            } else if span.piece.depth < MAX_SPLIT_DEPTH && len + 1 < STACK_CAPACITY {
                let (first, second) = span.halve();
                if let Some(slot) = stack.get_mut(len) {
                    *slot = second;
                    len += 1;
                }
                next = Some(first);
                continue;
            } else {
                self.line(span.piece.pts[0], span.piece.end());
            }
            if len > 0 {
                len -= 1;
                next = stack.get(len).copied();
            }
        }
    }

    /// Splits a line against the region with no stack and no depth cap short
    /// of [`LINE_TRIM_STEPS`], so a line of any length keeps only the stretch
    /// near the region. A straight piece neither of whose halves lies wholly
    /// beyond one edge has its midpoint inside the region, and from a point
    /// inside it the kept stretch grows towards each end by bisection.
    fn line(&mut self, p0: Point, p1: Point) {
        let (mut a, mut b) = (p0, p1);
        let mut suffix = 0.0;
        for _ in 0..LINE_TRIM_STEPS {
            let span = Span::line(a, b);
            let fate = self.fate(&span);
            if !matches!(fate, Fate::Split) {
                self.settle(&span, fate);
                self.skip_length(suffix);
                return;
            }
            let m = a.midpoint(b);
            let (left, right) = (Span::line(a, m), Span::line(m, b));
            if matches!(self.fate(&left), Fate::Skip) {
                self.skip(&left);
                a = m;
            } else if matches!(self.fate(&right), Fate::Skip) {
                suffix += right.arclen();
                b = m;
            } else {
                self.trim_towards(a, m);
                self.trim_from(m, b);
                self.skip_length(suffix);
                return;
            }
        }
        self.keep(&Span::line(a, b));
        self.skip_length(suffix);
    }

    /// Keeps or skips the line `a`–`end`, `end` inside the region: pieces
    /// wholly beyond an edge are skipped from `a` on, the rest is kept.
    fn trim_towards(&mut self, a: Point, end: Point) {
        let (mut a, mut b) = (a, end);
        let mut fate = Fate::Keep;
        for _ in 0..LINE_TRIM_STEPS {
            fate = self.fate(&Span::line(a, b));
            if !matches!(fate, Fate::Split) {
                break;
            }
            let q = a.midpoint(b);
            let head = Span::line(a, q);
            if matches!(self.fate(&head), Fate::Skip) {
                self.skip(&head);
                a = q;
            } else {
                b = q;
            }
        }
        self.settle(&Span::line(a, b), fate);
        if b != end {
            self.keep(&Span::line(b, end));
        }
    }

    /// Keeps or skips the line `start`–`b`, `start` inside the region: the
    /// stretch from `start` is kept, pieces wholly beyond an edge towards `b`
    /// are skipped.
    fn trim_from(&mut self, start: Point, b: Point) {
        let (mut a, mut b) = (start, b);
        let mut suffix = 0.0;
        let mut fate = Fate::Keep;
        for _ in 0..LINE_TRIM_STEPS {
            fate = self.fate(&Span::line(a, b));
            if !matches!(fate, Fate::Split) {
                break;
            }
            let q = a.midpoint(b);
            let tail = Span::line(q, b);
            if matches!(self.fate(&tail), Fate::Skip) {
                suffix += tail.arclen();
                b = q;
            } else {
                a = q;
            }
        }
        if a != start {
            self.keep(&Span::line(start, a));
        }
        self.settle(&Span::line(a, b), fate);
        self.skip_length(suffix);
    }

    fn keep(&mut self, span: &Span) {
        if self.run.elements().is_empty() {
            self.run.move_to(span.piece.pts[0]);
            self.run_offset = self.travelled;
        }
        self.run.push(span.to_el());
        let dashes = span.polygon_length() / self.period;
        self.over_budget |= dashes.is_nan() || dashes > MAX_DASHES_PER_ELEMENT;
        self.travelled += span.arclen();
    }

    fn skip(&mut self, span: &Span) {
        self.skip_length(span.arclen());
    }

    /// Skips `length` of arc; nothing when it is zero.
    fn skip_length(&mut self, length: f64) {
        if length == 0.0 {
            return;
        }
        self.flush_run(false);
        self.skipped = true;
        self.travelled += length;
    }

    /// Ends the current subpath; `closed` when it ended in a `ClosePath`.
    fn end_subpath(&mut self, closed: bool) {
        self.flush_run(closed && !self.skipped);
        self.travelled = 0.0;
        self.skipped = false;
    }

    /// Emits the gathered run, dashed from its offset or solid past the caps,
    /// closed when it is a whole closed subpath.
    fn flush_run(&mut self, close: bool) {
        if self.run.elements().is_empty() {
            return;
        }
        if close {
            self.run.close_path();
        }
        if self.sub_pixel || self.over_budget {
            self.out.extend(self.run.iter());
        } else {
            let offset = if self.run_offset.is_finite() {
                self.phase.rem_euclid(self.period) + self.run_offset.rem_euclid(self.period)
            } else {
                self.phase
            };
            self.out
                .extend(kurbo::dash(self.run.iter(), offset, &self.pattern));
        }
        self.run.truncate(0);
        self.over_budget = false;
    }
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

    use frust_scene::DashPattern;
    use kurbo::{
        BezPath, Circle, CubicBez, ParamCurve, ParamCurveNearest, PathSeg, QuadBez,
        RoundedRectRadii, Vec2,
    };
    use vello_common::fearless_simd::Level;
    use vello_common::strip_generator::GenerationMode;
    use vello_common::tile::Tile;

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

    /// Asserts that `outline`, a device-space stroke outline, covers each of
    /// `samples` exactly when the true stroke does — when its `offset` from the
    /// centerline is under `half_width` — skipping a sample within
    /// [`BAND_MARGIN`] of the band's edge.
    fn assert_band(
        case: &str,
        outline: &BezPath,
        samples: impl IntoIterator<Item = Point>,
        offset: impl Fn(Point) -> f64,
        half_width: f64,
    ) {
        for p in samples {
            let distance = offset(p);
            if (distance - half_width).abs() < BAND_MARGIN {
                continue;
            }
            let inside = far_safe_winding(outline, p) != 0;
            assert_eq!(inside, distance < half_width, "{case}, at {p:?}");
        }
    }

    /// How far inside or outside a stroke band a sampled point must lie to be
    /// checked: the 0.25-pixel outline bound plus a margin for the sample.
    const BAND_MARGIN: f64 = 0.3;

    /// The traced anisotropic counterexample: under a `1e10 × 1` scale, a
    /// cubic `1e4` units tall and a few billionths wide, whose device image is
    /// an arch 30 pixels deep, carries a stroke `2e-10` units wide — one pixel
    /// of device reach either side. A floor measured in user space is a few
    /// billionths of a unit across a kept piece thousands of units tall, which
    /// the `1e10` stretch makes tens of pixels and kurbo's collinearity test
    /// turns into the piece's chord, pixels off the arch; measured in device
    /// space the band stays where the arch is, wherever the viewport sits
    /// along it.
    #[test]
    fn an_anisotropic_stroke_keeps_its_band_within_the_device_bound() {
        let viewport = Rect::new(0.0, 0.0, 1024.0, 1024.0);
        let region = SplitRegion::around(viewport);
        let mut arch = BezPath::new();
        arch.move_to((0.0, -5000.0));
        arch.curve_to((4e-9, -5000.0), (4e-9, 5000.0), (0.0, 5000.0));
        for shift in [512.0, 2000.0, 3500.0] {
            let transform =
                Affine::translate((500.0, shift)) * Affine::scale_non_uniform(1e10, 1.0);
            let device = CubicBez::new(
                transform * Point::new(0.0, -5000.0),
                transform * Point::new(4e-9, -5000.0),
                transform * Point::new(4e-9, 5000.0),
                transform * Point::new(0.0, 5000.0),
            );
            // The arch is monotonic in y, so a row's centerline point is the
            // one bisection on y finds; across the viewport the arch moves
            // under a hundredth of a pixel sideways per row, so its band spans
            // one pixel either side of that point, well inside the margin.
            let centerline_x = |y: f64| {
                let (mut t0, mut t1) = (0.0, 1.0);
                for _ in 0..80 {
                    let t = 0.5 * (t0 + t1);
                    if device.eval(t).y < y {
                        t0 = t;
                    } else {
                        t1 = t;
                    }
                }
                device.eval(0.5 * (t0 + t1)).x
            };
            let samples: Vec<Point> = (0..64)
                .flat_map(|row| {
                    let y = f64::from(row) * 16.0 + 3.7;
                    let x = centerline_x(y);
                    (0..=48).map(move |step| Point::new(x - 6.0 + f64::from(step) * 0.25, y))
                })
                .collect();
            for (style, base) in stroke_styles() {
                let stroke = Stroke {
                    width: 2e-10,
                    ..base.clone()
                };
                assert!(
                    !fits_the_stroker(arch.iter(), &stroke, transform, region),
                    "{style} must take the oversized branch"
                );
                let mut expanded = StrokeCtx::default();
                expand_oversized_stroke(arch.iter(), &stroke, transform, region, &mut expanded);
                let outline = transform * expanded.output().clone();
                assert_band(
                    &format!("{style} stroke of the 1e10 × 1 arch shifted {shift}"),
                    &outline,
                    samples.iter().copied(),
                    |p| (p.x - centerline_x(p.y)).abs(),
                    1.0,
                );
            }
        }
    }

    /// A thin stroke along a huge parabola turned 45 degrees keeps its band:
    /// the transform's coefficients are each `1/√2` of its stretch, so a
    /// tolerance derived from them alone would exceed the 0.25-pixel bound.
    #[test]
    fn a_rotated_thin_stroke_of_a_huge_curve_keeps_its_band_inside_the_viewport() {
        let region = stroke_region();
        let mut parabola = BezPath::new();
        parabola.move_to((-1.0, 1.0));
        parabola.quad_to((0.0, -1.0), (1.0, 1.0));
        let turn = std::f64::consts::FRAC_PI_4;
        let direction = Vec2::new(turn.cos(), turn.sin());
        for scale in [1e9, 1e20, 1e25] {
            let transform =
                Affine::translate((32.0, 32.0)) * Affine::rotate(turn) * Affine::scale(scale);
            let half_width = 1.5;
            for (style, base) in stroke_styles() {
                let stroke = Stroke {
                    width: 2.0 * half_width / scale,
                    ..base.clone()
                };
                assert!(!fits_the_stroker(
                    parabola.iter(),
                    &stroke,
                    transform,
                    region
                ));
                let mut expanded = StrokeCtx::default();
                expand_oversized_stroke(parabola.iter(), &stroke, transform, region, &mut expanded);
                let outline = transform * expanded.output().clone();
                let samples = (0..=64).flat_map(|ix| {
                    (0..=64).map(move |iy| {
                        Point::new(f64::from(ix) + 0.37, f64::from(iy) * 0.98 + 0.11)
                    })
                });
                assert_band(
                    &format!("{style} stroke at scale {scale:e}, turned 45 degrees"),
                    &outline,
                    samples,
                    |p| direction.cross(p - Point::new(32.0, 32.0)).abs(),
                    half_width,
                );
            }
        }
    }

    /// Oversized strokes under strongly anisotropic and sheared transforms
    /// expand to a bounded outline in bounded time, at magnitudes up to 1e300:
    /// the tolerance follows the device size, so a stretch of `1e10` along one
    /// axis and `1e-10` along the other leaves the expansion bounded.
    #[test]
    fn oversized_strokes_under_anisotropic_transforms_expand_at_bounded_cost() {
        let region = stroke_region();
        let mut generator =
            StripGenerator::new(STROKE_VIEWPORT.0, STROKE_VIEWPORT.1, Level::baseline());
        let transforms = [
            Affine::scale_non_uniform(1e10, 1.0),
            Affine::scale_non_uniform(1.0, 1e-10),
            Affine::rotate(0.3) * Affine::scale_non_uniform(1e10, 1e-10),
            Affine::new([1.0, 0.0, 1e10, 1.0, 0.0, 0.0]),
        ];
        for size in [1e20, 1e100, 1e300] {
            for transform in transforms {
                for (style, base) in stroke_styles() {
                    for width in [2.0, size] {
                        let stroke = Stroke {
                            width,
                            ..base.clone()
                        };
                        for (shape, path) in stroke_centerlines(size) {
                            let case = format!(
                                "{style} {shape} at {size:e}, width {width:e}, under {transform:?}"
                            );
                            let started = Instant::now();
                            let mut expanded = StrokeCtx::default();
                            expand_oversized_stroke(
                                path.iter(),
                                &stroke,
                                transform,
                                region,
                                &mut expanded,
                            );
                            let outline = expanded.output().elements().len();
                            let mut storage = StripStorage::new(GenerationMode::Append);
                            generate_stroke(
                                &mut generator,
                                path.iter(),
                                &stroke,
                                transform,
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
    }

    /// The stroker keeps every stroke it handled before the user-space
    /// ceiling — a 4000-unit path zoomed out to a quarter on a large viewport,
    /// a viewport-sized one at a device scale of three — and gives up one whose
    /// user-space extent is past [`STROKER_EXTENT_CEILING`] of its tolerance
    /// however small it is on screen, the ceiling itself pinned on either side.
    #[test]
    fn the_stroker_keeps_ordinary_strokes_and_gives_up_huge_user_space_ones() {
        let large = SplitRegion::around(Rect::new(0.0, 0.0, 1920.0, 1080.0));
        let mut zoomed_out = BezPath::new();
        zoomed_out.move_to((0.0, 0.0));
        zoomed_out.curve_to((4000.0, 0.0), (0.0, 4000.0), (4000.0, 4000.0));
        zoomed_out.line_to((0.0, 4000.0));
        assert!(fits_the_stroker(
            zoomed_out.iter(),
            &round(4.0),
            Affine::scale(0.25),
            large
        ));
        let region = stroke_region();
        let mut on_screen = BezPath::new();
        on_screen.move_to((1.0, 1.0));
        on_screen.quad_to((20.0, 0.0), (20.0, 20.0));
        assert!(fits_the_stroker(
            on_screen.iter(),
            &round(1.0),
            Affine::scale(3.0),
            region
        ));

        let huge = 1e100;
        let tiny = Affine::scale(1.0 / huge);
        for (_, path) in stroke_centerlines(huge) {
            assert!(stroke_device_extent(path.iter(), &round(huge), tiny) <= region.split_extent);
            assert!(!fits_the_stroker(path.iter(), &round(huge), tiny, region));
        }

        // Under the identity the stroker's tolerance is 0.25 units, so the
        // ceiling sits at a user-space extent of 2^30 units: a line just
        // inside it keeps the stroker under a scale that leaves it a
        // thousandth of a pixel on screen, one just past it does not.
        let ceiling = STROKER_EXTENT_CEILING * STROKE_TOLERANCE;
        let shrink = Affine::scale(1e-12);
        for (length, fits) in [(0.999 * ceiling, true), (1.001 * ceiling, false)] {
            let mut line = BezPath::new();
            line.move_to((0.0, 0.0));
            line.line_to((length, 0.0));
            assert_eq!(
                fits_the_stroker(line.iter(), &Stroke::new(0.0), shrink, region),
                fits,
                "a {length:e}-unit line"
            );
        }
    }

    /// The coverage `storage` leaves on each pixel of the stroke viewport,
    /// row-major, read from its strips: a strip's alphas are stored a column
    /// of [`Tile::HEIGHT`] rows at a time, and a strip flagged `fill_gap`
    /// covers the whole gap back to the previous strip of its row.
    fn coverage(storage: &StripStorage) -> Vec<u8> {
        let width = usize::from(STROKE_VIEWPORT.0);
        let height = usize::from(STROKE_VIEWPORT.1);
        let rows = usize::from(Tile::HEIGHT);
        let mut pixels = vec![0_u8; width * height];
        let mut set = |x: usize, y: usize, alpha: u8| {
            if x < width && y < height {
                let pixel = &mut pixels[y * width + x];
                *pixel = (*pixel).max(alpha);
            }
        };
        let mut previous_end: Option<(u16, u16)> = None;
        for pair in storage.strips.windows(2) {
            let (strip, next) = (&pair[0], &pair[1]);
            if strip.is_sentinel() {
                previous_end = None;
                continue;
            }
            if let Some((_, end)) = previous_end.filter(|&(y, _)| y == strip.y && strip.fill_gap())
            {
                for x in end..strip.x {
                    for row in 0..rows {
                        set(usize::from(x), usize::from(strip.y) + row, 255);
                    }
                }
            }
            let strip_width = strip.width_to(next);
            for column in 0..usize::from(strip_width) {
                for row in 0..rows {
                    let index = strip.alpha_idx() as usize + column * rows + row;
                    if let Some(&alpha) = storage.alphas.get(index) {
                        set(
                            usize::from(strip.x) + column,
                            usize::from(strip.y) + row,
                            alpha,
                        );
                    }
                }
            }
            previous_end = Some((strip.y, strip.x + strip_width));
        }
        pixels
    }

    /// Largest per-pixel coverage difference, out of 255, the oversized branch
    /// may show against `vello_common`'s stroker on a stroke both can draw.
    const PARITY_PIXEL_TOLERANCE: u8 = 4;

    /// Largest total coverage difference, in pixels, the oversized branch may
    /// show against `vello_common`'s stroker on a stroke both can draw.
    const PARITY_AREA_TOLERANCE: f64 = 0.5;

    /// Moderate round-joined, round-capped strokes — a polyline, a curve and
    /// a self-intersecting loop, at three widths — each drawn by
    /// `vello_common`'s stroker and, forced, by the oversized branch at a
    /// `base` device tolerance, under the identity, a uniform scale of three
    /// and a 30-degree rotation. Yields the case name and whether the two
    /// coverages agree within the parity tolerances.
    fn stroker_parity(base: f64) -> Vec<(String, bool)> {
        let mut polyline = BezPath::new();
        polyline.move_to((6.0, 50.0));
        polyline.line_to((20.0, 8.0));
        polyline.line_to((34.0, 44.0));
        polyline.line_to((56.0, 12.0));
        let mut curve = BezPath::new();
        curve.move_to((8.0, 40.0));
        curve.curve_to((8.0, -10.0), (60.0, 70.0), (56.0, 14.0));
        curve.quad_to((40.0, 60.0), (20.0, 20.0));
        let mut ring = BezPath::new();
        ring.move_to((10.0, 32.0));
        ring.curve_to((70.0, -20.0), (70.0, 84.0), (10.0, 32.0));
        let centre = Affine::translate((32.0, 32.0));
        let transforms = [
            ("the identity", Affine::IDENTITY),
            (
                "a scale of 3",
                centre * Affine::scale(3.0) * centre.inverse(),
            ),
            (
                "a 30-degree rotation",
                centre * Affine::rotate(30_f64.to_radians()) * centre.inverse(),
            ),
        ];
        let region = stroke_region();
        let mut cases = Vec::new();
        for (name, transform) in transforms {
            for (shape, path) in [("polyline", &polyline), ("curve", &curve), ("loop", &ring)] {
                for width in [1.0, 3.0, 8.0] {
                    let stroke = round(width);
                    let stroker = coverage(&strips_of(|generator, storage| {
                        generator.generate_stroked_path(
                            path.iter(),
                            &stroke,
                            transform,
                            None,
                            storage,
                            None,
                        );
                    }));
                    let oversized = coverage(&strips_of(|generator, storage| {
                        let mut expanded = StrokeCtx::default();
                        expand_stroke_at(
                            path.iter(),
                            &stroke,
                            transform,
                            region,
                            base,
                            &mut expanded,
                        );
                        generate_fill(
                            generator,
                            expanded.output().iter(),
                            transform,
                            storage,
                            None,
                        );
                    }));
                    let pixel = stroker
                        .iter()
                        .zip(&oversized)
                        .map(|(a, b)| a.abs_diff(*b))
                        .max()
                        .unwrap_or(0);
                    let area = stroker
                        .iter()
                        .zip(&oversized)
                        .map(|(a, b)| f64::from(a.abs_diff(*b)))
                        .sum::<f64>()
                        / 255.0;
                    cases.push((
                        format!("{shape} {width} wide under {name}: {pixel}/255, {area:.3} px"),
                        pixel <= PARITY_PIXEL_TOLERANCE && area <= PARITY_AREA_TOLERANCE,
                    ));
                }
            }
        }
        cases
    }

    /// [`STROKE_TOLERANCE`] and the oversized branch's use of it copy a
    /// private `vello_common` rule. On every stroke both can draw, the branch
    /// forced at that tolerance covers what `vello_common`'s stroker covers,
    /// within [`PARITY_PIXEL_TOLERANCE`] per pixel and
    /// [`PARITY_AREA_TOLERANCE`] in total.
    #[test]
    fn the_oversized_branch_matches_the_stroker_on_moderate_strokes() {
        for (case, agrees) in stroker_parity(STROKE_TOLERANCE) {
            assert!(agrees, "{case}");
        }
    }

    /// The negative control for the parity test: four times the copied
    /// tolerance draws curves that disagree with `vello_common`'s stroker past
    /// the parity tolerances under each transform, so the test would catch
    /// that copy drifting.
    #[test]
    fn the_stroker_parity_test_fails_for_four_times_the_tolerance() {
        let cases = stroker_parity(4.0 * STROKE_TOLERANCE);
        for transform in ["the identity", "a scale of 3", "a 30-degree rotation"] {
            assert!(
                cases
                    .iter()
                    .any(|(case, agrees)| case.contains(transform) && !agrees),
                "nothing under {transform} disagrees: {cases:?}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Dashed strokes
    // -----------------------------------------------------------------------

    fn round(width: f64) -> Stroke {
        Stroke::new(width)
            .with_join(Join::Round)
            .with_caps(Cap::Round)
    }

    /// Each subpath of `path` as its start and end point.
    fn subpath_ends(path: &BezPath) -> Vec<(Point, Point)> {
        let mut ends = Vec::new();
        let mut start = None;
        let mut current = Point::ZERO;
        for el in path.iter() {
            match el {
                PathEl::MoveTo(p) => {
                    if let Some(s) = start.replace(p) {
                        ends.push((s, current));
                    }
                    current = p;
                }
                PathEl::ClosePath => current = start.unwrap_or(current),
                other => current = other.end_point().unwrap_or(current),
            }
        }
        if let Some(s) = start {
            ends.push((s, current));
        }
        ends
    }

    fn inside(rect: Rect, p: Point) -> bool {
        p.x > rect.x0 && p.x < rect.x1 && p.y > rect.y0 && p.y < rect.y1
    }

    fn strips_of(generate: impl FnOnce(&mut StripGenerator, &mut StripStorage)) -> StripStorage {
        let mut generator =
            StripGenerator::new(STROKE_VIEWPORT.0, STROKE_VIEWPORT.1, Level::baseline());
        let mut storage = StripStorage::new(GenerationMode::Append);
        generate(&mut generator, &mut storage);
        storage
    }

    /// Dashed strokes that fit the split extent — a polyline, a curve, a
    /// closed shape, under the identity, a scale and a rotation — lower to
    /// exactly `dash_path`'s elements and generate exactly the strips the
    /// whole-path lowering does.
    #[test]
    fn the_gate_keeps_ordinary_dashed_strokes_bit_identical() {
        let region = stroke_region();
        let mut poly = BezPath::new();
        poly.move_to((2.0, 3.0));
        poly.line_to((60.0, 10.0));
        poly.line_to((20.0, 61.5));
        let mut curve = BezPath::new();
        curve.move_to((-10.0, 40.0));
        curve.curve_to((10.0, -30.0), (50.0, 90.0), (70.0, 20.0));
        curve.quad_to((30.0, 30.0), (5.0, 5.0));
        let ring = Circle::new((32.0, 32.0), 20.0).to_path(0.1);
        let transforms = [
            Affine::IDENTITY,
            Affine::scale(1.75),
            Affine::translate((32.0, 32.0))
                * Affine::rotate(0.4)
                * Affine::translate((-32.0, -32.0)),
        ];
        let patterns = [
            DashPattern::new(4.0, 2.0),
            DashPattern::new(1.0, 7.0).with_phase(-3.25),
            DashPattern::new(13.0, 0.5).with_phase(40.0),
        ];
        for path in [&poly, &curve, &ring] {
            for transform in transforms {
                for dash in patterns {
                    let stroke = round(2.0);
                    assert!(
                        stroke_device_extent(path.iter(), &stroke, transform)
                            <= region.split_extent
                    );
                    let expected = dash_path(path, dash);
                    let actual = dash_within(path, dash, &stroke, transform, region);
                    assert_eq!(actual.elements().len(), expected.elements().len());
                    for (a, e) in actual.iter().zip(expected.iter()) {
                        assert!(same_kind(a, e), "{a:?} vs {e:?}");
                        assert_eq!(el_bits(a), el_bits(e), "{a:?} vs {e:?}");
                    }

                    let bounded = strips_of(|generator, storage| {
                        generate_dashed_stroke(
                            generator, path, dash, &stroke, transform, storage, None,
                        );
                    });
                    let whole = strips_of(|generator, storage| {
                        generate_stroke(
                            generator,
                            expected.iter(),
                            &stroke,
                            transform,
                            storage,
                            None,
                        );
                    });
                    assert_eq!(bounded.strips, whole.strips, "{dash:?} under {transform:?}");
                    assert_eq!(bounded.alphas, whole.alphas, "{dash:?} under {transform:?}");
                }
            }
        }
    }

    /// Largest distance between a dash end inside the viewport and the same
    /// end in the whole-path lowering, as a fraction of the path's length: the
    /// phase a run starts at is carried across the skipped length, exact for a
    /// line up to rounding and within the relative arc-length accuracy for a
    /// curve, which for a smooth one is far tighter than its bound.
    const PHASE_TOLERANCE: f64 = 1e-8;

    /// A dashed stroke crossing the viewport edge from far outside keeps,
    /// inside the viewport, exactly the dashes the whole-path lowering puts
    /// there: the same number, each end within [`PHASE_TOLERANCE`].
    #[test]
    fn dashes_inside_the_viewport_match_the_whole_path_lowering() {
        let region = stroke_region();
        let viewport = Rect::new(
            0.0,
            0.0,
            f64::from(STROKE_VIEWPORT.0),
            f64::from(STROKE_VIEWPORT.1),
        );
        let mut line = BezPath::new();
        line.move_to((-20_000.0, 30.5));
        line.line_to((20_000.0, 34.5));
        let mut corner = BezPath::new();
        corner.move_to((-9_000.0, -9_000.0));
        corner.line_to((32.0, 32.0));
        corner.line_to((9_000.0, 4.0));
        // An arc of radius 4000 whose top crosses the viewport's middle row,
        // entered from far below the viewport on both sides.
        let arc = Affine::translate((32.0, 4032.0)) * Circle::new((0.0, 0.0), 4000.0).to_path(1e-3);
        let patterns = [
            DashPattern::new(6.0, 4.0).with_phase(1.5),
            DashPattern::new(1.0, 2.0).with_phase(-17.0),
            DashPattern::new(3.0, 9.5),
        ];
        for (name, path) in [("line", &line), ("corner", &corner), ("arc", &arc)] {
            for dash in patterns {
                let stroke = round(2.0);
                assert!(
                    stroke_device_extent(path.iter(), &stroke, Affine::IDENTITY)
                        > region.split_extent,
                    "{name} must take the bounded lowering"
                );
                let whole = subpath_ends(&dash_path(path, dash));
                let bounded_path = dash_within(path, dash, &stroke, Affine::IDENTITY, region);
                let bounded = subpath_ends(&bounded_path);
                assert!(
                    bounded.len() * 20 < whole.len(),
                    "{name}: {} dashes kept of {}",
                    bounded.len(),
                    whole.len()
                );
                let visible = |ends: &[(Point, Point)]| -> Vec<(Point, Point)> {
                    ends.iter()
                        .copied()
                        .filter(|&(a, b)| inside(viewport, a) && inside(viewport, b))
                        .collect()
                };
                let (whole, bounded) = (visible(&whole), visible(&bounded));
                assert!(
                    !whole.is_empty(),
                    "{name}: no dash ends inside the viewport"
                );
                assert_eq!(whole.len(), bounded.len(), "{name} {dash:?}");
                let tolerance = PHASE_TOLERANCE * path.perimeter(1e-3);
                for (w, b) in whole.iter().zip(&bounded) {
                    let error = (w.0 - b.0).hypot().max((w.1 - b.1).hypot());
                    assert!(
                        error <= tolerance,
                        "{name} {dash:?}: dash {w:?} kept as {b:?}, {error:e} apart"
                    );
                }
            }
        }
    }

    /// A closed subpath the clip skips nothing of — every piece within reach
    /// of a stroke as wide as the shape — is one closed run, dashed exactly as
    /// `dash_path` dashes it.
    #[test]
    fn a_wholly_kept_closed_subpath_dashes_as_the_whole_path_lowering() {
        let region = stroke_region();
        let ring = Circle::new((32.0, 32.0), 300.0).to_path(0.1);
        let stroke = round(2000.0);
        assert!(stroke_device_extent(ring.iter(), &stroke, Affine::IDENTITY) > region.split_extent);
        for dash in [
            DashPattern::new(40.0, 25.0).with_phase(7.0),
            DashPattern::new(5000.0, 1.0),
        ] {
            let expected = dash_path(&ring, dash);
            let actual = dash_within(&ring, dash, &stroke, Affine::IDENTITY, region);
            assert_eq!(
                actual.elements().len(),
                expected.elements().len(),
                "{dash:?}"
            );
            for (a, e) in actual.iter().zip(expected.iter()) {
                assert!(same_kind(a, e), "{a:?} vs {e:?}");
                assert_eq!(el_bits(a), el_bits(e), "{a:?} vs {e:?}");
            }
        }
    }

    /// A pattern whose device period is under [`MIN_DEVICE_DASH_PERIOD`] —
    /// effective in user space, sub-pixel on screen — strokes the kept run
    /// solid: one subpath, both ends outside the expanded viewport.
    #[test]
    fn a_sub_pixel_device_period_strokes_the_kept_run_solid() {
        let region = stroke_region();
        let mut line = BezPath::new();
        line.move_to((-20_000.0, 32.0));
        line.line_to((20_000.0, 32.0));
        let stroke = round(2.0);
        for (dash, transform) in [
            (DashPattern::new(0.15, 0.15), Affine::IDENTITY),
            (
                DashPattern::new(2.0, 2.0),
                Affine::translate((32.0, 30.0)) * Affine::scale(1.0 / 16.0),
            ),
        ] {
            let device_period = (dash.on + dash.off) * largest_stretch(transform);
            assert!(device_period < MIN_DEVICE_DASH_PERIOD, "{device_period}");
            let dashed = dash_within(&line, dash, &stroke, transform, region);
            let ends = subpath_ends(&dashed);
            assert_eq!(ends.len(), 1, "{dash:?} under {transform:?}: {ends:?}");
            let (a, b) = ends[0];
            assert!(!inside(region.bounds, transform * a), "{a:?}");
            assert!(!inside(region.bounds, transform * b), "{b:?}");
        }
    }

    /// The dash gate gives up a dashed stroke the stroker gives up, however
    /// small it is on screen. Each centerline of `1e100` units, dashed at 4
    /// units and stroked `1e100` wide under a scale of `1e-100`, is 8e-100
    /// pixels a period, so its kept runs strike solid, and it lowers in time:
    /// the whole-path lowering would iterate about `1e100` of its periods. The
    /// ceiling itself is pinned for a dashed line: just inside it the stroke
    /// is dashed whole, just past it by the bounded lowering.
    #[test]
    fn the_dash_gate_gives_up_huge_user_space_strokes() {
        let region = stroke_region();
        let huge = 1e100;
        let tiny = Affine::scale(1.0 / huge);
        let stroke = round(huge);
        let dash = DashPattern::new(4.0, 4.0);
        for (shape, path) in stroke_centerlines(huge) {
            assert!(
                !fits_the_stroker(path.iter(), &stroke, tiny, region),
                "{shape}"
            );
            let started = Instant::now();
            let dashed = dash_within(&path, dash, &stroke, tiny, region);
            let elapsed = started.elapsed();
            assert!(elapsed < STROKE_TIME_LIMIT, "{shape} took {elapsed:?}");
            assert!(dashed.elements().len() <= STROKE_OUTLINE_LIMIT, "{shape}");
            // Each shape is about two pixels across on screen, so its whole
            // centerline is one kept run, and it strokes solid: the lowering
            // is the centerline itself, element for element.
            assert_eq!(dashed.elements().len(), path.elements().len(), "{shape}");
            for (a, e) in dashed.iter().zip(path.iter()) {
                assert!(same_kind(a, e), "{shape}: {a:?} vs {e:?}");
                assert_eq!(el_bits(a), el_bits(e), "{shape}: {a:?} vs {e:?}");
            }
        }

        // Under a scale of one or less the stroker's tolerance is a quarter
        // unit, so its ceiling sits at 2^30 units of centerline. Just inside it
        // a line dashes whole, as `dash_path` lowers it; just past it the
        // bounded lowering takes over. The shrink puts both lines a couple of
        // hundred pixels long on screen, within the split extent, and the
        // dash period is a few pixels and a hundred dashes a line, so the
        // ceiling alone decides.
        let ceiling = STROKER_EXTENT_CEILING * STROKE_TOLERANCE;
        let shrink = Affine::scale(2e-7);
        let dash = DashPattern::new(ceiling / 256.0, ceiling / 256.0);
        let zero = Stroke::new(0.0);
        for (length, whole) in [(0.999 * ceiling, true), (1.001 * ceiling, false)] {
            let mut line = BezPath::new();
            line.move_to((0.0, 0.0));
            line.line_to((length, 0.0));
            // Both lowerings dash a line this short on screen alike, so the
            // ceiling is pinned by the gate's own predicate.
            assert_eq!(
                whole,
                fits_the_stroker(line.iter(), &zero, shrink, region),
                "{length:e}-unit line"
            );
            let expected = if whole {
                dash_path(&line, dash)
            } else {
                dash_visible_stretch(&line, dash, &zero, shrink, region)
            };
            let actual = dash_within(&line, dash, &zero, shrink, region);
            assert_eq!(actual.elements().len(), expected.elements().len());
            for (a, e) in actual.iter().zip(expected.iter()) {
                assert!(same_kind(a, e), "{a:?} vs {e:?}");
                assert_eq!(el_bits(a), el_bits(e), "{a:?} vs {e:?}");
            }
        }
    }

    /// A centerline of any length keeps only the dashes near the viewport. A
    /// line is trimmed with no depth cap and a curve piece the split leaves
    /// oversized at its depth cap is trimmed as its chord, so a 1e300-unit
    /// line or curve dashes a stretch a few split extents long, every dash
    /// end within it.
    #[test]
    fn a_centerline_of_any_length_dashes_only_its_visible_stretch() {
        let region = stroke_region();
        let stroke = round(2.0);
        let reach = region.bounds.inflate(1.0, 1.0);
        let stretch = reach.inflate(region.split_extent, region.split_extent);
        for size in [1e18, 1e100, 1e300] {
            for (shape, path) in stroke_centerlines(size) {
                let started = Instant::now();
                let dashed = dash_within(
                    &path,
                    DashPattern::new(1.0, 1.0),
                    &stroke,
                    Affine::IDENTITY,
                    region,
                );
                assert!(started.elapsed() < STROKE_TIME_LIMIT);
                let ends = subpath_ends(&dashed);
                let case = format!("a {size:e} {shape}");
                assert!(ends.len() > 32, "{case} must dash: {} dashes", ends.len());
                assert!(ends.len() < 2000, "{case} kept {} dashes", ends.len());
                for (a, b) in ends {
                    assert!(
                        inside(stretch, a) && inside(stretch, b),
                        "{case}: {a:?}-{b:?}"
                    );
                }
            }
        }
    }

    /// A kept span longer than [`MAX_DASHES_PER_ELEMENT`] periods strokes solid: a
    /// stroke two million units wide reaches far enough to keep millions of
    /// units of a 1e18-unit line or curve, a span no pixel-sized pattern dashes
    /// in bounded time.
    #[test]
    fn a_run_past_the_dash_ceiling_strokes_solid() {
        let region = stroke_region();
        let stroke = round(2e6);
        for (shape, path) in stroke_centerlines(1e18) {
            let started = Instant::now();
            let dashed = dash_within(
                &path,
                DashPattern::new(1.0, 1.0),
                &stroke,
                Affine::IDENTITY,
                region,
            );
            assert!(started.elapsed() < STROKE_TIME_LIMIT, "{shape}");
            let runs = subpath_ends(&dashed).len();
            assert!((1..=4).contains(&runs), "{shape}: {runs} subpaths");
            assert!(dashed.elements().len() < 2000, "{shape}: {dashed:?}");
        }
    }

    /// The bounded lowering's own cost: lines, curves and closed shapes at
    /// every magnitude to 1e300, thin and as wide as the geometry, with
    /// patterns from a tenth of a unit to one well above the geometry, lower
    /// to a bounded output in bounded time.
    #[test]
    fn dashing_an_oversized_stroke_is_bounded_at_any_magnitude() {
        let region = stroke_region();
        let limit = STROKE_OUTLINE_LIMIT;
        for size in [1e6, 1e18, 1e40, 1e100, 1e300] {
            let mut closed = BezPath::new();
            closed.move_to((-size, 0.0));
            closed.curve_to(
                (size, 3.0 * size),
                (256.0 / 3.0 - size, 64.0 - 3.0 * size),
                (size, 64.0),
            );
            closed.line_to((size, 2.0 * size));
            closed.close_path();
            let mut shapes: Vec<(&str, BezPath)> = stroke_centerlines(size).into_iter().collect();
            shapes.push(("closed", closed));
            for (shape, path) in &shapes {
                for width in [2.0, size] {
                    for period in [0.1, 1.0, 8.0, 1e3] {
                        let dash = DashPattern::new(0.5 * period, 0.5 * period).with_phase(0.3);
                        let stroke = round(width);
                        let started = Instant::now();
                        let dashed = dash_within(path, dash, &stroke, Affine::IDENTITY, region);
                        let elapsed = started.elapsed();
                        let case = format!("{shape} at {size:e}, width {width:e}, period {period}");
                        assert!(elapsed < STROKE_TIME_LIMIT, "{case} took {elapsed:?}");
                        assert!(
                            dashed.elements().len() <= limit,
                            "{case} dashed to {} elements",
                            dashed.elements().len()
                        );
                    }
                }
            }
        }
    }

    /// A fine period over a long line that fits the stroker on screen is
    /// refused by the whole-path gate: its device period is under
    /// [`MIN_DEVICE_DASH_PERIOD`] or an element exceeds the dash budget, so it
    /// strokes solid in bounded time; at the budget's boundary the gate flips.
    #[test]
    fn the_dash_gate_refuses_a_fine_period_over_a_long_on_screen_line() {
        let solid = |dashed: &BezPath, path: &BezPath, case: &str| {
            assert_eq!(dashed.elements().len(), path.elements().len(), "{case}");
            for (a, e) in dashed.iter().zip(path.iter()) {
                assert!(same_kind(a, e), "{case}: {a:?} vs {e:?}");
                assert_eq!(el_bits(a), el_bits(e), "{case}: {a:?} vs {e:?}");
            }
        };
        let line = |length: f64| {
            let mut line = BezPath::new();
            line.move_to((0.0, 0.0));
            line.line_to((length, 0.0));
            line
        };

        // Counterexample A: on screen the line is about 7000 pixels, within
        // the split extent of a 1920x1080 viewport.
        let region = SplitRegion::around(Rect::new(0.0, 0.0, 1920.0, 1080.0));
        let transform = Affine::scale(7e-6);
        let stroke = round(1.0);
        let path = line(1e9);
        assert!(fits_the_stroker(path.iter(), &stroke, transform, region));
        let started = Instant::now();
        let dashed = dash_within(
            &path,
            DashPattern::new(0.05, 0.05),
            &stroke,
            transform,
            region,
        );
        let elapsed = started.elapsed();
        assert!(elapsed < STROKE_TIME_LIMIT, "took {elapsed:?}");
        solid(&dashed, &path, "fine period");

        // Per element: a device period under half a pixel, and over budget.
        let region = stroke_region();
        let short = line(2.0);
        let dash = DashPattern::new(5e-6, 5e-6);
        assert!(fits_the_stroker(
            short.iter(),
            &round(1.0),
            Affine::IDENTITY,
            region
        ));
        let dashed = dash_within(&short, dash, &round(1.0), Affine::IDENTITY, region);
        solid(&dashed, &short, "over budget");

        // The boundary: a device period at the minimum, an element just under
        // and just over the budget, in a region big enough to hold either.
        let region = SplitRegion::around(Rect::new(0.0, 0.0, 20_000.0, 20_000.0));
        let period = MIN_DEVICE_DASH_PERIOD;
        let dash = DashPattern::new(0.5 * period, 0.5 * period);
        let stroke = Stroke::new(0.0);
        for (factor, whole) in [(0.999, true), (1.001, false)] {
            let path = line(factor * MAX_DASHES_PER_ELEMENT * period);
            assert!(fits_the_stroker(
                path.iter(),
                &stroke,
                Affine::IDENTITY,
                region
            ));
            let actual = dash_within(&path, dash, &stroke, Affine::IDENTITY, region);
            if whole {
                let expected = dash_path(&path, dash);
                assert_eq!(actual.elements().len(), expected.elements().len());
                for (a, e) in actual.iter().zip(expected.iter()) {
                    assert_eq!(el_bits(a), el_bits(e), "{a:?} vs {e:?}");
                }
            } else {
                solid(&actual, &path, "just over budget");
            }
        }
    }

    /// A dense zig-zag whose end lines reach far off screen keeps its dashes:
    /// the dash budget is per element, so the many on-screen segments do not
    /// add up to a solid run.
    #[test]
    fn a_dense_on_screen_polyline_keeps_its_dashes_past_the_viewport() {
        let region = SplitRegion::around(Rect::new(0.0, 0.0, 1920.0, 1080.0));
        let mut path = BezPath::new();
        path.move_to((-10_000.0, 400.0));
        for i in 0..2000 {
            let y = if i % 2 == 0 { 100.0 } else { 700.0 };
            path.line_to((f64::from(i) * 0.5, y));
        }
        path.line_to((20_000.0, 400.0));
        let stroke = round(2.0);
        let dash = DashPattern::new(4.0, 4.0);
        assert!(stroke_device_extent(path.iter(), &stroke, Affine::IDENTITY) > region.split_extent);
        let started = Instant::now();
        let dashed = dash_within(&path, dash, &stroke, Affine::IDENTITY, region);
        assert!(started.elapsed() < STROKE_TIME_LIMIT * 4);
        let ends = subpath_ends(&dashed);
        assert!(ends.len() > 10_000, "{} subpaths", ends.len());
        assert!(ends.len() < 200_000, "{} subpaths", ends.len());
        let stretch = region
            .bounds
            .inflate(region.split_extent, region.split_extent)
            .inflate(2.0, 2.0);
        for (a, b) in ends {
            assert!(inside(stretch, a) && inside(stretch, b), "{a:?}-{b:?}");
        }
    }

    #[test]
    fn largest_stretch_is_the_largest_singular_value() {
        assert_eq!(largest_stretch(Affine::IDENTITY), 1.0);
        assert!((largest_stretch(Affine::scale_non_uniform(3.0, -0.5)) - 3.0).abs() < 1e-12);
        assert!((largest_stretch(Affine::rotate(0.7) * Affine::scale(2.5)) - 2.5).abs() < 1e-12);
        // A shear [[1, 1], [0, 1]] stretches by the golden ratio.
        let shear = Affine::new([1.0, 0.0, 1.0, 1.0, 0.0, 0.0]);
        assert!((largest_stretch(shear) - (1.0 + 5.0_f64.sqrt()) / 2.0).abs() < 1e-12);
        assert_eq!(largest_stretch(Affine::scale(0.0)), 0.0);
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
