//! Host-side relayout/paint cost evidence for a realistic *static* scene — the
//! "what does a frame cost when almost nothing changed?" instrument the
//! later render-model work is measured against.
//!
//! The scene is built through the public API only: a [`scaffold`] with an app
//! bar whose body is a [`navigator`] holding two pages (the root page plus one
//! transparent page pushed over it), the root page being a [`scroll_view`] over
//! a [`Column`] of 200 rows, each a [`Row`] of an icon and two [`text`]s.
//!
//! Two tests live here, split by what they can prove (same split as
//! `list_virtualization_bench.rs`):
//!
//! - [`relayout_and_paint_cost`] — `#[ignore]`d, host-machine dependent
//!   (`docs/TESTING.md` T8). Drives `RenderRoot` directly for 300 frames per
//!   scenario and prints mean/p95/p99 of `layout_with_text` and `paint` time
//!   separately (rebuild is untimed): (A) nothing changes; (B) one leaf text
//!   changes every frame; (C) the scroll offset changes every frame via
//!   `ScrollController::jump_to`. Scenario B's "signal write" is modelled as a
//!   shared `Rc<Cell<u64>>` read by the page builder (frust-widgets has no
//!   reactive dependency); the rebuild path it exercises is the same.
//!   Run: `cargo test -p frust-widgets --test relayout_cost_bench -- --ignored --nocapture`.
//! - [`scene_builds_with_two_hundred_row_pods`] — always on; asserts the scene
//!   actually materializes 200 row pods via `RenderRoot::inspect`. No timing.
//!
//! # Baseline before the render-model changes
//!
//! Host class: Linux workstation, i5-12600 (12 threads), 94 GiB RAM. Profile:
//! default `cargo test` (unoptimized + debuginfo), the same invocation
//! `list_virtualization_bench` documents. Times in microseconds. Later cards
//! append rows beneath.
//!
//! ```text
//! scenario                     pass          mean_us       p95_us       p99_us
//! A_static                     layout          66.28        68.91        75.42
//! A_static                     paint          103.19       107.83       112.49
//! B_one_leaf_changes           layout         146.97       161.81       171.25
//! B_one_leaf_changes           paint          106.79       114.17       128.34
//! C_scroll_only                layout          66.64        69.52        71.02
//! C_scroll_only                paint          137.98       159.07       162.40
//! ```
//!
//! # After p1-03 (per-pod layout skip, widget audit)
//!
//! Same host class (Linux workstation, i5-12600, 12 threads) and profile
//! (unoptimized `test` profile + debuginfo). The host may have been loaded by a
//! parallel build: a first run straight after one measured roughly double the
//! paint figures below with layout still under 2.5 µs (A) and 15 µs (C); these
//! are the middle of three back-to-back runs once it settled. Layout is what
//! the skip changes: in (A) only the root widget's own layout runs and every
//! pod below it returns its cached size, (B) lays out the changing row's path,
//! (C) the scroll surface's path while its content stays cached (the
//! `ScrollView` rebuild reports a queued jump as a layout change). Paint is
//! unchanged by design.
//!
//! ```text
//! scenario                     pass          mean_us       p95_us       p99_us
//! A_static                     layout           0.51         0.57         0.60
//! A_static                     paint          101.81       106.81       114.88
//! B_one_leaf_changes           layout          92.92       104.58       111.84
//! B_one_leaf_changes           paint          107.16       117.00       120.64
//! C_scroll_only                layout           1.47         1.73         2.40
//! C_scroll_only                paint          130.82       153.82       158.96
//! ```

use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

use frust_core::{FrameTime, PaintScene, RenderRoot};
use frust_text::TextContext;
use frust_widgets::{
    Column, IconSource, NavigatorController, Row, ScaffoldView, ScrollController, icon, navigator,
    scaffold, scroll_view, text,
};
use kurbo::{Point, Size};

const ROWS: usize = 200;
const FRAMES: usize = 300;
/// Index of the row whose text changes in scenario B.
const CHANGING_ROW: usize = 100;

const DOT: IconSource = IconSource {
    d: "M12 2a10 10 0 1 0 0 20a10 10 0 1 0 0-20z",
    design: 24.0,
};

fn window() -> Size {
    Size::new(390.0, 844.0)
}

/// A no-op paint sink: this suite measures pass cost, not emitted commands.
struct NullScene;

impl PaintScene for NullScene {
    fn fill_rect(&mut self, _origin: Point, _size: Size, _color: peniko::Color) {}
    fn draw_text(&mut self, _origin: Point, _text: &str) {}
}

type Root = RenderRoot<(), ScaffoldView<()>>;

/// Build the scene's per-frame view closure. The navigator (with its pushed
/// second page) is created once; the closure re-runs the whole view each
/// rebuild, like an app's build closure.
fn make_logic(
    scroll: ScrollController,
    tick: Rc<Cell<u64>>,
) -> impl FnMut(&mut ()) -> ScaffoldView<()> {
    let nav: NavigatorController<()> = NavigatorController::new();
    // Second page, pushed transparent so the root page stays live beneath.
    nav.push_transparent(|| text("second page"));
    move |_: &mut ()| {
        let scroll = scroll.clone();
        let tick = tick.clone();
        scaffold(navigator(&nav, move || {
            let tick = tick.get();
            let rows: Vec<_> = (0..ROWS)
                .map(|i| {
                    let label = if i == CHANGING_ROW {
                        format!("changing {tick}")
                    } else {
                        format!("detail {i}")
                    };
                    Row((icon(DOT), text(format!("Row {i}")), text(label)))
                })
                .collect();
            scroll_view(Column(rows)).controller(scroll.clone())
        }))
        .app_bar(text("Relayout bench"))
    }
}

