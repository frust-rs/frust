//! Chart geometry as pure functions, shared by the widgets: gap-aware line and area paths
//! (a gap breaks a line, an isolated sample becomes a mark), block-column layout, and
//! whole-pixel snapping. Nothing here paints or holds state.
//!
//! Coordinates are widget-local logical pixels, y down. Slots run oldest to newest and are
//! right-aligned: the newest sits at the plot's right edge, so a series shorter than its
//! capacity starts partway across. Only the newest `capacity` slots are drawn. A `None` or
//! non-finite slot is a gap. Timed points (`(t, value)`) are placed by time instead, over a
//! [`time_span`] whose end is the newest point, at the right edge; points further apart than
//! a threshold are a gap. Either way the line and area builders return the same shapes; block
//! columns are offered for slots only. Values are clamped to the `(min, max)` domain, which is
//! normalised the way [`YDomain::resolve`] normalises a fixed range (reversed is swapped,
//! degenerate is widened). A plot with no positive, finite width and height yields empty
//! geometry; nothing here panics or returns a non-finite coordinate.
//!
//! Snapping rounds to whole *logical* pixels, because a paint has no device scale to reach
//! (the QR widget floors its module size to logical pixels for the same reason). Block-column
//! edges, widths and baselines are snapped, as are rule positions; line and area vertices are
//! not, since they are antialiased curves.

use frust::Color;
use frust::authoring::{BezPath, Point, Rect};

use crate::{Tone, YDomain};

/// Which way values grow inside a plot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Orientation {
    /// The domain minimum sits on the bottom edge and values grow upward.
    #[default]
    Upright,
    /// The domain minimum sits on the top edge and values grow downward: the lower half of a
    /// mirrored pair.
    Inverted,
}

impl Orientation {
    /// The y of `value` in `plot`, clamped to `domain`. A non-finite value maps to the
    /// baseline.
    pub fn y_for(self, value: f32, domain: (f32, f32), plot: Rect) -> f64 {
        YMap::new(domain, plot, self).y(value)
    }

    /// The edge the domain minimum sits on: `plot.y1` upright, `plot.y0` inverted.
    pub fn baseline(self, plot: Rect) -> f64 {
        let y = match self {
            Self::Upright => plot.y1,
            Self::Inverted => plot.y0,
        };
        finite_or_zero(y)
    }
}

/// The y of `value` with the domain minimum at `plot.y1` and the maximum at `plot.y0`.
pub fn y_for(value: f32, domain: (f32, f32), plot: Rect) -> f64 {
    Orientation::Upright.y_for(value, domain, plot)
}

/// The y of `value` with the domain minimum at `plot.y0`, growing downward to `plot.y1`.
pub fn y_for_inverted(value: f32, domain: (f32, f32), plot: Rect) -> f64 {
    Orientation::Inverted.y_for(value, domain, plot)
}

/// The x distance between neighbouring slots: `plot.width() / (capacity - 1)`, and 0 for a
/// capacity of 0 or 1 or an unusable plot.
pub fn slot_spacing(capacity: usize, plot: Rect) -> f64 {
    if capacity <= 1 || !usable(plot) {
        return 0.0;
    }
    plot.width() / (capacity - 1) as f64
}

/// The x of the slot `age` steps older than the newest (0 is the newest, at `plot.x1`).
pub fn x_for_age(age: usize, capacity: usize, plot: Rect) -> f64 {
    finite_or_zero(plot.x1 - age as f64 * slot_spacing(capacity, plot))
}

/// The x of slot `index` (0 the oldest) of `len` slots, right-aligned.
pub fn x_for_slot(index: usize, len: usize, capacity: usize, plot: Rect) -> f64 {
    x_for_age(len.saturating_sub(1).saturating_sub(index), capacity, plot)
}

/// A line chart's shapes: open polylines over runs of two or more consecutive samples, and
/// the isolated samples a gap leaves alone on both sides, to be drawn as dots.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LineGeometry {
    /// One vertex list per run of two or more samples, oldest run first, each oldest to
    /// newest. No polyline crosses a gap.
    pub polylines: Vec<Vec<Point>>,
    /// The lone samples, oldest first.
    pub points: Vec<Point>,
}

