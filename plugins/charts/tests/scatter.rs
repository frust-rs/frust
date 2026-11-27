//! `scatter_chart` headless: a real `RenderRoot` painting into a recording scene.
//!
//! Writing a `Points` handle requests a process-wide repaint, so every test holds `STATICS`.

use std::any::Any;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use frust::authoring::scene::GlyphRun;
use frust::authoring::text::TextContext;
use frust::authoring::{
    BezPath, Brush, ChangeFlags, EventOutcome, InputEvent, PaintScene, Point, PointerButton,
    PointerEvent, PointerPhase, Rect, Role, Shape, Size,
};
use frust::kurbo::PathEl;
use frust::{Color, FrameTime, Theme};
use frust_charts::{
    Points, ScatterChartView, ScatterPoint, ScatterShape, Tone, YDomain, scatter_chart,
};
use frust_core::RenderRoot;
use frust_reactive::ReactiveRuntime;

static STATICS: Mutex<()> = Mutex::new(());

fn hold() -> MutexGuard<'static, ()> {
    STATICS.lock().unwrap_or_else(PoisonError::into_inner)
}

const RED: Color = Color::from_rgb8(0xd0, 0x10, 0x10);
const BLUE: Color = Color::from_rgb8(0x10, 0x20, 0xd0);
const GREEN: Color = Color::from_rgb8(0x10, 0xa0, 0x30);
const EPS: f64 = 1e-6;

#[derive(Clone, Debug)]
enum Op {
    Path(BezPath, Color),
    Rect(Rect, Color),
}

#[derive(Default)]
struct Rec {
    ops: Vec<(Op, usize)>,
    clips: Vec<Rect>,
    clip_depth: usize,
}

impl Rec {
    /// Every path fill: the colour and the bounding box of each subpath, in order.
    fn marks(&self) -> Vec<(Color, Vec<Rect>)> {
        self.ops
            .iter()
            .filter_map(|(op, _)| match op {
                Op::Path(path, color) => Some((*color, subpath_boxes(path))),
                Op::Rect(..) => None,
            })
            .collect()
    }

    fn mark_count(&self) -> usize {
        self.marks().iter().map(|(_, boxes)| boxes.len()).sum()
    }

    fn rules(&self) -> Vec<(Rect, Color)> {
        self.ops
            .iter()
            .filter_map(|(op, _)| match op {
                Op::Rect(r, c) => Some((*r, *c)),
                Op::Path(..) => None,
            })
            .collect()
    }
}

fn subpath_boxes(path: &BezPath) -> Vec<Rect> {
    let mut parts: Vec<Vec<PathEl>> = Vec::new();
    for el in path.elements() {
        if matches!(el, PathEl::MoveTo(_)) {
            parts.push(Vec::new());
        }
        parts
            .last_mut()
            .expect("a path starts with MoveTo")
            .push(*el);
    }
    parts
        .into_iter()
        .map(|els| BezPath::from_vec(els).bounding_box())
        .collect()
}

impl PaintScene for Rec {
    fn fill_rect(&mut self, origin: Point, size: Size, color: Color) {
        let rect = Rect::from_origin_size(origin, size);
        self.ops.push((Op::Rect(rect, color), self.clip_depth));
    }
    fn fill_rounded_rect(&mut self, _origin: Point, _size: Size, _radius: f64, _color: Color) {
        panic!("a scatter chart draws no rounded rect");
    }
    fn fill_path(&mut self, origin: Point, path: &BezPath, brush: &Brush) {
        let Brush::Solid(color) = brush else {
            panic!("expected a solid brush, got {brush:?}");
        };
        let mut moved = path.clone();
        moved.apply_affine(frust::kurbo::Affine::translate(origin.to_vec2()));
        self.ops.push((Op::Path(moved, *color), self.clip_depth));
    }
    fn stroke_path(&mut self, _origin: Point, _path: &BezPath, _width: f64, _brush: &Brush) {
        panic!("a scatter chart strokes nothing");
    }
    fn push_clip(&mut self, origin: Point, size: Size) {
        self.clips.push(Rect::from_origin_size(origin, size));
        self.clip_depth += 1;
    }
    fn pop_clip(&mut self) {
        self.clip_depth -= 1;
    }
    fn draw_glyph_run(&mut self, _run: GlyphRun) {
        panic!("a scatter chart draws no text");
    }
    fn draw_text(&mut self, _origin: Point, _text: &str) {
        panic!("a scatter chart draws no text");
    }
}

type View = ScatterChartView<()>;

struct Rig {
    rt: &'static ReactiveRuntime,
    root: RenderRoot<(), View>,
    text: TextContext,
}

