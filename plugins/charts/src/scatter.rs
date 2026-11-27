//! The `scatter_chart` widget: points placed on two value axes, each with its own size and
//! colour.
//!
//! Data lives in a [`Points`] handle: a shared, bounded list of `(x, y)` points, each with an
//! optional radius and an optional colour index. The widget reads it while painting, so a
//! write shows with no rebuild: the write requests a repaint and the next frame draws the new
//! points. The chart takes no input, draws no text and never requests a frame.
//!
//! Both axes are [`YDomain`] values, automatic by default: the range spans the data's minimum
//! to its maximum on that axis with both ends rounded outward. A point whose x or y lies
//! outside its domain is skipped, never clamped onto an edge.
//!
//! Points of one colour are batched into a single path per paint, and the built paths are
//! cached by the handle's revision, the plot size, the resolved domains and the colour that
//! stands in for an unset one, so an unchanged frame rebuilds no geometry. Circles are
//! paths, squares and crosses are made of whole-pixel rectangles.
//!
//! A point's colour is, in order: its colour index into [`colors`](ScatterChartView::colors)
//! (taken modulo the palette's length), [`tone_by_y`](ScatterChartView::tone_by_y) applied to
//! its y value, [`color`](ScatterChartView::color), the installed theme's primary colour, then
//! [`Tone::FALLBACK_CALM`]. Grid rules take the theme's `outline_variant`, then a fallback.

use std::collections::VecDeque;
use std::fmt;
use std::marker::PhantomData;
use std::sync::{Arc, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

use frust::authoring::{
    BezPath, BoxConstraints, Brush, BuildCtx, ChangeFlags, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, Point, Rect, Role, SemanticsCtx, Shape, Size, View, Widget,
};
use frust::kurbo::Circle;
use frust::{Color, Theme};

use crate::geometry::{grid_rule_ys, snap, y_for};
use crate::{Tone, YDomain};

const DEFAULT_RADIUS: f32 = 3.0;
const DEFAULT_HEIGHT: f64 = 160.0;
const UNBOUNDED_WIDTH: f64 = 160.0;
const FALLBACK_GRID: Color = Color::from_rgb8(0xd0, 0xd4, 0xda);
/// The tolerance circle paths are flattened to, in logical pixels.
const PATH_TOLERANCE: f64 = 0.1;

/// One point of a [`Points`] handle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScatterPoint {
    /// The value on the x axis.
    pub x: f32,
    /// The value on the y axis.
    pub y: f32,
    /// This point's radius in logical pixels; `None` takes the chart's default.
    pub radius: Option<f32>,
    /// An index into the chart's colour palette; `None` takes the chart's other colour rules.
    pub color: Option<usize>,
}

impl ScatterPoint {
    /// A point at `(x, y)` with the chart's default radius and colour.
    pub const fn new(x: f32, y: f32) -> Self {
        Self {
            x,
            y,
            radius: None,
            color: None,
        }
    }

    /// This point with its own radius.
    pub const fn with_radius(mut self, radius: f32) -> Self {
        self.radius = Some(radius);
        self
    }

    /// This point with a palette colour index.
    pub const fn with_color(mut self, index: usize) -> Self {
        self.color = Some(index);
        self
    }
}

impl From<(f32, f32)> for ScatterPoint {
    fn from((x, y): (f32, f32)) -> Self {
        Self::new(x, y)
    }
}

/// Whether `point` can be stored: both coordinates are finite. A non-finite radius is dropped.
fn sanitize(point: ScatterPoint) -> Option<ScatterPoint> {
    (point.x.is_finite() && point.y.is_finite()).then(|| ScatterPoint {
        radius: point.radius.filter(|r| r.is_finite()),
        ..point
    })
}

struct Store {
    points: VecDeque<ScatterPoint>,
    capacity: usize,
    revision: u64,
}

impl Store {
    fn push(&mut self, point: ScatterPoint) {
        if self.points.len() == self.capacity {
            self.points.pop_front();
        }
        self.points.push_back(point);
    }
}