fn frame_time(ms: f64) -> FrameTime {
    FrameTime::from_nanos((ms * 1_000_000.0) as u64)
}

/// One untimed-rebuild frame returning `(layout_us, paint_us)`.
fn timed_frame(
    root: &mut Root,
    logic: &mut impl FnMut(&mut ()) -> ScaffoldView<()>,
    tcx: &mut TextContext,
    ms: f64,
) -> (f64, f64) {
    root.rebuild(logic, &mut ());
    let t = Instant::now();
    root.layout_with_text(window(), tcx);
    let layout = t.elapsed().as_secs_f64() * 1_000_000.0;
    let mut sink = NullScene;
    let t = Instant::now();
    root.paint(&mut sink, frame_time(ms));
    let paint = t.elapsed().as_secs_f64() * 1_000_000.0;
    (layout, paint)
}

#[derive(Clone, Copy)]
enum Scenario {
    Static,
    OneLeaf,
    ScrollOnly,
}

fn run(scenario: Scenario) -> (Vec<f64>, Vec<f64>) {
    let scroll = ScrollController::new();
    let tick = Rc::new(Cell::new(0u64));
    let mut logic = make_logic(scroll.clone(), tick.clone());
    let mut root: Root = RenderRoot::new();
    let mut tcx = TextContext::new();

    // Settle: initial build, the navigator's queued push, text shaping caches.
    let mut ms = 0.0;
    for _ in 0..3 {
        timed_frame(&mut root, &mut logic, &mut tcx, ms);
        ms += 16.0;
    }

    let mut layouts = Vec::with_capacity(FRAMES);
    let mut paints = Vec::with_capacity(FRAMES);
    for f in 0..FRAMES {
        match scenario {
            Scenario::Static => {}
            Scenario::OneLeaf => tick.set(f as u64 + 1),
            Scenario::ScrollOnly => scroll.jump_to(4.0 * (f as f64 + 1.0)),
        }
        let (l, p) = timed_frame(&mut root, &mut logic, &mut tcx, ms);
        layouts.push(l);
        paints.push(p);
        ms += 16.0;
    }
    (layouts, paints)
}

struct Stats {
    mean: f64,
    p95: f64,
    p99: f64,
}

fn stats(mut samples: Vec<f64>) -> Stats {
    samples.sort_by(|a, b| a.partial_cmp(b).expect("frame timings are always finite"));
    let mean = samples.iter().sum::<f64>() / samples.len() as f64;
    Stats {
        mean,
        p95: percentile(&samples, 0.95),
        p99: percentile(&samples, 0.99),
    }
}

/// Nearest-rank percentile over an already-sorted sample set.
fn percentile(sorted: &[f64], p: f64) -> f64 {
    let rank = (((sorted.len() - 1) as f64) * p).round() as usize;
    sorted[rank.min(sorted.len() - 1)]
}

fn print_row(config: &str, pass: &str, s: Stats) {
    println!(
        "{config:<28} {pass:<8} {:>12.2} {:>12.2} {:>12.2}",
        s.mean, s.p95, s.p99
    );
}

#[test]
#[ignore = "perf evidence, host-machine dependent (docs/TESTING.md T8): run via \
            `cargo test -p frust-widgets --test relayout_cost_bench -- --ignored --nocapture`"]
fn relayout_and_paint_cost() {
    println!(
        "\n{:<28} {:<8} {:>12} {:>12} {:>12}",
        "scenario", "pass", "mean_us", "p95_us", "p99_us"
    );
    for (name, scenario) in [
        ("A_static", Scenario::Static),
        ("B_one_leaf_changes", Scenario::OneLeaf),
        ("C_scroll_only", Scenario::ScrollOnly),
    ] {
        let (layouts, paints) = run(scenario);
        print_row(name, "layout", stats(layouts));
        print_row(name, "paint", stats(paints));
    }
}

/// The scene materializes every row: 200 `Row` pods (each exactly icon + two
/// texts = 3 children) hang under the `Column`, reached via `inspect`.
#[test]
fn scene_builds_with_two_hundred_row_pods() {
    let scroll = ScrollController::new();
    let tick = Rc::new(Cell::new(0u64));
    let mut logic = make_logic(scroll, tick);
    let mut root: Root = RenderRoot::new();
    let mut tcx = TextContext::new();
    for f in 0..3 {
        timed_frame(&mut root, &mut logic, &mut tcx, f as f64 * 16.0);
    }
    let nodes = root.inspect();
    let row_pods = nodes
        .iter()
        .filter(|n| n.type_name.contains("FlexWidget") && n.children.len() == 3)
        .count();
    assert!(
        row_pods >= ROWS,
        "expected at least {ROWS} row pods (flex nodes with icon + 2 texts), found {row_pods} \
         among {} nodes",
        nodes.len()
    );
}