impl LineGeometry {
    /// Whether there is nothing to draw.
    pub fn is_empty(&self) -> bool {
        self.polylines.is_empty() && self.points.is_empty()
    }

    /// Each polyline as a path, ready to stroke.
    pub fn paths(&self) -> Vec<BezPath> {
        self.polylines.iter().map(|run| polyline(run)).collect()
    }

    /// Each polyline as a monotone smooth path (see [`smooth_polyline`]), ready to stroke.
    pub fn smooth_paths(&self) -> Vec<BezPath> {
        self.polylines
            .iter()
            .map(|run| smooth_polyline(run))
            .collect()
    }
}

/// An open path through `points` with straight segments; empty for no points.
pub fn polyline(points: &[Point]) -> BezPath {
    let mut path = BezPath::new();
    let mut points = points.iter();
    if let Some(&first) = points.next() {
        path.move_to(first);
        for &p in points {
            path.line_to(p);
        }
    }
    path
}

/// An open path through `points` that is a monotone cubic interpolation: it passes through
/// every point and never overshoots, so between two neighbouring points its y stays within
/// theirs. Tangents are the Fritsch-Carlson ones: the mean of the neighbouring secant slopes,
/// zero at a local extremum or a flat step, and limited so no segment overshoots. Two points
/// stay a straight segment, as does a pair that is not strictly left to right. Empty for no
/// points.
pub fn smooth_polyline(points: &[Point]) -> BezPath {
    let mut path = BezPath::new();
    let Some(&first) = points.first() else {
        return path;
    };
    path.move_to(first);
    if points.len() == 2 {
        path.line_to(points[1]);
        return path;
    }
    let tangents = monotone_tangents(points);
    for (i, pair) in points.windows(2).enumerate() {
        let (a, b) = (pair[0], pair[1]);
        let dx = b.x - a.x;
        if !(dx.is_finite() && dx > 0.0) {
            path.line_to(b);
            continue;
        }
        let third = dx / 3.0;
        path.curve_to(
            (a.x + third, a.y + tangents[i] * third),
            (b.x - third, b.y - tangents[i + 1] * third),
            (b.x, b.y),
        );
    }
    path
}

/// The slope (dy/dx) at each point for a monotone cubic through `points`.
fn monotone_tangents(points: &[Point]) -> Vec<f64> {
    let n = points.len();
    let secants: Vec<f64> = points
        .windows(2)
        .map(|w| {
            let dx = w[1].x - w[0].x;
            if dx > 0.0 {
                (w[1].y - w[0].y) / dx
            } else {
                0.0
            }
        })
        .collect();
    let mut m = vec![0.0; n];
    if n < 2 {
        return m;
    }
    m[0] = secants[0];
    m[n - 1] = secants[n - 2];
    for k in 1..n - 1 {
        let (a, b) = (secants[k - 1], secants[k]);
        m[k] = if a * b <= 0.0 { 0.0 } else { (a + b) / 2.0 };
    }
    for (k, &d) in secants.iter().enumerate() {
        if d == 0.0 {
            m[k] = 0.0;
            m[k + 1] = 0.0;
            continue;
        }
        let (a, b) = (m[k] / d, m[k + 1] / d);
        let s = a * a + b * b;
        if s > 9.0 {
            let tau = 3.0 / s.sqrt();
            m[k] = tau * a * d;
            m[k + 1] = tau * b * d;
        }
    }
    m
}

/// The line geometry of `slots` (oldest to newest) in `plot`.
pub fn line_segments<S>(
    slots: S,
    capacity: usize,
    domain: (f32, f32),
    plot: Rect,
    orientation: Orientation,
) -> LineGeometry
where
    S: IntoIterator<Item = Option<f32>>,
    S::IntoIter: DoubleEndedIterator,
{
    line_from_runs(runs(slots, capacity, domain, plot, orientation))
}