impl Rig {
    fn new() -> Self {
        Self {
            rt: ReactiveRuntime::init(Arc::new(|| {})),
            root: RenderRoot::new(),
            text: TextContext::new(),
        }
    }

    fn rebuild(&mut self, view: impl Fn() -> View) -> ChangeFlags {
        let mut logic = |_: &mut ()| view();
        self.rt
            .with_owner(|| self.root.rebuild(&mut logic, &mut ()))
    }

    fn paint(&mut self, width: f64, height: f64) -> Rec {
        self.root
            .layout_with_text(Size::new(width, height), &mut self.text as &mut dyn Any);
        let mut rec = Rec::default();
        self.root
            .paint(&mut rec, FrameTime::from_nanos(1_000_000_000));
        assert_eq!(rec.clip_depth, 0, "every pushed clip is popped");
        rec
    }
}

fn rig(view: impl Fn() -> View) -> Rig {
    let mut rig = Rig::new();
    rig.rebuild(view);
    rig
}

fn points_of(capacity: usize, points: &[(f32, f32)]) -> Points {
    let handle = Points::new(capacity);
    handle.extend(points.iter().copied().map(ScatterPoint::from));
    handle
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 0.05
}

fn assert_box(rect: Rect, expected: [f64; 4]) {
    let got = [rect.x0, rect.y0, rect.x1, rect.y1];
    assert!(
        got.iter().zip(expected).all(|(g, e)| near(*g, e)),
        "{got:?} != {expected:?}"
    );
}

fn fixed(view: View) -> View {
    view.x_domain(YDomain::fixed(0.0, 100.0))
        .y_domain(YDomain::fixed(0.0, 100.0))
}

#[test]
fn circles_sit_at_their_mapped_positions() {
    let _g = hold();
    let p = points_of(8, &[(50.0, 50.0), (25.0, 75.0), (100.0, 0.0)]);
    let mut rig = rig(|| fixed(scatter_chart(p.clone()).radius(4.0)));
    let rec = rig.paint(200.0, 100.0);
    let marks = rec.marks();
    assert_eq!(marks.len(), 1);
    let boxes = &marks[0].1;
    assert_eq!(boxes.len(), 3);
    // The data area is the 200 x 100 plot inset by the radius (4): x runs 4..196, y runs
    // bottom to top over 4..96.
    assert_box(boxes[0], [96.0, 46.0, 104.0, 54.0]);
    assert_box(boxes[1], [48.0, 23.0, 56.0, 31.0]);
    assert_box(boxes[2], [192.0, 92.0, 200.0, 100.0]);
}

#[test]
fn squares_and_crosses_land_on_whole_pixels() {
    let _g = hold();
    let p = points_of(8, &[(50.3, 50.3)]);
    let mut squares = rig(|| {
        fixed(
            scatter_chart(p.clone())
                .shape(ScatterShape::Square)
                .radius(3.0),
        )
    });
    let rec = squares.paint(201.0, 99.0);
    let boxes = &rec.marks()[0].1;
    assert_eq!(boxes.len(), 1);
    let b = boxes[0];
    for v in [b.x0, b.y0, b.x1, b.y1] {
        assert!((v - v.round()).abs() < EPS, "{b:?}");
    }
    assert!(near(b.width(), 6.0) && near(b.height(), 6.0), "{b:?}");

    let mut crosses = rig(|| {
        fixed(
            scatter_chart(p.clone())
                .shape(ScatterShape::Cross)
                .radius(4.0),
        )
    });
    let rec = crosses.paint(201.0, 99.0);
    let boxes = &rec.marks()[0].1;
    assert_eq!(boxes.len(), 2, "two bars");
    for b in boxes {
        for v in [b.x0, b.y0, b.x1, b.y1] {
            assert!((v - v.round()).abs() < EPS, "{b:?}");
        }
    }
    let (wide, tall) = (boxes[0], boxes[1]);
    assert!(near(wide.width(), 8.0) && near(tall.height(), 8.0));
    assert!(wide.height() >= 1.0 && tall.width() >= 1.0);
}

#[test]
fn points_outside_a_domain_are_skipped_not_clamped() {
    let _g = hold();
    let p = points_of(
        8,
        &[
            (5.0, 5.0),
            (11.0, 5.0),
            (-1.0, 5.0),
            (5.0, 10.5),
            (5.0, -0.5),
            (10.0, 10.0),
        ],
    );
    let mut rig = rig(|| {
        scatter_chart(p.clone())
            .x_domain(YDomain::fixed(0.0, 10.0))
            .y_domain(YDomain::fixed(0.0, 10.0))
    });
    let rec = rig.paint(100.0, 100.0);
    assert_eq!(rec.mark_count(), 2);
}

