//! `TimedSeries` and its charts: the handle's ordering, bounds and write accounting, the
//! time-mapped geometry, and a real `RenderRoot` painting `time_series` and `sparkline` over
//! timed data into a recording scene.
//!
//! Writing a handle requests a process-wide repaint, so every test holds `STATICS`.

use std::any::Any;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use frust::authoring::scene::GlyphRun;
use frust::authoring::text::TextContext;
use frust::authoring::{BezPath, Brush, ChangeFlags, PaintScene, Point, Rect, Shape, Size};
use frust::{Color, FrameTime};
use frust_charts::geometry::{
    Orientation, TIMED_COLUMN_WIDTH, time_span, timed_area_paths, timed_line_segments, x_for_time,
};
use frust_charts::{
    PlotStyle, Series, SeriesSource, SeriesStyle, SparkStyle, SparklineView, TimeSeriesView,
    TimedSeries, YAxis, YDomain, sparkline, time_series,
};
use frust_core::{RenderRoot, repaint_generation};
use frust_reactive::ReactiveRuntime;

static STATICS: Mutex<()> = Mutex::new(());

fn hold() -> MutexGuard<'static, ()> {
    STATICS.lock().unwrap_or_else(PoisonError::into_inner)
}

const RED: Color = Color::from_rgb8(0xd0, 0x10, 0x10);
const BLUE: Color = Color::from_rgb8(0x10, 0x20, 0xd0);
const GREY: Color = Color::from_rgb8(0x80, 0x80, 0x80);
const EPS: f64 = 1e-6;

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-3
}

fn points(series: &TimedSeries) -> Vec<(f64, f32)> {
    series.with_points(<[_]>::to_vec)
}

/// How many repaint requests `f` made.
fn repaints(f: impl FnOnce()) -> u64 {
    let before = repaint_generation();
    f();
    repaint_generation() - before
}

/// How many repaint requests and revision bumps `f` made on `series`.
fn writes(series: &TimedSeries, f: impl FnOnce()) -> (u64, u64) {
    let revision = series.revision();
    let repaints = repaints(f);
    (repaints, series.revision().wrapping_sub(revision))
}

// ---- The handle ----

#[test]
fn handles_are_send_and_sync() {
    fn check<T: Send + Sync + Clone>() {}
    check::<TimedSeries>();
    check::<SeriesSource>();
}

#[test]
#[should_panic(expected = "capacity must be at least 1")]
fn zero_capacity_panics() {
    let _ = TimedSeries::new(0);
}

#[test]
#[should_panic(expected = "finite and above 0")]
fn a_zero_window_panics() {
    let _ = TimedSeries::windowed(0.0);
}

#[test]
fn pushes_are_kept_in_time_order_and_a_repeated_time_replaces() {
    let _g = hold();
    let s = TimedSeries::new(8);
    assert!(s.is_empty());
    s.push(3.0, 30.0);
    s.push(1.0, 10.0);
    s.push(2.0, 20.0);
    s.push(5.0, 50.0);
    assert_eq!(
        points(&s),
        [(1.0, 10.0), (2.0, 20.0), (3.0, 30.0), (5.0, 50.0)]
    );

    // A repeated time replaces the value in place, once.
    assert_eq!(writes(&s, || s.push(2.0, 25.0)), (1, 1));
    assert_eq!(
        points(&s),
        [(1.0, 10.0), (2.0, 25.0), (3.0, 30.0), (5.0, 50.0)]
    );
    // The same point again changes nothing.
    assert_eq!(writes(&s, || s.push(2.0, 25.0)), (0, 0));

    assert_eq!(s.len(), 4);
    assert_eq!(s.latest(), Some((5.0, 50.0)));
    assert_eq!(s.max(), Some(50.0));
    assert_eq!(TimedSeries::new(2).max(), None);
    assert_eq!(TimedSeries::new(2).latest(), None);
}

#[test]
fn a_non_finite_time_or_value_is_rejected() {
    let _g = hold();
    let s = TimedSeries::new(8);
    s.push(1.0, 1.0);
    for (t, v) in [
        (f64::NAN, 2.0),
        (f64::INFINITY, 2.0),
        (2.0, f32::NAN),
        (2.0, f32::NEG_INFINITY),
    ] {
        assert_eq!(writes(&s, || s.push(t, v)), (0, 0), "({t}, {v})");
    }
    assert_eq!(
        writes(&s, || s.extend([(f64::NAN, 1.0), (3.0, f32::NAN)])),
        (0, 0)
    );
    assert_eq!(points(&s), [(1.0, 1.0)]);
}