/// A cheap-to-clone, `Send + Sync` handle to a bounded list of scatter points, oldest first.
///
/// The oldest point is dropped when a push would exceed the capacity, so a live feed cannot
/// grow without bound. A point with a non-finite `x` or `y` is not stored.
///
/// Every call that changes the content bumps [`revision`](Self::revision) once and calls
/// [`frust::request_repaint`] once, however many points it wrote; a call that changes nothing
/// does neither. Clones share one list, so a background thread can push while a chart paints.
/// A lock poisoned by a panicking writer stays usable: the list is always left consistent.
///
/// ```
/// use frust_charts::{Points, ScatterPoint};
///
/// let points = Points::new(500);
/// points.push(1.0, 2.0);
/// points.push_point(ScatterPoint::new(3.0, 4.0).with_radius(6.0).with_color(1));
/// assert_eq!(points.len(), 2);
/// ```
#[derive(Clone)]
pub struct Points {
    inner: Arc<RwLock<Store>>,
}

impl Points {
    /// An empty list holding at most `capacity` points.
    ///
    /// # Panics
    ///
    /// If `capacity` is 0.
    pub fn new(capacity: usize) -> Self {
        assert!(capacity >= 1, "Points::new: capacity must be at least 1");
        Self {
            inner: Arc::new(RwLock::new(Store {
                points: VecDeque::new(),
                capacity,
                revision: 0,
            })),
        }
    }

    fn read(&self) -> RwLockReadGuard<'_, Store> {
        self.inner.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn write(&self) -> RwLockWriteGuard<'_, Store> {
        self.inner.write().unwrap_or_else(PoisonError::into_inner)
    }

    /// Run `mutate`; when it reports a change, bump the revision and request one repaint
    /// (after the lock is released).
    fn mutate(&self, mutate: impl FnOnce(&mut Store) -> bool) {
        let changed = {
            let mut store = self.write();
            let changed = mutate(&mut store);
            if changed {
                store.revision = store.revision.wrapping_add(1);
            }
            changed
        };
        if changed {
            frust::request_repaint();
        }
    }

    /// The most points this list holds.
    pub fn capacity(&self) -> usize {
        self.read().capacity
    }

    /// How many points are stored, at most [`capacity`](Self::capacity).
    pub fn len(&self) -> usize {
        self.read().points.len()
    }

    /// Whether there are no points.
    pub fn is_empty(&self) -> bool {
        self.read().points.is_empty()
    }

    /// Append a point at `(x, y)`, dropping the oldest when full. A non-finite coordinate
    /// stores nothing.
    pub fn push(&self, x: f32, y: f32) {
        self.push_point(ScatterPoint::new(x, y));
    }

    /// Append one point, dropping the oldest when full.
    pub fn push_point(&self, point: ScatterPoint) {
        self.extend([point]);
    }

    /// Append every point in order: one revision bump and one repaint request for the whole
    /// call, none when nothing was stored.
    pub fn extend(&self, points: impl IntoIterator<Item = ScatterPoint>) {
        self.mutate(|store| {
            let mut changed = false;
            for point in points.into_iter().filter_map(sanitize) {
                store.push(point);
                changed = true;
            }
            changed
        });
    }

    /// Clear, then fill with `points`, keeping the newest [`capacity`](Self::capacity) of
    /// them. A call that leaves the content unchanged is a no-op.
    pub fn replace(&self, points: impl IntoIterator<Item = ScatterPoint>) {
        let capacity = self.capacity();
        let mut fresh: VecDeque<ScatterPoint> = VecDeque::new();
        for point in points.into_iter().filter_map(sanitize) {
            if fresh.len() == capacity {
                fresh.pop_front();
            }
            fresh.push_back(point);
        }
        self.mutate(|store| {
            if store.points == fresh {
                return false;
            }
            store.points = fresh;
            true
        });
    }

    /// Overwrite the point at `index` (0 is the oldest). A no-op when `index` is past the
    /// end, the point has a non-finite coordinate, or it equals the one already there.
    pub fn set(&self, index: usize, point: ScatterPoint) {
        let Some(point) = sanitize(point) else {
            return;
        };
        self.mutate(|store| match store.points.get_mut(index) {
            Some(slot) if *slot != point => {
                *slot = point;
                true
            }
            _ => false,
        });
    }

    /// Remove every point; a no-op on an empty list.
    pub fn clear(&self) {
        self.mutate(|store| {
            let changed = !store.points.is_empty();
            store.points.clear();
            changed
        });
    }

    /// A counter that moves on every content change: compare it to skip work.
    pub fn revision(&self) -> u64 {
        self.read().revision
    }

    /// Read the points oldest to newest without copying, under a short read lock.
    ///
    /// Keep `f` brief (a paint): a writer on another thread waits for it to return, and a
    /// write to this same list from inside `f` deadlocks.
    pub fn with_points<R>(&self, f: impl FnOnce(PointsView<'_>) -> R) -> R {
        let store = self.read();
        let (first, second) = store.points.as_slices();
        f(PointsView {
            first,
            second,
            revision: store.revision,
        })
    }

    /// Whether `self` and `other` share one list (a clone of the same handle).
    pub fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl fmt::Debug for Points {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let store = self.read();
        f.debug_struct("Points")
            .field("capacity", &store.capacity)
            .field("len", &store.points.len())
            .field("revision", &store.revision)
            .finish()
    }
}

