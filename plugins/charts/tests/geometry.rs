//! Chart geometry: hand-computed coordinates for line runs, area paths, block columns,
//! snapping and grid rules, plus degenerate inputs and a seeded finiteness sweep.

use std::sync::{Mutex, PoisonError};

use frust::authoring::{BezPath, Point, Rect};
use frust::kurbo::PathEl;
use frust_charts::Series;
use frust_charts::geometry::{
    BlockColumn, LineGeometry, Orientation, area_paths, block_columns, fit_columns, grid_rule_ys,
    line_segments, polyline, slot_spacing, snap, snap_rule_y, x_for_age, x_for_slot, y_for,
    y_for_inverted,
};

/// Series writes request a process-wide repaint; tests that write one hold this.
static STATICS: Mutex<()> = Mutex::new(());

const UP: Orientation = Orientation::Upright;
const DOWN: Orientation = Orientation::Inverted;

/// 90 x 100 with capacity 10: slot `i` of a full series sits at `x = 10 * i`, and a value `v`
/// in `0..10` at `y = 100 - 10 * v`.
fn plot() -> Rect {
    Rect::new(0.0, 0.0, 90.0, 100.0)
}

fn p(x: f64, y: f64) -> Point {
    Point::new(x, y)
}

fn gappy() -> [Option<f32>; 10] {
    [
        Some(1.0),
        Some(2.0),
        None,
        None,
        Some(5.0),
        None,
        Some(7.0),
        Some(8.0),
        Some(9.0),
        None,
    ]
}

fn elements(path: &BezPath) -> Vec<PathEl> {
    path.elements().to_vec()
}

fn path_points(path: &BezPath) -> Vec<Point> {
    path.elements()
        .iter()
        .flat_map(|el| match *el {
            PathEl::MoveTo(a) | PathEl::LineTo(a) => vec![a],
            PathEl::QuadTo(a, b) => vec![a, b],
            PathEl::CurveTo(a, b, c) => vec![a, b, c],
            PathEl::ClosePath => vec![],
        })
        .collect()
}

#[test]
fn x_mapping_is_right_aligned() {
    let plot = plot();
    assert_eq!(slot_spacing(10, plot), 10.0);
    assert_eq!(x_for_age(0, 10, plot), 90.0);
    assert_eq!(x_for_age(9, 10, plot), 0.0);
    // A filling window: 3 of 10 slots occupy the right edge.
    let xs: Vec<f64> = (0..3).map(|i| x_for_slot(i, 3, 10, plot)).collect();
    assert_eq!(xs, [70.0, 80.0, 90.0]);
    // Capacity 1 is a single point at the right edge.
    assert_eq!(slot_spacing(1, plot), 0.0);
    assert_eq!(x_for_slot(0, 1, 1, plot), 90.0);
}

#[test]
fn partial_fill_line_starts_partway_across() {
    let line = line_segments(
        [Some(1.0), Some(2.0), Some(3.0)],
        10,
        (0.0, 10.0),
        plot(),
        UP,
    );
    assert_eq!(
        line.polylines,
        [vec![p(70.0, 90.0), p(80.0, 80.0), p(90.0, 70.0)]]
    );
    assert!(line.points.is_empty());
}

#[test]
fn y_mapping_clamps_and_inverts() {
    let plot = Rect::new(0.0, 20.0, 10.0, 120.0);
    let domain = (0.0, 50.0);
    assert_eq!(y_for(0.0, domain, plot), 120.0);
    assert_eq!(y_for(50.0, domain, plot), 20.0);
    assert_eq!(y_for(25.0, domain, plot), 70.0);
    assert_eq!(y_for(-10.0, domain, plot), 120.0);
    assert_eq!(y_for(99.0, domain, plot), 20.0);
    assert_eq!(y_for(f32::NAN, domain, plot), 120.0);
    assert_eq!(y_for_inverted(0.0, domain, plot), 20.0);
    assert_eq!(y_for_inverted(50.0, domain, plot), 120.0);
    assert_eq!(y_for_inverted(10.0, domain, plot), 40.0);
    assert_eq!(UP.baseline(plot), 120.0);
    assert_eq!(DOWN.baseline(plot), 20.0);
}