#[test]
fn a_window_drops_points_older_than_newest_minus_window() {
    let _g = hold();
    let s = TimedSeries::windowed(10.0);
    assert_eq!((s.capacity(), s.window_secs()), (None, Some(10.0)));
    s.extend([(0.0, 1.0), (4.0, 2.0), (8.0, 3.0)]);
    assert_eq!(s.len(), 3);
    // Newest 14: the window starts at 4, which stays; 0 goes.
    s.push(14.0, 4.0);
    assert_eq!(points(&s), [(4.0, 2.0), (8.0, 3.0), (14.0, 4.0)]);
    // A late point inside the window is inserted; one before it is a no-op.
    s.push(6.0, 9.0);
    assert_eq!(writes(&s, || s.push(3.0, 9.0)), (0, 0));
    assert_eq!(
        points(&s),
        [(4.0, 2.0), (6.0, 9.0), (8.0, 3.0), (14.0, 4.0)]
    );
    // A leap forward empties all but the newest.
    s.push(100.0, 5.0);
    assert_eq!(points(&s), [(100.0, 5.0)]);
}

#[test]
fn capacity_drops_the_oldest_points() {
    let _g = hold();
    let s = TimedSeries::new(3);
    s.extend((0..5).map(|i| (f64::from(i), i as f32)));
    assert_eq!(points(&s), [(2.0, 2.0), (3.0, 3.0), (4.0, 4.0)]);
    // A full series ignores a point older than all it holds, and drops its oldest for a
    // point inside the range.
    assert_eq!(writes(&s, || s.push(1.0, 9.0)), (0, 0));
    s.push(2.5, 9.0);
    assert_eq!(points(&s), [(2.5, 9.0), (3.0, 3.0), (4.0, 4.0)]);
}

#[test]
fn both_bounds_apply_together() {
    let _g = hold();
    let s = TimedSeries::new(3).window(5.0);
    assert_eq!((s.capacity(), s.window_secs()), (Some(3), Some(5.0)));
    s.extend([(0.0, 0.0), (1.0, 1.0), (2.0, 2.0), (3.0, 3.0)]);
    assert_eq!(points(&s), [(1.0, 1.0), (2.0, 2.0), (3.0, 3.0)]);
    s.push(7.0, 7.0);
    assert_eq!(points(&s), [(2.0, 2.0), (3.0, 3.0), (7.0, 7.0)]);

    // Narrowing the window of a live series evicts at once and counts as one write.
    let narrowed = s.clone();
    assert_eq!(
        writes(&s, move || {
            let _ = narrowed.window(2.0);
        }),
        (1, 1)
    );
    assert_eq!(points(&s), [(7.0, 7.0)]);
}

#[test]
fn every_changing_call_writes_once_and_a_no_op_writes_nothing() {
    let _g = hold();
    let s = TimedSeries::new(8);
    assert_eq!(s.revision(), 0);
    assert_eq!(writes(&s, || s.push(1.0, 1.0)), (1, 1));
    assert_eq!(
        writes(&s, || s.extend([(2.0, 2.0), (3.0, 3.0), (4.0, 4.0)])),
        (1, 1)
    );
    assert_eq!(writes(&s, || s.extend([])), (0, 0));
    assert_eq!(
        writes(&s, || s.replace([
            (1.0, 1.0),
            (2.0, 2.0),
            (3.0, 3.0),
            (4.0, 4.0)
        ])),
        (0, 0),
        "replace with the same content"
    );
    assert_eq!(writes(&s, || s.replace([(9.0, 1.0), (8.0, 2.0)])), (1, 1));
    assert_eq!(points(&s), [(8.0, 2.0), (9.0, 1.0)]);
    assert_eq!(writes(&s, || s.clear()), (1, 1));
    assert_eq!(writes(&s, || s.clear()), (0, 0));
    // A setting on an empty series is stored without a write; on data it is one.
    let quiet = s.clone();
    assert_eq!(
        writes(&s, move || {
            let _ = quiet.gap_after(5.0);
        }),
        (0, 0)
    );
    assert_eq!(s.gap_after_secs(), Some(5.0));
    s.push(1.0, 1.0);
    let loud = s.clone();
    assert_eq!(
        writes(&s, move || {
            let _ = loud.gap_after(6.0);
        }),
        (1, 1)
    );
}

