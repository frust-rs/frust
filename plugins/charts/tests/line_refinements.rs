//! Line refinements: monotone smoothing and threshold colouring along a line.

use frust::Color;
use frust::authoring::{BezPath, Point, Rect};
use frust::kurbo::{CubicBez, ParamCurve, PathEl};
use frust_charts::Tone;
use frust_charts::geometry::{
    LineGeometry, Orientation, line_segments, smooth_polyline, tone_gradient,
};

const CALM: Color = Color::from_rgb8(0x2e, 0x9e, 0x5b);
const BUSY: Color = Color::from_rgb8(0xe0, 0xa3, 0x1a);
const CRITICAL: Color = Color::from_rgb8(0xd6, 0x3b, 0x3b);

fn pt(x: f64, y: f64) -> Point {
    Point::new(x, y)
}

/// The cubic segments of `path` with their start points.
fn cubics(path: &BezPath) -> Vec<CubicBez> {
    let mut start = Point::ZERO;
    let mut out = Vec::new();
    for el in path.elements() {
        match *el {
            PathEl::MoveTo(p) => start = p,
            PathEl::LineTo(p) => {
                out.push(CubicBez::new(start, start, p, p));
                start = p;
            }
            PathEl::CurveTo(a, b, c) => {
                out.push(CubicBez::new(start, a, b, c));
                start = c;
            }
            _ => {}
        }
    }
    out
}

#[test]
fn a_smoothed_path_passes_through_every_sample() {
    let points = [pt(0.0, 50.0), pt(10.0, 40.0), pt(20.0, 45.0), pt(35.0, 5.0)];
    let path = smooth_polyline(&points);
    let segments = cubics(&path);
    assert_eq!(segments.len(), 3);
    for (i, seg) in segments.iter().enumerate() {
        assert_eq!(seg.p0, points[i]);
        assert_eq!(seg.p3, points[i + 1]);
    }
}

#[test]
fn monotone_data_never_overshoots_between_samples() {
    let points = [
        pt(0.0, 90.0),
        pt(10.0, 88.0),
        pt(20.0, 30.0),
        pt(30.0, 28.0),
        pt(40.0, 5.0),
        pt(55.0, 4.0),
    ];
    for seg in cubics(&smooth_polyline(&points)) {
        let (lo, hi) = (seg.p0.y.min(seg.p3.y), seg.p0.y.max(seg.p3.y));
        for k in 0..=200 {
            let y = seg.eval(f64::from(k) / 200.0).y;
            assert!(y >= lo - 1e-9 && y <= hi + 1e-9, "{y} outside {lo}..{hi}");
        }
    }
}

#[test]
fn a_local_extremum_stays_flat_instead_of_overshooting() {
    let points = [pt(0.0, 50.0), pt(10.0, 10.0), pt(20.0, 50.0)];
    for seg in cubics(&smooth_polyline(&points)) {
        for k in 0..=100 {
            assert!(seg.eval(f64::from(k) / 100.0).y >= 10.0 - 1e-9);
        }
    }
}

#[test]
fn two_points_stay_a_straight_segment() {
    let path = smooth_polyline(&[pt(0.0, 0.0), pt(10.0, 5.0)]);
    assert_eq!(
        path.elements(),
        &[PathEl::MoveTo(pt(0.0, 0.0)), PathEl::LineTo(pt(10.0, 5.0))]
    );
    assert!(smooth_polyline(&[]).elements().is_empty());
}

#[test]
fn gaps_still_break_the_smoothed_path() {
    let slots = [
        Some(10.0),
        Some(20.0),
        Some(15.0),
        None,
        Some(40.0),
        Some(30.0),
        None,
        Some(50.0),
    ];
    let geometry: LineGeometry = line_segments(
        slots,
        8,
        (0.0, 100.0),
        Rect::new(0.0, 0.0, 70.0, 100.0),
        Orientation::Upright,
    );
    let paths = geometry.smooth_paths();
    assert_eq!(paths.len(), 2);
    assert_eq!(geometry.points.len(), 1);
    let moves = |p: &BezPath| {
        p.elements()
            .iter()
            .filter(|e| matches!(e, PathEl::MoveTo(_)))
            .count()
    };
    assert!(paths.iter().all(|p| moves(p) == 1));
}

#[test]
fn tone_stops_sit_at_the_thresholds_of_a_zero_to_hundred_domain() {
    let tone = Tone::load(CALM, BUSY, CRITICAL);
    let plot = Rect::new(0.0, 0.0, 100.0, 200.0);
    let g = tone_gradient(&tone, (0.0, 100.0), plot, Orientation::Upright, 1.0);
    assert_eq!((g.start_y, g.end_y), (200.0, 0.0));
    let offsets: Vec<f32> = g.stops.iter().map(|s| s.0).collect();
    assert_eq!(offsets, [0.0, 0.6, 0.6, 0.85, 0.85, 1.0]);
    let colours: Vec<Color> = g.stops.iter().map(|s| s.1).collect();
    assert_eq!(colours, [CALM, CALM, BUSY, BUSY, CRITICAL, CRITICAL]);
}

#[test]
fn an_inverted_half_runs_from_the_top_edge() {
    let tone = Tone::load(CALM, BUSY, CRITICAL);
    let plot = Rect::new(0.0, 10.0, 100.0, 110.0);
    let g = tone_gradient(&tone, (0.0, 100.0), plot, Orientation::Inverted, 1.0);
    assert_eq!((g.start_y, g.end_y), (10.0, 110.0));
    assert_eq!(g.stops[1].0, 0.6);
}

#[test]
fn thresholds_outside_the_domain_clamp_and_alpha_scales() {
    let tone = Tone::load(CALM, BUSY, CRITICAL);
    let plot = Rect::new(0.0, 0.0, 100.0, 100.0);
    let g = tone_gradient(&tone, (0.0, 50.0), plot, Orientation::Upright, 0.5);
    let offsets: Vec<f32> = g.stops.iter().map(|s| s.0).collect();
    assert_eq!(offsets, [0.0, 1.0, 1.0, 1.0, 1.0, 1.0]);
    assert!((g.stops[0].1.components[3] - 0.5).abs() < 1e-6);
    let bare = tone_gradient(
        &Tone::new(CALM),
        (0.0, 50.0),
        plot,
        Orientation::Upright,
        1.0,
    );
    assert_eq!(bare.stops.len(), 2);
    assert!(bare.stops.iter().all(|s| s.1 == CALM));
}

#[test]
fn a_changed_domain_moves_the_stops() {
    let tone = Tone::load(CALM, BUSY, CRITICAL);
    let plot = Rect::new(0.0, 0.0, 100.0, 100.0);
    let a = tone_gradient(&tone, (0.0, 100.0), plot, Orientation::Upright, 1.0);
    let b = tone_gradient(&tone, (0.0, 200.0), plot, Orientation::Upright, 1.0);
    assert_ne!(a, b);
    assert_eq!(b.stops[1].0, 0.3);
}