#[test]
fn gaps_split_runs_and_leave_isolated_points() {
    let line = line_segments(gappy(), 10, (0.0, 10.0), plot(), UP);
    assert_eq!(
        line,
        LineGeometry {
            polylines: vec![
                vec![p(0.0, 90.0), p(10.0, 80.0)],
                vec![p(60.0, 30.0), p(70.0, 20.0), p(80.0, 10.0)],
            ],
            points: vec![p(40.0, 50.0)],
        }
    );
    // No segment spans a gap: every polyline's x steps by exactly one slot.
    for run in &line.polylines {
        for pair in run.windows(2) {
            assert_eq!(pair[1].x - pair[0].x, 10.0);
        }
    }
    let paths = line.paths();
    assert_eq!(
        elements(&paths[0]),
        [PathEl::MoveTo(p(0.0, 90.0)), PathEl::LineTo(p(10.0, 80.0))]
    );
    assert!(elements(&polyline(&[])).is_empty());
}

#[test]
fn area_paths_close_to_the_runs_own_baseline_span() {
    let paths = area_paths(gappy(), 10, (0.0, 10.0), plot(), UP);
    assert_eq!(paths.len(), 3);
    assert_eq!(
        elements(&paths[0]),
        [
            PathEl::MoveTo(p(0.0, 90.0)),
            PathEl::LineTo(p(10.0, 80.0)),
            PathEl::LineTo(p(10.0, 100.0)),
            PathEl::LineTo(p(0.0, 100.0)),
            PathEl::ClosePath,
        ]
    );
    // The lone 5 is a one-slot column centred on its x.
    assert_eq!(
        elements(&paths[1]),
        [
            PathEl::MoveTo(p(35.0, 100.0)),
            PathEl::LineTo(p(35.0, 50.0)),
            PathEl::LineTo(p(45.0, 50.0)),
            PathEl::LineTo(p(45.0, 100.0)),
            PathEl::ClosePath,
        ]
    );
    assert_eq!(
        elements(&paths[2]),
        [
            PathEl::MoveTo(p(60.0, 30.0)),
            PathEl::LineTo(p(70.0, 20.0)),
            PathEl::LineTo(p(80.0, 10.0)),
            PathEl::LineTo(p(80.0, 100.0)),
            PathEl::LineTo(p(60.0, 100.0)),
            PathEl::ClosePath,
        ]
    );
}

#[test]
fn isolated_column_stays_inside_the_plot() {
    // The newest slot sits on the right edge, so its column keeps only its left half.
    let paths = area_paths([None, Some(5.0)], 10, (0.0, 10.0), plot(), UP);
    assert_eq!(
        elements(&paths[0]),
        [
            PathEl::MoveTo(p(85.0, 100.0)),
            PathEl::LineTo(p(85.0, 50.0)),
            PathEl::LineTo(p(90.0, 50.0)),
            PathEl::LineTo(p(90.0, 100.0)),
            PathEl::ClosePath,
        ]
    );
}

#[test]
fn mirrored_pair_inverts_line_and_area() {
    let line = line_segments(gappy(), 10, (0.0, 10.0), plot(), DOWN);
    assert_eq!(line.polylines[0], [p(0.0, 10.0), p(10.0, 20.0)]);
    assert_eq!(line.points, [p(40.0, 50.0)]);
    let paths = area_paths(gappy(), 10, (0.0, 10.0), plot(), DOWN);
    assert_eq!(
        elements(&paths[0]),
        [
            PathEl::MoveTo(p(0.0, 10.0)),
            PathEl::LineTo(p(10.0, 20.0)),
            PathEl::LineTo(p(10.0, 0.0)),
            PathEl::LineTo(p(0.0, 0.0)),
            PathEl::ClosePath,
        ]
    );
}