#[test]
fn clones_share_one_set_of_points_across_threads() {
    let _g = hold();
    let s = TimedSeries::new(4);
    let worker = s.clone();
    assert!(s.ptr_eq(&worker) && !s.ptr_eq(&TimedSeries::new(4)));
    std::thread::spawn(move || worker.push(1.5, 42.0))
        .join()
        .unwrap();
    assert_eq!(s.latest(), Some((1.5, 42.0)));
    let a = SeriesSource::from(s.clone());
    assert!(a.ptr_eq(&SeriesSource::from(s.clone())) && a.is_timed());
    assert!(!a.ptr_eq(&SeriesSource::from(Series::new(4))));
    assert_eq!((a.max(), a.revision()), (Some(42.0), s.revision()));
}

// ---- Geometry ----

/// 100 x 100 with values `0..10` at `y = 100 - 10 * v`.
fn plot() -> Rect {
    Rect::new(0.0, 0.0, 100.0, 100.0)
}

#[test]
fn the_time_span_ends_at_the_newest_point() {
    let pts = [(10.0, 1.0), (12.0, 2.0), (20.0, 3.0)];
    assert_eq!(time_span(&pts, None), Some((10.0, 20.0)));
    assert_eq!(time_span(&pts, Some(30.0)), Some((-10.0, 20.0)));
    assert_eq!(time_span(&pts, Some(f64::NAN)), Some((10.0, 20.0)));
    assert_eq!(time_span(&[], None), None);
    assert_eq!(time_span(&[(f64::NAN, 1.0)], None), None);

    assert_eq!(x_for_time(20.0, (10.0, 20.0), plot()), 100.0);
    assert_eq!(x_for_time(10.0, (10.0, 20.0), plot()), 0.0);
    assert_eq!(x_for_time(12.5, (10.0, 20.0), plot()), 25.0);
    // A single point sits at the right edge.
    assert_eq!(x_for_time(5.0, (5.0, 5.0), plot()), 100.0);
}

#[test]
fn timed_points_land_at_hand_computed_positions() {
    // Window 40 over newest 50: x = 100 - (50 - t) * 100 / 40.
    let pts = [(20.0, 1.0), (30.0, 2.0), (50.0, 4.0)];
    let line = timed_line_segments(
        &pts,
        Some(40.0),
        None,
        (0.0, 10.0),
        plot(),
        Orientation::Upright,
    );
    assert_eq!(line.polylines.len(), 1);
    let xy: Vec<(f64, f64)> = line.polylines[0].iter().map(|p| (p.x, p.y)).collect();
    assert_eq!(xy, [(25.0, 90.0), (50.0, 80.0), (100.0, 60.0)]);

    // Without a window the oldest point is at the left edge.
    let line = timed_line_segments(&pts, None, None, (0.0, 10.0), plot(), Orientation::Upright);
    let xs: Vec<f64> = line.polylines[0].iter().map(|p| p.x).collect();
    assert!(
        near(xs[0], 0.0) && near(xs[1], 100.0 / 3.0) && near(xs[2], 100.0),
        "{xs:?}"
    );

    // Inverted hangs from the top edge.
    let line = timed_line_segments(&pts, None, None, (0.0, 10.0), plot(), Orientation::Inverted);
    let ys: Vec<f64> = line.polylines[0].iter().map(|p| p.y).collect();
    assert_eq!(ys, [10.0, 20.0, 40.0]);
}