/// The closed fill paths of `slots` in `plot`, oldest first.
///
/// A run of two or more samples is its polyline, then down to the baseline at the run's last
/// x, back along the baseline to its first x, closed. An isolated sample is a column one slot
/// spacing wide (the plot width at capacity 1), centred on it and kept inside the plot, from
/// the baseline up to the sample. The baseline is the edge the domain minimum sits on.
pub fn area_paths<S>(
    slots: S,
    capacity: usize,
    domain: (f32, f32),
    plot: Rect,
    orientation: Orientation,
) -> Vec<BezPath>
where
    S: IntoIterator<Item = Option<f32>>,
    S::IntoIter: DoubleEndedIterator,
{
    let half = if capacity == 1 {
        plot.width() / 2.0
    } else {
        slot_spacing(capacity, plot) / 2.0
    };
    let runs = runs(slots, capacity, domain, plot, orientation);
    area_from_runs(runs, half, plot, orientation, false)
}

/// [`area_paths`] with each run's top edge a monotone smooth curve (see [`smooth_polyline`]).
pub(crate) fn smooth_area_paths<S>(
    slots: S,
    capacity: usize,
    domain: (f32, f32),
    plot: Rect,
    orientation: Orientation,
) -> Vec<BezPath>
where
    S: IntoIterator<Item = Option<f32>>,
    S::IntoIter: DoubleEndedIterator,
{
    let half = if capacity == 1 {
        plot.width() / 2.0
    } else {
        slot_spacing(capacity, plot) / 2.0
    };
    let runs = runs(slots, capacity, domain, plot, orientation);
    area_from_runs(runs, half, plot, orientation, true)
}

/// The width in logical pixels of the area column an isolated timed sample fills.
pub const TIMED_COLUMN_WIDTH: f64 = 3.0;

/// The `(start, end)` time span a timed plot's x axis runs over, from the finite times in
/// `points`: `[newest - window, newest]` with a `window` (finite and above 0), else
/// `[oldest, newest]`. `None` when no point has a finite time.
pub fn time_span(points: &[(f64, f32)], window: Option<f64>) -> Option<(f64, f64)> {
    let mut times = points.iter().map(|p| p.0).filter(|t| t.is_finite());
    let first = times.next()?;
    let (oldest, newest) = times.fold((first, first), |(lo, hi), t| (lo.min(t), hi.max(t)));
    match window.filter(|w| w.is_finite() && *w > 0.0) {
        Some(window) => Some((newest - window, newest)),
        None => Some((oldest, newest)),
    }
}

/// The x of time `t` over `span`: `span.1` at `plot.x1` and `span.0` at `plot.x0`. An empty
/// span (a single point) puts every time at `plot.x1`.
pub fn x_for_time(t: f64, span: (f64, f64), plot: Rect) -> f64 {
    let (start, end) = span;
    let length = end - start;
    if !(length.is_finite() && length > 0.0) {
        return finite_or_zero(plot.x1);
    }
    finite_or_zero(plot.x1 - (end - t) / length * plot.width())
}

/// The line geometry of timed `points` (`(t, value)`, non-decreasing in `t`) in `plot`, each
/// at its time over [`time_span`]`(points, window)`, so the newest sits at `plot.x1`.
///
/// A point outside the span, or with a non-finite time or value, is a gap. Two consecutive
/// points more than `gap_after` seconds apart are not connected (a negative or non-finite
/// threshold connects every pair), so a point alone between two such gaps is an isolated
/// sample.
pub fn timed_line_segments(
    points: &[(f64, f32)],
    window: Option<f64>,
    gap_after: Option<f64>,
    domain: (f32, f32),
    plot: Rect,
    orientation: Orientation,
) -> LineGeometry {
    line_from_runs(timed_runs(
        points,
        window,
        gap_after,
        domain,
        plot,
        orientation,
    ))
}

/// The closed fill paths of timed `points` in `plot`, oldest first, with the runs of
/// [`timed_line_segments`].
///
/// A run of two or more samples closes to the baseline as in [`area_paths`]; an isolated
/// sample is a column [`TIMED_COLUMN_WIDTH`] wide, centred on it and kept inside the plot.
pub fn timed_area_paths(
    points: &[(f64, f32)],
    window: Option<f64>,
    gap_after: Option<f64>,
    domain: (f32, f32),
    plot: Rect,
    orientation: Orientation,
) -> Vec<BezPath> {
    let runs = timed_runs(points, window, gap_after, domain, plot, orientation);
    area_from_runs(runs, TIMED_COLUMN_WIDTH / 2.0, plot, orientation, false)
}