/// Borrowed read access to a [`Points`] list, oldest to newest, from [`Points::with_points`].
#[derive(Clone, Copy, Debug)]
pub struct PointsView<'a> {
    first: &'a [ScatterPoint],
    second: &'a [ScatterPoint],
    revision: u64,
}

impl<'a> PointsView<'a> {
    /// How many points there are.
    pub fn len(&self) -> usize {
        self.first.len() + self.second.len()
    }

    /// Whether there are no points.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The handle's revision when this view was taken.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The point at `index` (0 is the oldest); `None` past the end.
    pub fn get(&self, index: usize) -> Option<ScatterPoint> {
        match index.checked_sub(self.first.len()) {
            None => self.first.get(index).copied(),
            Some(rest) => self.second.get(rest).copied(),
        }
    }

    /// Every point, oldest to newest.
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = ScatterPoint> + 'a {
        self.first.iter().chain(self.second.iter()).copied()
    }
}

/// The mark drawn for each point.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScatterShape {
    /// A filled disc (the default).
    #[default]
    Circle,
    /// A filled square on whole pixels.
    Square,
    /// A plus sign of two crossing bars on whole pixels.
    Cross,
}

#[derive(Clone)]
struct Spec {
    points: Points,
    x_domain: YDomain,
    y_domain: YDomain,
    radius: f32,
    color: Option<Color>,
    colors: Vec<Color>,
    tone: Option<Tone>,
    shape: ScatterShape,
    grid: (Vec<f32>, Vec<f32>),
    height: Option<f32>,
    semantics: Option<String>,
}

impl Spec {
    /// What a widget holding `old` must redo to show `self`.
    fn changes(&self, old: &Self) -> ChangeFlags {
        if self.height != old.height {
            return ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if !self.points.ptr_eq(&old.points)
            || self.x_domain != old.x_domain
            || self.y_domain != old.y_domain
            || self.radius != old.radius
            || self.color != old.color
            || self.colors != old.colors
            || self.tone != old.tone
            || self.shape != old.shape
            || self.grid != old.grid
            || self.semantics != old.semantics
        {
            return ChangeFlags::PAINT;
        }
        ChangeFlags::NONE
    }
}

/// A scatter chart of `points`.
///
/// ```
/// use frust_charts::{Points, ScatterShape, YDomain, scatter_chart};
///
/// let points = Points::new(1_000);
/// points.push(1.0, 20.0);
/// let _view = scatter_chart::<()>(points.clone())
///     .y_domain(YDomain::fixed(0.0, 100.0))
///     .shape(ScatterShape::Square)
///     .radius(4.0)
///     .grid(&[0.0, 0.5, 1.0], &[0.0, 0.5, 1.0])
///     .height(160.0)
///     .semantics("Latency by request size");
/// ```
pub fn scatter_chart<S: 'static>(points: Points) -> ScatterChartView<S> {
    ScatterChartView {
        spec: Spec {
            points,
            x_domain: YDomain::auto(),
            y_domain: YDomain::auto(),
            radius: DEFAULT_RADIUS,
            color: None,
            colors: Vec::new(),
            tone: None,
            shape: ScatterShape::Circle,
            grid: (Vec::new(), Vec::new()),
            height: None,
            semantics: None,
        },
        _state: PhantomData,
    }
}