#[test]
fn a_pair_further_apart_than_gap_after_is_not_connected() {
    // Gaps of 8 and 10 s with a 5 s threshold: [0, 1, 2] [10] [20, 21].
    let pts = [
        (0.0, 1.0),
        (1.0, 1.0),
        (2.0, 1.0),
        (10.0, 5.0),
        (20.0, 2.0),
        (21.0, 2.0),
    ];
    let line = timed_line_segments(
        &pts,
        None,
        Some(5.0),
        (0.0, 10.0),
        plot(),
        Orientation::Upright,
    );
    let runs: Vec<usize> = line.polylines.iter().map(Vec::len).collect();
    assert_eq!(runs, [3, 2]);
    let lone_x = 100.0 - 11.0 * 100.0 / 21.0;
    assert_eq!(line.points.len(), 1);
    assert!(near(line.points[0].x, lone_x) && near(line.points[0].y, 50.0));
    // Exactly the threshold apart is still connected: 8 s joins, 10 s parts.
    let line = timed_line_segments(
        &pts,
        None,
        Some(8.0),
        (0.0, 10.0),
        plot(),
        Orientation::Upright,
    );
    assert_eq!((line.polylines.len(), line.points.len()), (2, 0));

    let areas = timed_area_paths(
        &pts,
        None,
        Some(5.0),
        (0.0, 10.0),
        plot(),
        Orientation::Upright,
    );
    assert_eq!(areas.len(), 3);
    let column = areas[1].bounding_box();
    assert!(near(column.width(), TIMED_COLUMN_WIDTH), "{column:?}");
    assert!(near(column.center().x, lone_x) && near(column.y0, 50.0) && near(column.y1, 100.0));
    // A run closes to the baseline between its own end points.
    let run = areas[0].bounding_box();
    assert!(near(run.x0, 0.0) && near(run.x1, 200.0 / 21.0) && near(run.y1, 100.0));
}

#[test]
fn an_isolated_column_is_kept_inside_the_plot() {
    let lone = [(7.0, 5.0)];
    let areas = timed_area_paths(&lone, None, None, (0.0, 10.0), plot(), Orientation::Upright);
    let column = areas[0].bounding_box();
    assert!(near(column.x1, 100.0) && near(column.x0, 100.0 - TIMED_COLUMN_WIDTH / 2.0));
    let line = timed_line_segments(&lone, None, None, (0.0, 10.0), plot(), Orientation::Upright);
    assert_eq!(line.points, [Point::new(100.0, 50.0)]);
}

#[test]
fn degenerate_inputs_yield_finite_or_empty_geometry() {
    let up = Orientation::Upright;
    let flat = Rect::new(0.0, 0.0, 100.0, 0.0);
    let pts = [(1.0, 1.0), (2.0, 2.0)];
    assert!(timed_line_segments(&pts, None, None, (0.0, 10.0), flat, up).is_empty());
    assert!(timed_area_paths(&[], None, None, (0.0, 10.0), plot(), up).is_empty());
    let wild = [
        (f64::NAN, 1.0),
        (-1e300, f32::MAX),
        (0.0, f32::NAN),
        (1e300, -f32::MAX),
        (1e300, 3.0),
    ];
    let line = timed_line_segments(&wild, Some(1e-9), Some(-1.0), (5.0, 5.0), plot(), up);
    for p in line.polylines.iter().flatten().chain(&line.points) {
        assert!(p.x.is_finite() && p.y.is_finite(), "{p:?}");
    }
}

// ---- time_series over timed data ----

#[derive(Clone, Debug)]
enum Op {
    Fill(Rect, Brush),
    Stroke(Vec<Point>),
    Rect(Rect, Color),
}

#[derive(Default)]
struct Rec {
    ops: Vec<Op>,
    depth: usize,
}

fn vertices(origin: Point, path: &BezPath) -> Vec<Point> {
    path.elements()
        .iter()
        .filter_map(|el| el.end_point())
        .map(|p| p + origin.to_vec2())
        .collect()
}

impl Rec {
    fn strokes(&self) -> Vec<Vec<Point>> {
        self.ops
            .iter()
            .filter_map(|op| match op {
                Op::Stroke(points) => Some(points.clone()),
                _ => None,
            })
            .collect()
    }

    fn fills(&self, gradient: bool) -> Vec<Rect> {
        self.ops
            .iter()
            .filter_map(|op| match op {
                Op::Fill(r, Brush::Gradient(_)) if gradient => Some(*r),
                Op::Fill(r, Brush::Solid(_)) if !gradient => Some(*r),
                _ => None,
            })
            .collect()
    }

    fn rects(&self, color: Color) -> Vec<Rect> {
        self.ops
            .iter()
            .filter_map(|op| match op {
                Op::Rect(r, c) if *c == color => Some(*r),
                _ => None,
            })
            .collect()
    }

    fn xs(&self, stroke: usize) -> Vec<f64> {
        self.strokes()[stroke].iter().map(|p| p.x).collect()
    }
}