#[test]
fn auto_domains_span_the_data() {
    let _g = hold();
    // Data 0..100 on both axes resolves to 0..100, so the extremes reach the corners of the
    // data area, which is the plot inset by the radius.
    let p = points_of(8, &[(0.0, 0.0), (100.0, 100.0)]);
    let mut rig = rig(|| scatter_chart(p.clone()).radius(2.0));
    let rec = rig.paint(200.0, 100.0);
    let boxes = &rec.marks()[0].1;
    assert_eq!(boxes.len(), 2);
    assert_box(boxes[0], [0.0, 96.0, 4.0, 100.0]);
    assert_box(boxes[1], [196.0, 0.0, 200.0, 4.0]);
}

#[test]
fn each_point_may_set_its_radius_and_colour() {
    let _g = hold();
    let p = Points::new(8);
    p.extend([
        ScatterPoint::new(20.0, 50.0),
        ScatterPoint::new(40.0, 50.0).with_radius(6.0).with_color(1),
        ScatterPoint::new(60.0, 50.0).with_color(0),
        ScatterPoint::new(80.0, 50.0).with_color(3),
    ]);
    let mut rig = rig(|| {
        fixed(
            scatter_chart(p.clone())
                .radius(2.0)
                .color(GREEN)
                .colors([RED, BLUE]),
        )
    });
    let marks = rig.paint(100.0, 100.0).marks();
    let colours: Vec<Color> = marks.iter().map(|(c, _)| *c).collect();
    // Order of first appearance: the plain colour, blue (index 1), red (0); index 3 wraps to 1.
    assert_eq!(colours, [GREEN, BLUE, RED]);
    assert_eq!(marks[0].1.len(), 1);
    assert!(near(marks[0].1[0].width(), 4.0));
    assert_eq!(marks[1].1.len(), 2);
    assert!(near(marks[1].1[0].width(), 12.0), "{:?}", marks[1].1);
    assert!(near(marks[1].1[1].width(), 4.0));
    assert_eq!(marks[2].1.len(), 1);
}

#[test]
fn tone_by_y_colours_points_without_a_palette_colour() {
    let _g = hold();
    let tone = Tone::load(GREEN, Color::from_rgb8(0xe0, 0xa0, 0x10), RED);
    let p = Points::new(8);
    p.extend([
        ScatterPoint::new(10.0, 10.0),
        ScatterPoint::new(20.0, 90.0),
        ScatterPoint::new(30.0, 10.0).with_color(0),
    ]);
    let mut rig = rig(|| {
        fixed(
            scatter_chart(p.clone())
                .tone_by_y(tone.clone())
                .colors([BLUE]),
        )
    });
    let colours: Vec<Color> = rig
        .paint(100.0, 100.0)
        .marks()
        .iter()
        .map(|(c, _)| *c)
        .collect();
    assert_eq!(colours, [GREEN, RED, BLUE]);
}

#[test]
fn points_of_one_colour_are_one_path() {
    let _g = hold();
    let p = Points::new(1_000);
    p.extend(
        (0..600).map(|i| ScatterPoint::new((i % 100) as f32, (i / 6) as f32).with_color(i % 2)),
    );
    let mut rig = rig(|| fixed(scatter_chart(p.clone()).colors([RED, BLUE])));
    let rec = rig.paint(300.0, 100.0);
    let marks = rec.marks();
    assert_eq!(marks.len(), 2, "one path per colour");
    assert_eq!(rec.mark_count(), 600);
}

#[test]
fn replacing_then_painting_updates_without_a_rebuild() {
    let _g = hold();
    let p = points_of(8, &[(50.0, 50.0)]);
    let mut rig = rig(|| fixed(scatter_chart(p.clone()).radius(2.0)));
    let first = rig.paint(100.0, 100.0);
    assert_box(first.marks()[0].1[0], [48.0, 48.0, 52.0, 52.0]);
    // An unchanged frame draws the same thing.
    assert_eq!(
        format!("{:?}", rig.paint(100.0, 100.0).ops),
        format!("{:?}", first.ops)
    );

    p.replace([ScatterPoint::new(10.0, 10.0), ScatterPoint::new(90.0, 90.0)]);
    // No rebuild: the same widget paints the new points.
    let second = rig.paint(100.0, 100.0);
    let boxes = &second.marks()[0].1;
    assert_eq!(boxes.len(), 2);
    assert_box(boxes[0], [9.6, 86.4, 13.6, 90.4]);

    p.push(50.0, 50.0);
    assert_eq!(rig.paint(100.0, 100.0).mark_count(), 3);
    p.clear();
    assert!(rig.paint(100.0, 100.0).marks().is_empty());

    // A resize rebuilds the geometry for the new plot.
    p.replace([ScatterPoint::new(50.0, 50.0)]);
    let wide = rig.paint(200.0, 100.0);
    assert_box(wide.marks()[0].1[0], [98.0, 48.0, 102.0, 52.0]);
}