/// [`timed_area_paths`] with each run's top edge a monotone smooth curve (see
/// [`smooth_polyline`]).
pub(crate) fn smooth_timed_area_paths(
    points: &[(f64, f32)],
    window: Option<f64>,
    gap_after: Option<f64>,
    domain: (f32, f32),
    plot: Rect,
    orientation: Orientation,
) -> Vec<BezPath> {
    let runs = timed_runs(points, window, gap_after, domain, plot, orientation);
    area_from_runs(runs, TIMED_COLUMN_WIDTH / 2.0, plot, orientation, true)
}

/// The stops of a vertical gradient that colours by value through `tone`, over a half-plot
/// `plot` whose domain is `domain`.
///
/// The gradient starts at the baseline (offset 0, the domain minimum) and ends at the far
/// edge (offset 1, the domain maximum), so offsets ascend with value in either `orientation`.
/// Each threshold is a hard edge: two stops at the same offset, the colour below it then the
/// colour from it. A threshold outside the domain is clamped to the nearest edge; a tone with
/// no stops is its base colour alone. Every colour's alpha is scaled by `alpha`.
pub fn tone_gradient(
    tone: &Tone,
    domain: (f32, f32),
    plot: Rect,
    orientation: Orientation,
    alpha: f32,
) -> ToneGradient {
    let baseline = orientation.baseline(plot);
    let far = match orientation {
        Orientation::Upright => finite_or_zero(plot.y0),
        Orientation::Inverted => finite_or_zero(plot.y1),
    };
    let span = far - baseline;
    let scale = |c: Color| c.with_alpha(c.components[3] * alpha);
    let mut stops = vec![(0.0, scale(tone.base()))];
    let mut previous = tone.base();
    if span != 0.0 {
        for &(at, color) in tone.stops() {
            let offset = ((orientation.y_for(at, domain, plot) - baseline) / span).clamp(0.0, 1.0);
            stops.push((offset as f32, scale(previous)));
            stops.push((offset as f32, scale(color)));
            previous = color;
        }
    }
    stops.push((1.0, scale(previous)));
    ToneGradient {
        start_y: baseline,
        end_y: far,
        stops,
    }
}

/// A value-keyed vertical gradient: see [`tone_gradient`].
#[derive(Clone, Debug, PartialEq)]
pub struct ToneGradient {
    /// The widget-local y of offset 0 (the domain minimum).
    pub start_y: f64,
    /// The widget-local y of offset 1 (the domain maximum).
    pub end_y: f64,
    /// `(offset, colour)`, offsets ascending; coincident offsets are hard edges.
    pub stops: Vec<(f32, Color)>,
}

/// One block column: its rectangle and the (unclamped) sample it shows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlockColumn {
    /// The column, snapped to whole logical pixels on x and at the baseline.
    pub rect: Rect,
    /// The sample's value, for colouring (for example with [`Tone`](crate::Tone)).
    pub value: f32,
}

/// One column per present slot, oldest first, right-aligned so the newest column's right
/// edge is `plot.x1` (snapped).
///
/// `column_width` and `gap` are snapped first; a column narrower than 1 px after snapping
/// yields nothing. A gap slot leaves its place empty. A column that would start left of the
/// plot (snapped) is dropped, not clipped, so when more slots exist than fit, the newest win.
/// A column is at least 1 px tall, so a sample at the domain minimum stays visible.
pub fn block_columns<S>(
    slots: S,
    capacity: usize,
    column_width: f64,
    gap: f64,
    domain: (f32, f32),
    plot: Rect,
    orientation: Orientation,
) -> Vec<BlockColumn>
where
    S: IntoIterator<Item = Option<f32>>,
    S::IntoIter: DoubleEndedIterator,
{
    let width = snap(column_width);
    if !usable(plot) || width < 1.0 {
        return Vec::new();
    }
    let pitch = width + snap(gap).max(0.0);
    let (right, left_limit) = (snap(plot.x1), snap(plot.x0));
    let baseline = snap(orientation.baseline(plot));
    let map = YMap::new(domain, plot, orientation);
    let mut columns = Vec::new();
    for (age, slot) in slots.into_iter().rev().take(capacity).enumerate() {
        let x1 = right - age as f64 * pitch;
        let x0 = x1 - width;
        if x0 < left_limit {
            break;
        }
        let Some(value) = slot.filter(|v| v.is_finite()) else {
            continue;
        };
        let height = (baseline - map.y(value)).abs().max(1.0);
        let rect = match orientation {
            Orientation::Upright => Rect::new(x0, baseline - height, x1, baseline),
            Orientation::Inverted => Rect::new(x0, baseline, x1, baseline + height),
        };
        columns.push(BlockColumn { rect, value });
    }
    columns.reverse();
    columns
}