impl PaintScene for Rec {
    fn fill_rect(&mut self, origin: Point, size: Size, color: Color) {
        self.ops
            .push(Op::Rect(Rect::from_origin_size(origin, size), color));
    }
    fn fill_rounded_rect(&mut self, origin: Point, size: Size, _radius: f64, color: Color) {
        self.fill_rect(origin, size, color);
    }
    fn fill_path(&mut self, origin: Point, path: &BezPath, brush: &Brush) {
        let rect = path.bounding_box() + origin.to_vec2();
        self.ops.push(Op::Fill(rect, brush.clone()));
    }
    fn stroke_path(&mut self, origin: Point, path: &BezPath, _width: f64, _brush: &Brush) {
        self.ops.push(Op::Stroke(vertices(origin, path)));
    }
    fn push_clip(&mut self, _origin: Point, _size: Size) {
        self.depth += 1;
    }
    fn pop_clip(&mut self) {
        self.depth -= 1;
    }
    fn draw_glyph_run(&mut self, _run: GlyphRun) {}
    fn draw_text(&mut self, _origin: Point, _text: &str) {}
}

type View = TimeSeriesView<()>;

struct Rig {
    rt: &'static ReactiveRuntime,
    root: RenderRoot<(), View>,
    text: TextContext,
}

impl Rig {
    fn new(view: impl Fn() -> View) -> Self {
        let mut rig = Self {
            rt: ReactiveRuntime::init(Arc::new(|| {})),
            root: RenderRoot::new(),
            text: TextContext::new(),
        };
        rig.rebuild(view);
        rig
    }

    fn rebuild(&mut self, view: impl Fn() -> View) -> ChangeFlags {
        let mut logic = |_: &mut ()| view();
        self.rt
            .with_owner(|| self.root.rebuild(&mut logic, &mut ()))
    }

    fn paint_at(&mut self, width: f64, height: f64, nanos: u64) -> Rec {
        self.root
            .layout_with_text(Size::new(width, height), &mut self.text as &mut dyn Any);
        let mut rec = Rec::default();
        self.root.paint(&mut rec, FrameTime::from_nanos(nanos));
        assert_eq!(rec.depth, 0, "every pushed clip is popped");
        rec
    }

    fn paint(&mut self, width: f64, height: f64) -> Rec {
        self.paint_at(width, height, 1_000_000_000)
    }
}

fn timed(series: TimedSeries, pts: &[(f64, f32)]) -> TimedSeries {
    series.extend(pts.iter().copied());
    series
}

#[test]
fn a_window_places_the_newest_point_at_the_right_edge() {
    let _g = hold();
    // Window 60 over newest 160: x = 300 - (160 - t) * 300 / 60.
    let s = timed(
        TimedSeries::windowed(60.0),
        &[(100.0, 10.0), (115.0, 20.0), (130.0, 30.0), (160.0, 40.0)],
    );
    let mut rig = Rig::new(|| {
        time_series()
            .style(PlotStyle::Line)
            .series(s.clone(), SeriesStyle::color(RED))
    });
    let rec = rig.paint(300.0, 50.0);
    assert_eq!(rec.strokes().len(), 1);
    assert_eq!(rec.xs(0), [0.0, 75.0, 150.0, 300.0]);
    let ys: Vec<f64> = rec.strokes()[0].iter().map(|p| p.y).collect();
    assert_eq!(ys, [45.0, 40.0, 35.0, 30.0]);
    assert!(rec.fills(true).is_empty());

    // Less history than the window starts partway across.
    let short = timed(TimedSeries::windowed(100.0), &[(50.0, 10.0), (60.0, 10.0)]);
    let mut rig = Rig::new(|| {
        time_series()
            .style(PlotStyle::Line)
            .series(short.clone(), SeriesStyle::color(RED))
    });
    assert_eq!(rig.paint(300.0, 50.0).xs(0), [270.0, 300.0]);
}

#[test]
fn without_a_window_the_points_span_the_plot() {
    let _g = hold();
    let s = timed(
        TimedSeries::new(16),
        &[(10.0, 1.0), (11.0, 1.0), (13.0, 1.0), (14.0, 1.0)],
    );
    let mut rig = Rig::new(|| {
        time_series()
            .style(PlotStyle::Line)
            .series(s.clone(), SeriesStyle::color(RED))
    });
    assert_eq!(rig.paint(300.0, 50.0).xs(0), [0.0, 75.0, 225.0, 300.0]);
}