/// The declarative view built by [`scatter_chart`].
pub struct ScatterChartView<S: 'static> {
    spec: Spec,
    _state: PhantomData<fn(&mut S)>,
}

impl<S: 'static> ScatterChartView<S> {
    /// The x axis range (default [`YDomain::auto`]).
    pub fn x_domain(mut self, domain: YDomain) -> Self {
        self.spec.x_domain = domain;
        self
    }

    /// The y axis range (default [`YDomain::auto`]).
    pub fn y_domain(mut self, domain: YDomain) -> Self {
        self.spec.y_domain = domain;
        self
    }

    /// The radius, in logical pixels, of a point that sets none (default 3). A square's side
    /// is twice the radius and a cross's arms reach the radius from the centre.
    pub fn radius(mut self, radius: f32) -> Self {
        self.spec.radius = radius;
        self
    }

    /// One colour for every point with no palette colour and no tone.
    pub fn color(mut self, color: Color) -> Self {
        self.spec.color = Some(color);
        self
    }

    /// The palette a point's colour index selects from, taken modulo its length.
    pub fn colors(mut self, colors: impl IntoIterator<Item = Color>) -> Self {
        self.spec.colors = colors.into_iter().collect();
        self
    }

    /// Colour a point with no palette colour by its y value through `tone`.
    pub fn tone_by_y(mut self, tone: Tone) -> Self {
        self.spec.tone = Some(tone);
        self
    }

    /// The mark drawn for each point (default [`ScatterShape::Circle`]).
    pub fn shape(mut self, shape: ScatterShape) -> Self {
        self.spec.shape = shape;
        self
    }

    /// Grid rules: vertical rules at `x_fractions` of the width (0.0 the left edge) and
    /// horizontal rules at `y_fractions` of the height (0.0 the bottom edge). Fractions are
    /// clamped to `0.0..=1.0`; non-finite ones are skipped.
    pub fn grid(mut self, x_fractions: &[f32], y_fractions: &[f32]) -> Self {
        self.spec.grid = (x_fractions.to_vec(), y_fractions.to_vec());
        self
    }

    /// A fixed height in logical pixels (default: the available height when bounded, else 160).
    pub fn height(mut self, height: f32) -> Self {
        self.spec.height = Some(height);
        self
    }

    /// An accessible name; the chart is exposed as one image node only when this is set.
    pub fn semantics(mut self, label: impl Into<String>) -> Self {
        self.spec.semantics = Some(label.into());
        self
    }
}

impl<S: 'static> View<S> for ScatterChartView<S> {
    type Element = ScatterChartWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> ScatterChartWidget {
        ScatterChartWidget {
            spec: self.spec.clone(),
            extents: None,
            built: None,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ScatterChartWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let flags = self.spec.changes(&prev.spec);
        if flags != ChangeFlags::NONE {
            element.spec = self.spec.clone();
            element.extents = None;
            element.built = None;
        }
        flags
    }
}

/// The smallest and largest x and y of the stored points.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Extents {
    x: (f32, f32),
    y: (f32, f32),
}

fn extents_of(view: &PointsView<'_>) -> Option<Extents> {
    let mut points = view.iter();
    let first = points.next()?;
    let mut e = Extents {
        x: (first.x, first.x),
        y: (first.y, first.y),
    };
    for p in points {
        e.x = (e.x.0.min(p.x), e.x.1.max(p.x));
        e.y = (e.y.0.min(p.y), e.y.1.max(p.y));
    }
    Some(e)
}