#[test]
fn only_the_newest_capacity_slots_are_drawn() {
    let line = line_segments([Some(3.0), Some(4.0)], 1, (0.0, 10.0), plot(), UP);
    assert!(line.polylines.is_empty());
    assert_eq!(line.points, [p(90.0, 60.0)]);
    let paths = area_paths([Some(3.0), Some(4.0)], 1, (0.0, 10.0), plot(), UP);
    assert_eq!(path_points(&paths[0])[0], p(45.0, 100.0));
}

#[test]
fn series_slots_feed_the_geometry() {
    let _hold = STATICS.lock().unwrap_or_else(PoisonError::into_inner);
    let series = Series::new(10);
    series.extend(gappy());
    series.push(4.0); // wraps the ring: the oldest 1 drops off
    let line = series.with_slots(|view| line_segments(view.iter(), 10, (0.0, 10.0), plot(), UP));
    assert_eq!(line.points, [p(0.0, 80.0), p(30.0, 50.0), p(90.0, 60.0)]);
    assert_eq!(
        line.polylines,
        [vec![p(50.0, 30.0), p(60.0, 20.0), p(70.0, 10.0)]]
    );
    let columns =
        series.with_slots(|view| block_columns(view.iter(), 10, 4.0, 1.0, (0.0, 10.0), plot(), UP));
    assert_eq!(columns.last().map(|c| c.value), Some(4.0));
}

fn column(x0: f64, y0: f64, x1: f64, y1: f64, value: f32) -> BlockColumn {
    BlockColumn {
        rect: Rect::new(x0, y0, x1, y1),
        value,
    }
}

#[test]
fn block_columns_right_align_skip_gaps_and_drop_the_oldest() {
    // 20 px fits 4 columns of 4 with 1 between: [1,5] [6,10] [11,15] [16,20].
    let plot = Rect::new(0.0, 0.0, 20.0, 100.0);
    assert_eq!(fit_columns(20.0, 4.0, 1.0), 4);
    let slots = [Some(1.0), Some(2.0), None, Some(4.0), Some(5.0), Some(6.0)];
    let columns = block_columns(slots, 10, 4.0, 1.0, (0.0, 10.0), plot, UP);
    assert_eq!(
        columns,
        [
            column(6.0, 60.0, 10.0, 100.0, 4.0),
            column(11.0, 50.0, 15.0, 100.0, 5.0),
            column(16.0, 40.0, 20.0, 100.0, 6.0),
        ]
    );
    let inverted = block_columns(slots, 10, 4.0, 1.0, (0.0, 10.0), plot, DOWN);
    assert_eq!(inverted[2], column(16.0, 0.0, 20.0, 60.0, 6.0));
    // Capacity bounds the slots considered, independently of what fits.
    let two = block_columns(slots, 2, 4.0, 1.0, (0.0, 10.0), plot, UP);
    assert_eq!(two.len(), 2);
}

#[test]
fn block_columns_keep_a_minimum_height() {
    let plot = Rect::new(0.0, 0.0, 20.0, 100.0);
    let columns = block_columns([Some(-5.0), Some(0.0)], 10, 4.0, 1.0, (0.0, 10.0), plot, UP);
    assert_eq!(
        columns,
        [
            column(11.0, 99.0, 15.0, 100.0, -5.0),
            column(16.0, 99.0, 20.0, 100.0, 0.0),
        ]
    );
    let down = block_columns([Some(0.0)], 10, 4.0, 1.0, (0.0, 10.0), plot, DOWN);
    assert_eq!(down, [column(16.0, 0.0, 20.0, 1.0, 0.0)]);
}