#[test]
fn an_outage_breaks_the_area_and_a_lone_point_is_a_mark() {
    let _g = hold();
    let s = timed(
        TimedSeries::new(16).gap_after(5.0),
        &[
            (0.0, 10.0),
            (1.0, 10.0),
            (2.0, 10.0),
            (10.0, 50.0),
            (20.0, 20.0),
            (21.0, 20.0),
        ],
    );
    let mut rig = Rig::new(|| time_series().series(s.clone(), SeriesStyle::color(RED)));
    let rec = rig.paint(210.0, 50.0);
    // Two runs stroke; the lone sample is a dot and a narrow area column.
    assert_eq!(rec.strokes().len(), 2);
    assert_eq!(rec.xs(0), [0.0, 10.0, 20.0]);
    assert_eq!(rec.xs(1), [200.0, 210.0]);
    let areas = rec.fills(true);
    assert_eq!(areas.len(), 3, "{areas:?}");
    assert!(near(areas[1].width(), TIMED_COLUMN_WIDTH) && near(areas[1].center().x, 100.0));
    let dots = rec.fills(false);
    assert_eq!(dots.len(), 1);
    assert!(near(dots[0].center().x, 100.0) && near(dots[0].center().y, 25.0));
}

#[test]
fn a_push_then_paint_with_no_rebuild_updates_the_output() {
    let _g = hold();
    let s = timed(TimedSeries::new(16), &[(0.0, 10.0), (1.0, 20.0)]);
    let mut rig = Rig::new(|| {
        time_series()
            .style(PlotStyle::Line)
            .series(s.clone(), SeriesStyle::color(RED))
    });
    assert_eq!(rig.paint(300.0, 50.0).xs(0), [0.0, 300.0]);
    let writer = s.clone();
    assert_eq!(repaints(|| writer.push(3.0, 30.0)), 1);
    assert_eq!(rig.paint(300.0, 50.0).xs(0), [0.0, 100.0, 300.0]);
    // The same view rebuilt is no change: the data reaches the paint through the handle.
    let flags = rig.rebuild(|| {
        time_series()
            .style(PlotStyle::Line)
            .series(s.clone(), SeriesStyle::color(RED))
    });
    assert_eq!(flags, ChangeFlags::NONE);
}

#[test]
fn the_picture_stands_still_when_writes_stop() {
    let _g = hold();
    let s = timed(TimedSeries::windowed(30.0), &[(5.0, 10.0), (20.0, 20.0)]);
    let mut rig = Rig::new(|| {
        time_series()
            .style(PlotStyle::Line)
            .series(s.clone(), SeriesStyle::color(RED))
    });
    let early = rig.paint_at(300.0, 50.0, 1_000_000_000).strokes();
    let late = rig.paint_at(300.0, 50.0, 60_000_000_000).strokes();
    assert_eq!(early, late);
}

#[test]
fn blocks_fall_back_to_a_line_for_timed_data() {
    let _g = hold();
    let timed_series = timed(TimedSeries::new(8), &[(0.0, 10.0), (1.0, 20.0)]);
    let slots = Series::new(8);
    slots.extend([Some(10.0), Some(20.0)]);
    let mut rig = Rig::new(|| {
        time_series()
            .style(PlotStyle::Blocks {
                column_width: 4.0,
                gap: 1.0,
            })
            .series(slots.clone(), SeriesStyle::color(BLUE))
            .series(timed_series.clone(), SeriesStyle::color(RED))
    });
    let rec = rig.paint(300.0, 50.0);
    assert_eq!(
        rec.rects(BLUE).len(),
        2,
        "the slot series keeps its columns"
    );
    assert!(rec.rects(RED).is_empty());
    assert_eq!(rec.strokes().len(), 1);
    assert!(rec.fills(true).is_empty(), "a line has no fill");
}

#[test]
fn an_automatic_domain_follows_timed_data() {
    let _g = hold();
    let s = timed(TimedSeries::new(8), &[(0.0, 21.0), (1.0, 42.0)]);
    let mut rig = Rig::new(|| {
        time_series()
            .style(PlotStyle::Line)
            .domain(YDomain::auto_from_zero())
            .series(s.clone(), SeriesStyle::color(RED))
    });
    // 42 resolves to 0..50: y = 50 - v.
    let ys: Vec<f64> = rig.paint(300.0, 50.0).strokes()[0]
        .iter()
        .map(|p| p.y)
        .collect();
    assert!(near(ys[0], 29.0) && near(ys[1], 8.0), "{ys:?}");
}