#[test]
fn input_is_ignored() {
    let _g = hold();
    let p = points_of(4, &[(1.0, 1.0), (2.0, 2.0)]);
    let mut rig = rig(|| scatter_chart(p.clone()));
    rig.paint(200.0, 100.0);
    for phase in [PointerPhase::Down, PointerPhase::Up] {
        let event = InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(100.0, 50.0),
            button: PointerButton::Primary,
        });
        let outcome: EventOutcome = rig.root.event(&mut (), &event);
        assert!(!outcome.handled);
    }
}

#[test]
fn every_draw_is_clipped_to_the_widget_and_marks_stay_inside_or_clip() {
    let _g = hold();
    // Points on the edges are drawn whole, inside the one clip.
    let p = points_of(8, &[(0.0, 0.0), (100.0, 100.0), (50.0, 50.0)]);
    let mut rig = rig(|| {
        fixed(
            scatter_chart(p.clone())
                .radius(5.0)
                .grid(&[0.0, 1.0], &[0.0, 1.0]),
        )
    });
    let rec = rig.paint(200.0, 100.0);
    let bounds = Rect::from_origin_size(Point::ZERO, Size::new(200.0, 100.0));
    assert_eq!(rec.clips, [bounds]);
    assert!(rec.ops.iter().all(|(_, depth)| *depth == 1));
    for (rect, _) in rec.rules() {
        assert!(
            rect.x0 >= -EPS && rect.y0 >= -EPS && rect.x1 <= 200.0 + EPS && rect.y1 <= 100.0 + EPS,
            "{rect:?}"
        );
    }
}

#[test]
fn points_on_the_range_edge_are_drawn_whole_and_outside_ones_are_clipped() {
    let _g = hold();
    let edge = [
        (0.0, 0.0),
        (100.0, 0.0),
        (0.0, 100.0),
        (100.0, 100.0),
        (50.0, 50.0),
    ];
    for shape in [
        ScatterShape::Circle,
        ScatterShape::Square,
        ScatterShape::Cross,
    ] {
        let mut data = edge.to_vec();
        data.push((100.5, 50.0));
        data.push((50.0, -0.5));
        let p = points_of(16, &data);
        let mut rig = rig(|| fixed(scatter_chart(p.clone()).shape(shape).radius(5.0)));
        let rec = rig.paint(200.0, 100.0);
        let boxes: Vec<Rect> = rec.marks().into_iter().flat_map(|(_, b)| b).collect();
        // Out-of-range points are skipped; a cross is two bars per point.
        let per_point = if shape == ScatterShape::Cross { 2 } else { 1 };
        assert_eq!(boxes.len(), edge.len() * per_point, "{shape:?}");
        for b in boxes {
            assert!(
                b.x0 >= -EPS && b.y0 >= -EPS && b.x1 <= 200.0 + EPS && b.y1 <= 100.0 + EPS,
                "{shape:?} mark {b:?} reaches outside the 200 x 100 widget"
            );
        }
    }
}

#[test]
fn grid_rules_sit_on_whole_pixels() {
    let _g = hold();
    let p = points_of(4, &[(1.0, 1.0)]);
    let mut rig = rig(|| scatter_chart(p.clone()).grid(&[0.0, 0.5, 1.0], &[0.0, 0.5]));
    let rec = rig.paint(200.0, 100.0);
    let rules = rec.rules();
    assert_eq!(rules.len(), 5);
    let vertical: Vec<f64> = rules
        .iter()
        .filter(|(r, _)| near(r.width(), 1.0) && near(r.height(), 100.0))
        .map(|(r, _)| r.x0)
        .collect();
    assert_eq!(vertical, [0.0, 100.0, 199.0]);
    let horizontal: Vec<f64> = rules
        .iter()
        .filter(|(r, _)| near(r.width(), 200.0))
        .map(|(r, _)| r.y0)
        .collect();
    assert_eq!(horizontal, [50.0, 99.0]);
}