/// How many block columns of `column_width` with `gap` between them fit across `width`, with
/// the same snapping as [`block_columns`] (exact for a plot whose edges are whole pixels).
/// Size a [`Series`](crate::Series)' capacity with it.
pub fn fit_columns(width: f64, column_width: f64, gap: f64) -> usize {
    let column = snap(column_width);
    if !width.is_finite() || column < 1.0 || width < column {
        return 0;
    }
    let gap = snap(gap).max(0.0);
    ((width + gap) / (column + gap)).floor() as usize
}

/// `v` rounded to a whole logical pixel; 0 for a non-finite `v`.
pub fn snap(v: f64) -> f64 {
    // Adding 0.0 turns a rounded -0.0 into 0.0.
    if v.is_finite() { v.round() + 0.0 } else { 0.0 }
}

/// The top of a 1 px tall horizontal rule at `y`: the whole pixel row whose centre is nearest
/// `y`, so the rule lands crisply on one row.
pub fn snap_rule_y(y: f64) -> f64 {
    snap(y - 0.5)
}

/// Rule tops (see [`snap_rule_y`]) for horizontal rules at `fractions` of the plot's height,
/// 0.0 the bottom edge and 1.0 the top, in the order given. Fractions are clamped to
/// `0.0..=1.0` and non-finite ones skipped; every rule stays inside the (snapped) plot.
pub fn grid_rule_ys(fractions: &[f32], plot: Rect) -> Vec<f64> {
    if !usable(plot) {
        return Vec::new();
    }
    let top = snap(plot.y0);
    let bottom = (snap(plot.y1) - 1.0).max(top);
    fractions
        .iter()
        .filter(|f| f.is_finite())
        .map(|&f| {
            let y = plot.y1 - f64::from(f.clamp(0.0, 1.0)) * plot.height();
            snap_rule_y(y).clamp(top, bottom)
        })
        .collect()
}

/// Whether `plot` has a positive, finite width and height.
fn usable(plot: Rect) -> bool {
    [plot.x0, plot.y0, plot.x1, plot.y1]
        .iter()
        .all(|v| v.is_finite())
        && plot.width() > 0.0
        && plot.height() > 0.0
}

fn finite_or_zero(v: f64) -> f64 {
    if v.is_finite() { v } else { 0.0 }
}

/// A value-to-y mapping with the domain normalised once.
struct YMap {
    min: f64,
    span: f64,
    baseline: f64,
    /// Signed height from the baseline to the domain maximum.
    extent: f64,
}

impl YMap {
    fn new(domain: (f32, f32), plot: Rect, orientation: Orientation) -> Self {
        let (min, max) = YDomain::fixed(domain.0, domain.1).resolve(None);
        let (min, max) = (f64::from(min), f64::from(max));
        let height = finite_or_zero(plot.height());
        Self {
            min,
            span: max - min,
            baseline: orientation.baseline(plot),
            extent: match orientation {
                Orientation::Upright => -height,
                Orientation::Inverted => height,
            },
        }
    }

    fn y(&self, value: f32) -> f64 {
        if !value.is_finite() {
            return self.baseline;
        }
        let t = ((f64::from(value) - self.min) / self.span).clamp(0.0, 1.0);
        finite_or_zero(self.baseline + t * self.extent)
    }
}

