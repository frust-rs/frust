//! `qr_code` headless: a real `RenderRoot` painting into a recording scene.

use std::sync::Arc;

use frust::authoring::{PaintScene, Point, Size};
use frust::{Color, FrameTime};
use frust_core::RenderRoot;
use frust_qr::{EccLevel, dark_runs, encode, qr_code};
use frust_reactive::ReactiveRuntime;

#[derive(Default)]
struct Rec {
    rects: Vec<(Point, Size, Color)>,
}

impl PaintScene for Rec {
    fn fill_rect(&mut self, origin: Point, size: Size, color: Color) {
        self.rects.push((origin, size, color));
    }
    fn draw_text(&mut self, _origin: Point, _text: &str) {}
}

fn paint(view_side: f64, ecc: EccLevel) -> Rec {
    let rt = ReactiveRuntime::init(Arc::new(|| {}));
    let mut root: RenderRoot<(), _> = RenderRoot::new();
    let mut logic = |_: &mut ()| qr_code::<()>("frust").ecc(ecc);
    rt.with_owner(|| root.rebuild(&mut logic, &mut ()));
    root.layout(Size::new(view_side, view_side));
    let mut rec = Rec::default();
    root.paint(&mut rec, FrameTime::from_nanos(1_000_000_000));
    rec
}

#[test]
fn rect_count_is_dark_runs_plus_background_and_all_on_grid() {
    let rec = paint(200.0, EccLevel::M);
    let m = encode(b"frust", EccLevel::M).unwrap();
    let runs: usize = (0..m.width()).map(|y| dark_runs(m.row(y)).count()).sum();
    assert_eq!(rec.rects.len(), runs + 1);

    let (bg_origin, bg_size, bg_color) = rec.rects[0];
    assert_eq!((bg_origin, bg_size), (Point::ZERO, Size::new(200.0, 200.0)));
    assert_eq!(bg_color, Color::WHITE);

    // 21 modules + 2 * 4 quiet = 29; floor(200 / 29) = 6; offset floor((200 - 174) / 2) = 13.
    let (module, offset) = (6.0, 13.0);
    for (origin, size, color) in &rec.rects[1..] {
        assert_eq!(*color, Color::BLACK);
        assert_eq!(size.height, module);
        assert_eq!(size.width % module, 0.0);
        assert_eq!((origin.x - offset) % module, 0.0);
        assert_eq!((origin.y - offset) % module, 0.0);
        assert!(origin.x >= offset + 4.0 * module);
        assert!(origin.x + size.width <= offset + (4.0 + 21.0) * module);
    }
}