/// What the built paths depend on besides the widget's own options.
#[derive(Clone, Debug, PartialEq)]
struct BuildKey {
    revision: u64,
    plot: Rect,
    x_domain: (f32, f32),
    y_domain: (f32, f32),
    default_color: Color,
}

struct Built {
    key: BuildKey,
    /// One path per distinct colour, in order of first appearance.
    groups: Vec<(Color, BezPath)>,
}

/// The retained widget behind [`ScatterChartView`].
pub struct ScatterChartWidget {
    spec: Spec,
    /// The data's extents and the revision they were taken at.
    extents: Option<(u64, Option<Extents>)>,
    built: Option<Built>,
}

/// A finite, non-negative length.
fn length(value: f32) -> f64 {
    if value.is_finite() {
        f64::from(value).max(0.0)
    } else {
        0.0
    }
}

impl ScatterChartWidget {
    fn color_of(&self, point: &ScatterPoint, default: Color) -> Color {
        let spec = &self.spec;
        if let (Some(index), false) = (point.color, spec.colors.is_empty()) {
            return spec.colors[index % spec.colors.len()];
        }
        match (&spec.tone, spec.color) {
            (Some(tone), _) => tone.color_at(point.y),
            (None, Some(color)) => color,
            (None, None) => default,
        }
    }

    /// Append `point`'s mark, centred at `(cx, cy)` with `radius`, to `path`.
    fn add_mark(&self, path: &mut BezPath, (cx, cy): (f64, f64), radius: f64) {
        match self.spec.shape {
            ScatterShape::Circle => append(path, &Circle::new((cx, cy), radius)),
            ScatterShape::Square => {
                let side = snap(radius * 2.0).max(1.0);
                let (x, y) = (snap(cx - radius), snap(cy - radius));
                append(path, &Rect::new(x, y, x + side, y + side));
            }
            ScatterShape::Cross => {
                let arm = snap(radius).max(1.0);
                let bar = snap(radius / 2.0).max(1.0);
                let (x, y) = (snap(cx), snap(cy));
                let half = (bar / 2.0).floor();
                append(path, &Rect::new(x - arm, y - half, x + arm, y - half + bar));
                append(path, &Rect::new(x - half, y - arm, x - half + bar, y + arm));
            }
        }
    }

    fn build(&self, view: &PointsView<'_>, key: BuildKey) -> Built {
        let (x_lo, x_hi) = key.x_domain;
        let (y_lo, y_hi) = key.y_domain;
        let default_radius = length(self.spec.radius);
        let drawn = |point: &ScatterPoint| {
            let in_range = point.x >= x_lo && point.x <= x_hi && point.y >= y_lo && point.y <= y_hi;
            let radius = point.radius.map_or(default_radius, length);
            (in_range && radius > 0.0).then_some(radius)
        };
        // The data area is the plot inset by the largest mark's reach, so a point on the range
        // edge is drawn whole inside the clip; the grid still spans the full plot.
        let reach = view
            .iter()
            .filter_map(|p| drawn(&p))
            .fold(0.0_f64, f64::max);
        let inset = match self.spec.shape {
            ScatterShape::Circle => reach.ceil(),
            // Snapping to whole pixels can move a rectangle's far edge up to a pixel outward.
            ScatterShape::Square | ScatterShape::Cross => reach.ceil() + 1.0,
        }
        .min((key.plot.width().min(key.plot.height()) / 2.0).max(0.0));
        let plot = key.plot.inset(-inset);
        let mut groups: Vec<(Color, BezPath)> = Vec::new();
        for point in view.iter() {
            let Some(radius) = drawn(&point) else {
                continue;
            };
            let fraction = f64::from(point.x - x_lo) / f64::from(x_hi - x_lo);
            let cx = plot.x0 + fraction * plot.width();
            let cy = y_for(point.y, key.y_domain, plot);
            let color = self.color_of(&point, key.default_color);
            let at = groups.iter().position(|(c, _)| *c == color);
            let at = at.unwrap_or_else(|| {
                groups.push((color, BezPath::new()));
                groups.len() - 1
            });
            self.add_mark(&mut groups[at].1, (cx, cy), radius);
        }
        Built { key, groups }
    }