#[test]
fn fit_columns_matches_block_columns() {
    for width in [1.0, 4.0, 9.0, 19.0, 20.0, 37.0, 100.0] {
        for (column_width, gap) in [(4.0, 1.0), (1.0, 0.0), (3.0, 2.0), (7.0, 0.0)] {
            let plot = Rect::new(10.0, 0.0, 10.0 + width, 50.0);
            let full = vec![Some(1.0); 200];
            let laid = block_columns(full, 200, column_width, gap, (0.0, 2.0), plot, UP);
            assert_eq!(
                laid.len(),
                fit_columns(width, column_width, gap),
                "width {width}, column {column_width}, gap {gap}"
            );
        }
    }
    assert_eq!(fit_columns(19.0, 4.0, 1.0), 4);
    assert_eq!(fit_columns(18.0, 4.0, 1.0), 3);
    assert_eq!(fit_columns(3.0, 4.0, 1.0), 0);
    assert_eq!(fit_columns(10.0, 0.0, 1.0), 0);
    assert_eq!(fit_columns(f64::NAN, 4.0, 1.0), 0);
    assert_eq!(fit_columns(10.0, f64::INFINITY, 1.0), 0);
    assert_eq!(fit_columns(10.0, 2.0, f64::NAN), 5);
}

#[test]
fn snapping_lands_on_whole_pixels() {
    assert_eq!(snap(3.4), 3.0);
    assert_eq!(snap(3.6), 4.0);
    assert_eq!(snap(f64::NAN), 0.0);
    assert!(snap(-0.2).is_sign_positive());
    assert_eq!(snap_rule_y(10.3), 10.0);
    assert_eq!(snap_rule_y(10.9), 10.0);
    assert_eq!(snap_rule_y(11.2), 11.0);

    let plot = Rect::new(0.3, 0.4, 20.6, 100.4);
    let slots = [Some(1.3), None, Some(7.7), Some(0.0)];
    for orientation in [UP, DOWN] {
        let columns = block_columns(slots, 10, 3.6, 0.7, (0.0, 10.0), plot, orientation);
        assert_eq!(columns.len(), 3);
        for c in &columns {
            assert_eq!(c.rect.x0.fract(), 0.0);
            assert_eq!(c.rect.x1.fract(), 0.0);
            assert_eq!(c.rect.width(), 4.0);
            let baseline = match orientation {
                Orientation::Upright => c.rect.y1,
                Orientation::Inverted => c.rect.y0,
            };
            assert_eq!(baseline, snap(orientation.baseline(plot)));
        }
        assert_eq!(columns[2].rect.x1, 21.0);
    }
}

#[test]
fn grid_rules_snap_and_stay_inside() {
    let plot = Rect::new(0.0, 0.4, 10.0, 100.4);
    let ys = grid_rule_ys(&[0.0, 0.5, 1.0, 2.0, f32::NAN, -1.0], plot);
    assert_eq!(ys, [99.0, 50.0, 0.0, 0.0, 99.0]);
    assert!(ys.iter().all(|y| y.fract() == 0.0));
    assert!(grid_rule_ys(&[0.5], Rect::new(0.0, 0.0, 10.0, 0.0)).is_empty());
}

#[test]
fn degenerate_inputs_yield_empty_geometry() {
    let empty: [Option<f32>; 0] = [];
    let domain = (0.0, 10.0);
    let bad_plots = [
        Rect::new(0.0, 0.0, 0.0, 100.0),
        Rect::new(0.0, 0.0, 90.0, 0.0),
        Rect::new(90.0, 0.0, 0.0, 100.0),
        Rect::new(0.0, 100.0, 90.0, 0.0),
        Rect::new(f64::NAN, 0.0, 90.0, 100.0),
        Rect::new(0.0, 0.0, f64::INFINITY, 100.0),
    ];
    assert!(line_segments(empty, 10, domain, plot(), UP).is_empty());
    assert!(area_paths(empty, 10, domain, plot(), UP).is_empty());
    assert!(block_columns(empty, 10, 4.0, 1.0, domain, plot(), UP).is_empty());
    assert!(line_segments(gappy(), 0, domain, plot(), UP).is_empty());
    assert!(area_paths(gappy(), 0, domain, plot(), UP).is_empty());
    assert!(block_columns(gappy(), 0, 4.0, 1.0, domain, plot(), UP).is_empty());
    assert!(line_segments([None, Some(f32::NAN)], 10, domain, plot(), UP).is_empty());
    for bad in bad_plots {
        assert!(line_segments(gappy(), 10, domain, bad, UP).is_empty());
        assert!(area_paths(gappy(), 10, domain, bad, DOWN).is_empty());
        assert!(block_columns(gappy(), 10, 4.0, 1.0, domain, bad, UP).is_empty());
        assert!(grid_rule_ys(&[0.0, 1.0], bad).is_empty());
        assert_eq!(slot_spacing(10, bad), 0.0);
    }
    for (width, gap) in [(0.0, 1.0), (0.4, 1.0), (f64::NAN, 1.0), (-4.0, 0.0)] {
        assert!(block_columns(gappy(), 10, width, gap, domain, plot(), UP).is_empty());
    }
}