#[test]
fn a_mirrored_pair_mixes_both_handles() {
    let _g = hold();
    let up = timed(TimedSeries::new(8), &[(0.0, 50.0), (2.0, 50.0)]);
    let down = Series::new(4);
    down.extend([Some(50.0), Some(50.0)]);
    let mut rig = Rig::new(|| {
        time_series().style(PlotStyle::Line).mirrored(
            (up.clone(), SeriesStyle::color(RED)),
            (down.clone(), SeriesStyle::color(BLUE)),
        )
    });
    let strokes = rig.paint(300.0, 56.0).strokes();
    assert_eq!(strokes.len(), 2);
    assert!(strokes[0].iter().all(|p| near(p.y, 14.0)), "{strokes:?}");
    assert_eq!(
        strokes[0].iter().map(|p| p.x).collect::<Vec<_>>(),
        [0.0, 300.0]
    );
    assert!(strokes[1].iter().all(|p| near(p.y, 42.0)), "{strokes:?}");
}

#[test]
fn a_gutter_narrows_the_time_axis_too() {
    let _g = hold();
    let s = timed(TimedSeries::new(8), &[(0.0, 10.0), (4.0, 60.0)]);
    let mut rig = Rig::new(|| {
        time_series()
            .style(PlotStyle::Line)
            .grid_color(GREY)
            .y_axis(YAxis::new().gutter(true))
            .series(s.clone(), SeriesStyle::color(RED))
    });
    let rec = rig.paint(300.0, 56.0);
    let gutter = rec.rects(GREY)[0].x0;
    assert!(gutter > 0.0, "{gutter}");
    let xs = rec.xs(0);
    assert!(
        (xs[0] - gutter).abs() < EPS && xs[1] == 300.0,
        "{xs:?} {gutter}"
    );
}

// ---- sparkline over timed data ----

fn spark(view: SparklineView, width: f64, height: f64) -> Rec {
    let mut root: RenderRoot<(), SparklineView> = RenderRoot::new();
    let mut view = Some(view);
    root.rebuild(&mut |_: &mut ()| view.take().expect("built once"), &mut ());
    root.layout(Size::new(width, height));
    let mut rec = Rec::default();
    root.paint(&mut rec, FrameTime::from_nanos(1_000_000_000));
    rec
}

#[test]
fn a_timed_sparkline_draws_a_line_in_any_style() {
    let _g = hold();
    let s = timed(
        TimedSeries::new(8).gap_after(2.0),
        &[(0.0, 0.0), (1.0, 100.0), (10.0, 50.0)],
    );
    // The default style is blocks: timed data still draws a line, inset by the 1 px dot.
    let rec = spark(sparkline(s.clone()).color(RED), 102.0, 24.0);
    assert!(
        rec.rects(RED).len() == 1,
        "only the lone dot is a rect: {:?}",
        rec.ops
    );
    let strokes = rec.strokes();
    assert_eq!(strokes.len(), 1);
    let xy: Vec<(f64, f64)> = strokes[0].iter().map(|p| (p.x, p.y)).collect();
    assert_eq!(xy, [(1.0, 23.0), (11.0, 1.0)]);
    let dot = rec.rects(RED)[0];
    assert!(
        near(dot.center().x, 101.0) && near(dot.center().y, 12.0),
        "{dot:?}"
    );

    let line = spark(
        sparkline(s.clone()).style(SparkStyle::Line { width: 2.0 }),
        102.0,
        24.0,
    );
    assert_eq!(line.strokes().len(), 1);
}

#[test]
fn a_timed_sparkline_follows_a_push() {
    let _g = hold();
    let s = timed(TimedSeries::new(8), &[(0.0, 10.0), (1.0, 20.0)]);
    let mut root: RenderRoot<(), SparklineView> = RenderRoot::new();
    let mut view = Some(sparkline(s.clone()).style(SparkStyle::line()));
    root.rebuild(&mut |_: &mut ()| view.take().expect("built once"), &mut ());
    root.layout(Size::new(102.0, 24.0));
    let mut paint = || {
        let mut rec = Rec::default();
        root.paint(&mut rec, FrameTime::from_nanos(1_000_000_000));
        rec.strokes()
    };
    assert_eq!(paint()[0].len(), 2);
    s.push(4.0, 30.0);
    let strokes = paint();
    assert_eq!(
        strokes[0].iter().map(|p| p.x).collect::<Vec<_>>(),
        [1.0, 26.0, 101.0]
    );
}