/// The runs of consecutive present samples among the newest `capacity` slots, oldest run
/// first, each run's points oldest to newest.
fn runs<S>(
    slots: S,
    capacity: usize,
    domain: (f32, f32),
    plot: Rect,
    orientation: Orientation,
) -> Vec<Vec<Point>>
where
    S: IntoIterator<Item = Option<f32>>,
    S::IntoIter: DoubleEndedIterator,
{
    if !usable(plot) {
        return Vec::new();
    }
    let spacing = slot_spacing(capacity, plot);
    let map = YMap::new(domain, plot, orientation);
    let mut runs = Vec::new();
    let mut current: Vec<Point> = Vec::new();
    for (age, slot) in slots.into_iter().rev().take(capacity).enumerate() {
        match slot.filter(|v| v.is_finite()) {
            Some(value) => {
                let x = finite_or_zero(plot.x1 - age as f64 * spacing);
                current.push(Point::new(x, map.y(value)));
            }
            None if !current.is_empty() => {
                current.reverse();
                runs.push(std::mem::take(&mut current));
            }
            None => {}
        }
    }
    if !current.is_empty() {
        current.reverse();
        runs.push(current);
    }
    runs.reverse();
    runs
}

/// Split runs into a line's polylines and its isolated samples.
fn line_from_runs(runs: Vec<Vec<Point>>) -> LineGeometry {
    let mut geometry = LineGeometry::default();
    for run in runs {
        if run.len() == 1 {
            geometry.points.push(run[0]);
        } else {
            geometry.polylines.push(run);
        }
    }
    geometry
}

/// Close each run to the baseline: a run of two or more along its polyline, an isolated
/// sample as a column `half` either side of it, kept inside the plot.
fn area_from_runs(
    runs: Vec<Vec<Point>>,
    half: f64,
    plot: Rect,
    orientation: Orientation,
    smooth: bool,
) -> Vec<BezPath> {
    let baseline = orientation.baseline(plot);
    let mut paths = Vec::new();
    for run in runs {
        let mut path = BezPath::new();
        if let [p] = run[..] {
            let (x0, x1) = ((p.x - half).max(plot.x0), (p.x + half).min(plot.x1));
            path.move_to((x0, baseline));
            path.line_to((x0, p.y));
            path.line_to((x1, p.y));
            path.line_to((x1, baseline));
        } else {
            path = if smooth {
                smooth_polyline(&run)
            } else {
                polyline(&run)
            };
            let (first, last) = (run[0], run[run.len() - 1]);
            path.line_to((last.x, baseline));
            path.line_to((first.x, baseline));
        }
        path.close_path();
        paths.push(path);
    }
    paths
}

/// The runs of timed points (see [`timed_line_segments`]), oldest run first, each oldest to
/// newest.
fn timed_runs(
    points: &[(f64, f32)],
    window: Option<f64>,
    gap_after: Option<f64>,
    domain: (f32, f32),
    plot: Rect,
    orientation: Orientation,
) -> Vec<Vec<Point>> {
    if !usable(plot) {
        return Vec::new();
    }
    let Some(span) = time_span(points, window) else {
        return Vec::new();
    };
    let gap_after = gap_after.filter(|g| g.is_finite() && *g >= 0.0);
    let map = YMap::new(domain, plot, orientation);
    let mut runs = Vec::new();
    let mut current: Vec<Point> = Vec::new();
    let mut previous: Option<f64> = None;
    for &(t, value) in points {
        let drawn = t.is_finite() && value.is_finite() && (span.0..=span.1).contains(&t);
        let parted = match (previous, gap_after) {
            (Some(previous), Some(gap_after)) => t - previous > gap_after,
            _ => false,
        };
        if (!drawn || parted) && !current.is_empty() {
            runs.push(std::mem::take(&mut current));
        }
        if !drawn {
            previous = None;
            continue;
        }
        current.push(Point::new(x_for_time(t, span, plot), map.y(value)));
        previous = Some(t);
    }
    if !current.is_empty() {
        runs.push(current);
    }
    runs
}