    fn paint_grid(&self, origin: Point, plot: Rect, color: Color, scene: &mut dyn PaintScene) {
        let (x_fractions, y_fractions) = &self.spec.grid;
        let left = snap(plot.x0);
        let right = (snap(plot.x1) - 1.0).max(left);
        let mut xs: Vec<f64> = x_fractions
            .iter()
            .filter(|f| f.is_finite())
            .map(|&f| {
                let x = plot.x0 + f64::from(f.clamp(0.0, 1.0)) * plot.width();
                snap(x - 0.5).clamp(left, right)
            })
            .collect();
        xs.sort_by(f64::total_cmp);
        xs.dedup();
        for x in xs {
            scene.fill_rect(
                Point::new(origin.x + x, origin.y + plot.y0),
                Size::new(1.0, plot.height()),
                color,
            );
        }
        let mut ys = grid_rule_ys(y_fractions, plot);
        ys.sort_by(f64::total_cmp);
        ys.dedup();
        for y in ys {
            scene.fill_rect(
                Point::new(origin.x + plot.x0, origin.y + y),
                Size::new(plot.width(), 1.0),
                color,
            );
        }
    }
}

/// Append a shape's outline to `path`.
fn append(path: &mut BezPath, shape: &impl Shape) {
    for element in shape.path_elements(PATH_TOLERANCE) {
        path.push(element);
    }
}

impl Widget for ScatterChartWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let max = bc.max();
        let width = if max.width.is_finite() {
            max.width
        } else {
            UNBOUNDED_WIDTH
        };
        let height = match self.spec.height.map(f64::from) {
            Some(height) if height.is_finite() => height.max(0.0),
            _ if max.height.is_finite() => max.height,
            _ => DEFAULT_HEIGHT,
        };
        bc.constrain(Size::new(width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let (origin, size) = (ctx.origin(), ctx.size());
        if !(size.width > 0.0 && size.height > 0.0) {
            return;
        }
        let theme = Theme::from_paint_ctx(ctx);
        let default_color = theme.map_or(Tone::FALLBACK_CALM, |t| t.scheme().primary);
        let grid_color = theme.map_or(FALLBACK_GRID, |t| t.scheme().outline_variant);
        let plot = Rect::from_origin_size(Point::ZERO, size);

        scene.push_clip(origin, size);
        self.paint_grid(origin, plot, grid_color, scene);
        let points = self.spec.points.clone();
        points.with_points(|view| {
            let revision = view.revision();
            if self.extents.map(|(r, _)| r) != Some(revision) {
                self.extents = Some((revision, extents_of(&view)));
            }
            let extents = self.extents.and_then(|(_, e)| e);
            let key = BuildKey {
                revision,
                plot,
                x_domain: self.spec.x_domain.resolve_range(extents.map(|e| e.x)),
                y_domain: self.spec.y_domain.resolve_range(extents.map(|e| e.y)),
                default_color,
            };
            if self.built.as_ref().map(|b| &b.key) != Some(&key) {
                self.built = Some(self.build(&view, key));
            }
        });
        if let Some(built) = &self.built {
            for (color, path) in &built.groups {
                scene.fill_path(origin, path, &Brush::Solid(*color));
            }
        }
        scene.pop_clip();
    }

    fn event(&mut self, _ctx: &mut EventCtx, _event: &InputEvent) -> EventResult {
        EventResult::Ignored
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        if let Some(label) = &self.spec.semantics {
            ctx.push_node(Role::Image, |node| node.set_label(label.as_str()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Extents, Points, ScatterPoint, extents_of};

    #[test]
    fn extents_span_the_stored_points() {
        let points = Points::new(4);
        points.extend([(1.0, 5.0), (-2.0, 9.0), (3.0, 0.0)].map(ScatterPoint::from));
        let e = points.with_points(|view| extents_of(&view));
        assert_eq!(
            e,
            Some(Extents {
                x: (-2.0, 3.0),
                y: (0.0, 9.0)
            })
        );
    }
}