#[test]
fn unresolved_domains_are_normalised() {
    // A reversed domain is swapped, a degenerate or non-finite one widened.
    let reversed = line_segments(gappy(), 10, (10.0, 0.0), plot(), UP);
    assert_eq!(
        reversed,
        line_segments(gappy(), 10, (0.0, 10.0), plot(), UP)
    );
    for domain in [
        (3.0, 3.0),
        (f32::NAN, f32::NAN),
        (0.0, f32::INFINITY),
        (-f32::MAX, f32::MAX),
    ] {
        let line = line_segments(gappy(), 10, domain, plot(), UP);
        assert_eq!(line.polylines.len(), 2);
        assert!(line.polylines.iter().flatten().all(|q| q.is_finite()));
        let y = y_for(5.0, domain, plot());
        assert!(
            y.is_finite() && (0.0..=100.0).contains(&y),
            "{domain:?} -> {y}"
        );
    }
}

/// A fixed-seed xorshift64 generator, so the sweep needs no dependency.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    /// A uniform-ish value in `lo..hi`.
    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (self.next() >> 11) as f64 / (1u64 << 53) as f64 * (hi - lo)
    }

    fn value(&mut self) -> f32 {
        match self.below(12) {
            0 => f32::NAN,
            1 => f32::INFINITY,
            2 => f32::MAX,
            3 => -f32::MAX,
            _ => self.range(-500.0, 500.0) as f32,
        }
    }

    fn slot(&mut self) -> Option<f32> {
        (self.below(4) != 0).then(|| self.value())
    }
}

#[test]
fn every_coordinate_is_finite_over_a_seeded_sweep() {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    for _ in 0..2000 {
        let len = rng.below(40) as usize;
        let slots: Vec<Option<f32>> = (0..len).map(|_| rng.slot()).collect();
        let capacity = rng.below(45) as usize;
        let (x0, y0) = (rng.range(-1e4, 1e4), rng.range(-1e4, 1e4));
        let plot = Rect::new(
            x0,
            y0,
            x0 + rng.range(-50.0, 2000.0),
            y0 + rng.range(-50.0, 2000.0),
        );
        let domain = (rng.value(), rng.value());
        let orientation = if rng.below(2) == 0 { UP } else { DOWN };
        let (column_width, gap) = (rng.range(-2.0, 20.0), rng.range(-2.0, 6.0));

        let line = line_segments(slots.iter().copied(), capacity, domain, plot, orientation);
        assert!(line.points.iter().all(|q| q.is_finite()));
        for path in line.paths() {
            assert!(path_points(&path).iter().all(|q| q.is_finite()));
        }
        for path in area_paths(slots.iter().copied(), capacity, domain, plot, orientation) {
            assert!(path_points(&path).iter().all(|q| q.is_finite()));
        }
        let columns = block_columns(
            slots.iter().copied(),
            capacity,
            column_width,
            gap,
            domain,
            plot,
            orientation,
        );
        for c in &columns {
            let r = c.rect;
            assert!([r.x0, r.y0, r.x1, r.y1].iter().all(|v| v.is_finite()));
            assert!(
                r.height() >= 1.0 - 1e-9 && r.x0.fract() == 0.0 && r.width() == snap(column_width)
            );
        }
        let fractions = [0.0, 0.25, rng.value(), 1.0];
        assert!(grid_rule_ys(&fractions, plot).iter().all(|y| y.is_finite()));
        let _ = fit_columns(plot.width(), column_width, gap);
    }
}