#[test]
fn colours_come_from_the_caller_then_the_theme_then_the_fallback() {
    let _g = hold();
    let p = points_of(4, &[(1.0, 1.0)]);
    let colour = |rig: &mut Rig| rig.paint(100.0, 100.0).marks()[0].0;

    let mut unthemed = rig(|| scatter_chart(p.clone()));
    assert_eq!(colour(&mut unthemed), Tone::FALLBACK_CALM);

    let theme = Theme::neutral();
    let primary = theme.scheme().primary;
    let mut themed = Rig::new();
    themed.root.set_theme(Box::new(theme));
    themed.rebuild(|| scatter_chart(p.clone()));
    assert_eq!(colour(&mut themed), primary);

    let mut explicit = Rig::new();
    explicit.root.set_theme(Box::new(Theme::neutral()));
    explicit.rebuild(|| scatter_chart(p.clone()).color(RED));
    assert_eq!(colour(&mut explicit), RED);
}

#[test]
fn semantics_is_one_image_node_only_when_labelled() {
    let _g = hold();
    let p = Points::new(4);
    let images = |rig: &Rig| {
        rig.root
            .semantics()
            .nodes
            .into_iter()
            .filter(|(_, n)| n.role() == Role::Image)
            .map(|(_, n)| n.label().map(str::to_owned))
            .collect::<Vec<_>>()
    };
    let mut plain = rig(|| scatter_chart(p.clone()));
    plain.paint(100.0, 100.0);
    assert!(images(&plain).is_empty());
    let mut labelled = rig(|| scatter_chart(p.clone()).semantics("Latency"));
    labelled.paint(100.0, 100.0);
    assert_eq!(images(&labelled), [Some("Latency".to_owned())]);
}

#[test]
fn rebuild_reports_the_least_work() {
    let _g = hold();
    let p = points_of(4, &[(1.0, 1.0)]);
    let base = || scatter_chart(p.clone());
    let mut rig = rig(base);
    assert_eq!(rig.rebuild(base), ChangeFlags::NONE);
    assert_eq!(rig.rebuild(|| base().radius(5.0)), ChangeFlags::PAINT);
    assert_eq!(
        rig.rebuild(|| base().radius(5.0).shape(ScatterShape::Square)),
        ChangeFlags::PAINT
    );
    assert_eq!(
        rig.rebuild(|| base().height(80.0)),
        ChangeFlags::LAYOUT | ChangeFlags::PAINT
    );
    let other = Points::new(4);
    assert_eq!(
        rig.rebuild(|| scatter_chart(other.clone()).height(80.0)),
        ChangeFlags::PAINT
    );
}

#[test]
fn points_handle_is_bounded_and_counts_changes() {
    let _g = hold();
    let p = Points::new(3);
    assert!(p.is_empty());
    for i in 0..5 {
        p.push(i as f32, 0.0);
    }
    assert_eq!(p.len(), 3);
    assert_eq!(p.capacity(), 3);
    let xs: Vec<f32> = p.with_points(|v| v.iter().map(|pt| pt.x).collect());
    assert_eq!(xs, [2.0, 3.0, 4.0]);
    p.with_points(|v| {
        assert_eq!(v.get(0).map(|pt| pt.x), Some(2.0));
        assert_eq!(v.get(3), None);
    });

    let r = p.revision();
    p.push(f32::NAN, 1.0);
    p.push(1.0, f32::INFINITY);
    p.extend(std::iter::empty());
    p.replace(p.with_points(|v| v.iter().collect::<Vec<_>>()));
    assert_eq!(p.revision(), r, "no-op calls leave the revision alone");

    p.extend([(7.0, 1.0), (8.0, 1.0)].map(ScatterPoint::from));
    assert_eq!(p.revision(), r + 1, "one bump per call");
    p.set(0, ScatterPoint::new(9.0, 9.0));
    assert_eq!(p.revision(), r + 2);
    p.set(0, ScatterPoint::new(9.0, 9.0));
    p.set(10, ScatterPoint::new(9.0, 9.0));
    assert_eq!(p.revision(), r + 2);
    p.replace((0..10).map(|i| ScatterPoint::new(i as f32, 0.0)));
    assert_eq!(p.len(), 3, "replace keeps the newest points");
    assert_eq!(p.with_points(|v| v.get(0)).map(|pt| pt.x), Some(7.0));
    p.clear();
    let r = p.revision();
    p.clear();
    assert_eq!(p.revision(), r);

    let twin = p.clone();
    assert!(p.ptr_eq(&twin) && !p.ptr_eq(&Points::new(3)));
    twin.push(1.0, 1.0);
    assert_eq!(p.len(), 1);
}
