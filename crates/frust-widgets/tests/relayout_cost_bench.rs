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
//!   Set `FRUST_NO_LAYOUT_SKIP=1` to run the skip-off arm (`set_layout_skip(false)`,
//!   every pod lays out on every pass); the header prints `arm: skip=on|off`.
//! - [`retained_layer_cost`] — `#[ignore]`d; the retained-layer scenarios D/E/F with
//!   a retained on/off arm (`FRUST_NO_RETAINED_LAYERS=1`), see the last section.
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
//!
//! # After p1 (layout skip gated on pending LAYOUT; FRUST_NO_LAYOUT_SKIP arm)
//!
//! Measured at `feature/render-model-p1` @ 6e86476b plus the harness change
//! be0cd57d (the `FRUST_NO_LAYOUT_SKIP` arm and these rows). Same host class
//! (Linux workstation, i5-12600, 12 threads) and profile (unoptimized `test`
//! profile + debuginfo). Both arms ran back to back at a 1-minute host load of
//! 2.9-3.0 (below the ~4 threshold; an earlier arm started at load 4.7 and is
//! not recorded). `arm: skip=on` is the default `set_layout_skip(true)`;
//! `arm: skip=off` is `FRUST_NO_LAYOUT_SKIP=1`, which calls
//! `set_layout_skip(false)` so every pod lays out on every pass. The off arm
//! matches the baseline layout figures above within host noise, so the skip is
//! the only variable.
//! Paint is unchanged by design and varies run to run within a few percent.
//!
//! Layout ratio, one leaf changes / no change (mean): skip on
//! 93.56 / 0.52 = 179.9x; skip off 160.05 / 76.25 = 2.10x.
//!
//! ```text
//! arm: skip=on
//! scenario                     pass          mean_us       p95_us       p99_us
//! A_static                     layout           0.52         0.56         1.32
//! A_static                     paint          105.65       110.12       177.54
//! B_one_leaf_changes           layout          93.56       104.18       107.76
//! B_one_leaf_changes           paint          106.34       111.37       113.54
//! C_scroll_only                layout           1.46         1.62         3.02
//! C_scroll_only                paint          137.10       161.16       179.75
//! ```
//!
//! ```text
//! arm: skip=off
//! scenario                     pass          mean_us       p95_us       p99_us
//! A_static                     layout          76.25        79.35        80.54
//! A_static                     paint          102.81       106.43       111.80
//! B_one_leaf_changes           layout         160.05       172.42       176.47
//! B_one_leaf_changes           paint          106.49       111.61       115.48
//! C_scroll_only                layout          75.20        78.99        81.34
//! C_scroll_only                paint          131.86       154.38       156.65
//! ```
//!
//! # After p3-05..p3-08 (retained layers, host leg; `FRUST_NO_RETAINED_LAYERS` arm)
//!
//! Measured at `feature/render-model-p3` @ f83479e3 plus the harness change that
//! adds these rows (scenarios D/E/F, [`retained_layer_cost`]). Same host class
//! (Linux workstation, i5-12600, 12 threads); profile is **release**
//! (`--release`), so the figures are not comparable with the unoptimized
//! A/B/C sections above. Two runs per arm, run back to back (on, off, on, off);
//! the second run of each arm is recorded (the steadier: smaller p95 spread).
//! The 1-minute host load was 3.7-4.4 throughout, so absolute microseconds are
//! indicative; the command counts are deterministic. Run:
//! `cargo test -p frust-widgets --test relayout_cost_bench --release -- --ignored --nocapture retained_layer_cost`
//! (default = retained on; `FRUST_NO_RETAINED_LAYERS=1` = `set_retained_layers(false)`).
//!
//! There is no engine on the host, so a counting scene plays its part: it
//! records every `push_retained` and feeds the resident set (key, page extent
//! as `round(size * scale)` px, scale 1.0) back as the next frame's ledger via
//! `set_retained_ledger`, answers `current_transform()`, and counts body
//! commands (fill / rounded fill / stroke / text / glyph run / image / path
//! calls) separately from `push_retained` and `draw_retained`.
//!
//! Scenarios (300 frames after 3 warm-up frames, 16.67 ms frame times):
//! D = `AnimatedOpacity(..).cached(true)` over 200 rows fading 1.0 to 0.0;
//! E = a navigator over 200-row pages cycling push (120 ms fade), pop,
//! push (120 ms iOS slide), pop, one command per 10 frames, so a cycle of
//! 40 frames has 7-8 transition frames and 2-3 rest frames per half; counts
//! are taken from the third cycle (seed = the frame the push/pop is issued,
//! mid = 3 frames in, rest = the last frame of the half). Layout mean for E is
//! dominated by laying out each freshly pushed page (~6 ms, the p99);
//! F = one navigator page at rest, one leaf text changing every frame (the B
//! shape), no transition.
//!
//! Delta, retained on vs off (paint mean): D 0.11 vs 49.00 us (steady frame:
//! 0 body commands + 1 DrawRetained vs 600 body commands); E 32.63 vs 111.52 us
//! (mean 204 body + 1.45 DrawRetained + 0.05 PushRetained per frame vs 1076
//! body); **F 48.53 vs 48.54 us, identical, 600 body commands and no retained
//! command in either arm** - a navigator page at rest paints plain, so the
//! steady-state host cost of the retained model is nil (no regression on the
//! F row against the plan's "no steady-state regression" bar).
//!
//! ```text
//! arm: retained=on
//! scenario                     pass          mean_us       p95_us       p99_us
//! D_cached_opacity             layout           0.04         0.05         0.06
//! D_cached_opacity             paint            0.11         0.14         0.18
//! E_navigator_push_pop         layout         298.13         0.16      6220.95
//! E_navigator_push_pop         paint           32.63        70.35       224.66
//! F_navigator_rest             layout           0.04         0.05         0.06
//! F_navigator_rest             paint           48.53        49.89        55.47
//! scenario                     frame                  body     push     draw
//! D_cached_opacity             first (warm-up 0)       600        1        0
//! D_cached_opacity             steady (frame 150)        0        0        1
//! D_cached_opacity             mean per frame         0.00     0.00     1.00
//! E_navigator_push_pop         first (warm-up 0)       600        0        0
//! E_navigator_push_pop         fade_push seed            0        0        1
//! E_navigator_push_pop         fade_push mid             0        0        2
//! E_navigator_push_pop         fade_push rest          600        0        0
//! E_navigator_push_pop         fade_pop seed             0        0        1
//! E_navigator_push_pop         fade_pop mid              0        0        2
//! E_navigator_push_pop         fade_pop rest           600        0        0
//! E_navigator_push_pop         slide_push seed         600        0        1
//! E_navigator_push_pop         slide_push mid          600        0        1
//! E_navigator_push_pop         slide_push rest         600        0        0
//! E_navigator_push_pop         slide_pop seed            0        0        2
//! E_navigator_push_pop         slide_pop mid             0        0        2
//! E_navigator_push_pop         slide_pop rest          600        0        0
//! E_navigator_push_pop         mean per frame       204.00     0.05     1.45
//! F_navigator_rest             first (warm-up 0)       600        0        0
//! F_navigator_rest             steady (frame 150)      600        0        0
//! F_navigator_rest             mean per frame       600.00     0.00     0.00
//! ```
//!
//! ```text
//! arm: retained=off
//! scenario                     pass          mean_us       p95_us       p99_us
//! D_cached_opacity             layout           0.05         0.06         0.07
//! D_cached_opacity             paint           49.00        52.16        58.31
//! E_navigator_push_pop         layout         299.89         0.14      6215.24
//! E_navigator_push_pop         paint          111.52       138.77       283.50
//! F_navigator_rest             layout           0.04         0.04         0.07
//! F_navigator_rest             paint           48.54        50.20        55.17
//! scenario                     frame                  body     push     draw
//! D_cached_opacity             first (warm-up 0)       600        0        0
//! D_cached_opacity             steady (frame 150)      600        0        0
//! D_cached_opacity             mean per frame       600.00     0.00     0.00
//! E_navigator_push_pop         first (warm-up 0)       600        0        0
//! E_navigator_push_pop         fade_push seed          600        0        0
//! E_navigator_push_pop         fade_push mid          1200        0        0
//! E_navigator_push_pop         fade_push rest          600        0        0
//! E_navigator_push_pop         fade_pop seed           600        0        0
//! E_navigator_push_pop         fade_pop mid           1200        0        0
//! E_navigator_push_pop         fade_pop rest           600        0        0
//! E_navigator_push_pop         slide_push seed        1200        0        0
//! E_navigator_push_pop         slide_push mid         1200        0        0
//! E_navigator_push_pop         slide_push rest         600        0        0
//! E_navigator_push_pop         slide_pop seed         1200        0        0
//! E_navigator_push_pop         slide_pop mid          1200        0        0
//! E_navigator_push_pop         slide_pop rest          600        0        0
//! E_navigator_push_pop         mean per frame      1076.00     0.00     0.00
//! F_navigator_rest             first (warm-up 0)       600        0        0
//! F_navigator_rest             steady (frame 150)      600        0        0
//! F_navigator_rest             mean per frame       600.00     0.00     0.00
//! ```

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use frust_core::{Curve, FrameTime, PaintScene, RenderRoot, RetainedLedger, View};
use frust_text::TextContext;
use frust_widgets::motion::animated_opacity;
use frust_widgets::{
    Column, IconSource, NavigatorController, PageTransition, Row, ScaffoldView, ScrollController,
    Timing, TransitionSpec, icon, navigator, scaffold, scroll_view, text,
};
use kurbo::{Affine, Point, Rect, Size};

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

fn run(scenario: Scenario, no_layout_skip: bool) -> (Vec<f64>, Vec<f64>) {
    let scroll = ScrollController::new();
    let tick = Rc::new(Cell::new(0u64));
    let mut logic = make_logic(scroll.clone(), tick.clone());
    let mut root: Root = RenderRoot::new();
    if no_layout_skip {
        root.set_layout_skip(false);
    }
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
    let no_layout_skip = std::env::var_os("FRUST_NO_LAYOUT_SKIP").is_some();
    println!("\narm: skip={}", if no_layout_skip { "off" } else { "on" });
    println!(
        "{:<28} {:<8} {:>12} {:>12} {:>12}",
        "scenario", "pass", "mean_us", "p95_us", "p99_us"
    );
    for (name, scenario) in [
        ("A_static", Scenario::Static),
        ("B_one_leaf_changes", Scenario::OneLeaf),
        ("C_scroll_only", Scenario::ScrollOnly),
    ] {
        let (layouts, paints) = run(scenario, no_layout_skip);
        print_row(name, "layout", stats(layouts));
        print_row(name, "paint", stats(paints));
    }
}

// ---------------------------------------------------------------------------
// Retained-layer scenarios (D, E, F): a counting scene that plays the engine's
// part (records every `push_retained`, feeds the resident set back as the next
// frame's ledger), mirroring `tests/repaint_boundary.rs`.
// ---------------------------------------------------------------------------

/// Commands one frame emitted, as the counting scene saw them.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
struct Counts {
    /// Fill / text / image / path calls (the "body commands").
    body: usize,
    push_retained: usize,
    draw_retained: usize,
}

/// A counting `PaintScene` that also emulates the renderer's resident-page
/// ledger: every recorded page stays resident at `round(size * scale)` px.
struct CountScene {
    counts: Counts,
    stack: Vec<Affine>,
    resident: HashMap<u64, ((u32, u32), f32)>,
}

impl CountScene {
    fn new() -> Self {
        Self {
            counts: Counts::default(),
            stack: vec![Affine::scale(1.0)],
            resident: HashMap::new(),
        }
    }

    /// The ledger the renderer would publish after the frame just painted.
    fn ledger(&self) -> RetainedLedger {
        RetainedLedger::from_entries(
            1,
            self.resident
                .iter()
                .map(|(key, (extent, scale))| (*key, *extent, *scale)),
        )
    }
}

impl PaintScene for CountScene {
    fn fill_rect(&mut self, _origin: Point, _size: Size, _color: peniko::Color) {
        self.counts.body += 1;
    }
    fn fill_rounded_rect(&mut self, _o: Point, _s: Size, _r: f64, _c: peniko::Color) {
        self.counts.body += 1;
    }
    fn stroke_line(&mut self, _p0: Point, _p1: Point, _w: f64, _c: peniko::Color) {
        self.counts.body += 1;
    }
    fn draw_text(&mut self, _origin: Point, _text: &str) {
        self.counts.body += 1;
    }
    fn draw_glyph_run(&mut self, _run: frust_scene::GlyphRun) {
        self.counts.body += 1;
    }
    fn draw_image(&mut self, _data: &peniko::ImageData, _dest: Rect) {
        self.counts.body += 1;
    }
    fn fill_path(&mut self, _origin: Point, _path: &kurbo::BezPath, _brush: &peniko::Brush) {
        self.counts.body += 1;
    }
    fn stroke_path(
        &mut self,
        _origin: Point,
        _path: &kurbo::BezPath,
        _width: f64,
        _brush: &peniko::Brush,
    ) {
        self.counts.body += 1;
    }
    fn push_transform(&mut self, transform: Affine) {
        let current = *self.stack.last().expect("root transform");
        self.stack.push(current * transform);
    }
    fn pop_transform(&mut self) {
        if self.stack.len() > 1 {
            self.stack.pop();
        }
    }
    fn push_retained(&mut self, key: u64, _origin: Point, size: Size, _alpha: f32, scale: f64) {
        self.counts.push_retained += 1;
        let extent = (
            size.width.round().max(0.0) as u32,
            size.height.round().max(0.0) as u32,
        );
        self.resident.insert(key, (extent, scale as f32));
    }
    fn pop_retained(&mut self) {}
    fn draw_retained(&mut self, _key: u64, _o: Point, _s: Size, _alpha: f32, _scale: f64) {
        self.counts.draw_retained += 1;
    }
    fn current_transform(&self) -> Option<Affine> {
        self.stack.last().copied()
    }
}

/// 200 rows of icon + two texts, the same row shape as the A/B/C scene.
fn rows_view(tick: u64) -> impl View<()> {
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
    Column(rows)
}

/// 16.67 ms frame cadence in nanoseconds.
fn frame_ns(index: usize) -> FrameTime {
    FrameTime::from_nanos((index as f64 * 16_666_667.0) as u64)
}

/// Output of one retained-scenario run.
struct RetainedRun {
    layouts: Vec<f64>,
    paints: Vec<f64>,
    /// Counts per frame, warm-up frames first (`WARMUP` of them).
    counts: Vec<Counts>,
}

const WARMUP: usize = 3;

/// Drive `frames` measured frames (after `WARMUP` settle frames) of `logic`.
/// `before(frame)` runs before each measured frame (state changes, nav calls);
/// it receives `usize::MAX` for none of the warm-up frames (they never call it).
fn drive_retained<V: View<()>>(
    logic: &mut impl FnMut(&mut ()) -> V,
    retained_on: bool,
    frames: usize,
    mut before: impl FnMut(usize),
) -> RetainedRun {
    let mut root: RenderRoot<(), V> = RenderRoot::new();
    if !retained_on {
        root.set_retained_layers(false);
    }
    let mut tcx = TextContext::new();
    let mut scene = CountScene::new();
    let mut run = RetainedRun {
        layouts: Vec::with_capacity(frames),
        paints: Vec::with_capacity(frames),
        counts: Vec::with_capacity(frames + WARMUP),
    };
    for f in 0..WARMUP + frames {
        let measured = f >= WARMUP;
        if measured {
            before(f - WARMUP);
        }
        root.rebuild(logic, &mut ());
        let t = Instant::now();
        root.layout_with_text(window(), &mut tcx);
        let layout = t.elapsed().as_secs_f64() * 1_000_000.0;
        scene.counts = Counts::default();
        let t = Instant::now();
        root.paint(&mut scene, frame_ns(f));
        let paint = t.elapsed().as_secs_f64() * 1_000_000.0;
        root.set_retained_ledger(&scene.ledger());
        run.counts.push(scene.counts);
        if measured {
            run.layouts.push(layout);
            run.paints.push(paint);
        }
    }
    run
}

/// D: `AnimatedOpacity(..).cached(true)` over 200 rows, fading for the whole run.
fn run_d(retained_on: bool, frames: usize) -> RetainedRun {
    let target = Rc::new(Cell::new(1.0f64));
    let t = target.clone();
    let mut logic = move |_: &mut ()| {
        animated_opacity(t.get(), rows_view(0))
            .timing(Timing::Duration(Duration::from_secs(60), Curve::Linear))
            .cached(true)
    };
    let started = Cell::new(false);
    drive_retained(&mut logic, retained_on, frames, |_| {
        if !started.replace(true) {
            target.set(0.0);
        }
    })
}

/// Frames per navigator half-cycle (push or pop): a seed frame, the 120 ms
/// transition (about 7 frames), then rest frames.
const HALF: usize = 10;

/// What the navigator is doing on measured frame `f` of scenario E.
/// Cycle of `4 * HALF`: fade push, fade pop, slide push, slide pop.
fn e_phase(f: usize) -> (&'static str, usize) {
    let cycle = f % (4 * HALF);
    let kind = match cycle / HALF {
        0 => "fade_push",
        1 => "fade_pop",
        2 => "slide_push",
        _ => "slide_pop",
    };
    (kind, cycle % HALF)
}

fn transition(preset: PageTransition) -> TransitionSpec {
    TransitionSpec::new(
        preset,
        Timing::Duration(Duration::from_millis(120), Curve::Linear),
    )
}

/// E: a navigator over 200-row pages cycling push (fade) / pop, push (slide) / pop.
fn run_e(retained_on: bool, frames: usize) -> RetainedRun {
    let ctrl: NavigatorController<()> = NavigatorController::new();
    let c = ctrl.clone();
    let mut logic = move |_: &mut ()| navigator(&c, || rows_view(0));
    drive_retained(&mut logic, retained_on, frames, |f| {
        let (kind, idx) = e_phase(f);
        if idx != 0 {
            return;
        }
        match kind {
            "fade_push" => ctrl.push_with(
                || rows_view(1),
                transition(PageTransition::ReducedCrossfade),
            ),
            "slide_push" => ctrl.push_with(|| rows_view(1), transition(PageTransition::IosPush)),
            _ => ctrl.pop(),
        }
    })
}

/// F: one navigator page at rest; one leaf changes every frame, no transition.
fn run_f(retained_on: bool, frames: usize) -> RetainedRun {
    let ctrl: NavigatorController<()> = NavigatorController::new();
    let tick = Rc::new(Cell::new(0u64));
    let (c, t) = (ctrl.clone(), tick.clone());
    let mut logic = move |_: &mut ()| {
        let tick = t.get();
        navigator(&c, move || rows_view(tick))
    };
    drive_retained(&mut logic, retained_on, frames, |f| tick.set(f as u64 + 1))
}

fn mean_counts(counts: &[Counts]) -> (f64, f64, f64) {
    let n = counts.len().max(1) as f64;
    (
        counts.iter().map(|c| c.body).sum::<usize>() as f64 / n,
        counts.iter().map(|c| c.push_retained).sum::<usize>() as f64 / n,
        counts.iter().map(|c| c.draw_retained).sum::<usize>() as f64 / n,
    )
}

fn print_counts(config: &str, frame: &str, c: Counts) {
    println!(
        "{config:<28} {frame:<18} {:>8} {:>8} {:>8}",
        c.body, c.push_retained, c.draw_retained
    );
}

#[test]
#[ignore = "perf evidence, host-machine dependent (docs/TESTING.md T8): run via \
            `cargo test -p frust-widgets --test relayout_cost_bench --release -- --ignored --nocapture`"]
fn retained_layer_cost() {
    let retained_on = std::env::var_os("FRUST_NO_RETAINED_LAYERS").is_none();
    println!("\narm: retained={}", if retained_on { "on" } else { "off" });
    println!(
        "{:<28} {:<8} {:>12} {:>12} {:>12}",
        "scenario", "pass", "mean_us", "p95_us", "p99_us"
    );
    let runs = [
        ("D_cached_opacity", run_d(retained_on, FRAMES)),
        ("E_navigator_push_pop", run_e(retained_on, FRAMES)),
        ("F_navigator_rest", run_f(retained_on, FRAMES)),
    ];
    let mut tables = Vec::new();
    for (name, run) in runs {
        print_row(name, "layout", stats(run.layouts.clone()));
        print_row(name, "paint", stats(run.paints.clone()));
        tables.push((name, run));
    }
    println!(
        "\n{:<28} {:<18} {:>8} {:>8} {:>8}",
        "scenario", "frame", "body", "push", "draw"
    );
    for (name, run) in &tables {
        let measured = &run.counts[WARMUP..];
        print_counts(name, "first (warm-up 0)", run.counts[0]);
        match *name {
            "E_navigator_push_pop" => {
                // The third cycle: caches and ledger warm.
                let at = |f: usize| measured[2 * 4 * HALF + f];
                for (label, base) in [
                    ("fade_push", 0),
                    ("fade_pop", HALF),
                    ("slide_push", 2 * HALF),
                    ("slide_pop", 3 * HALF),
                ] {
                    print_counts(name, &format!("{label} seed"), at(base));
                    print_counts(name, &format!("{label} mid"), at(base + 3));
                    print_counts(name, &format!("{label} rest"), at(base + HALF - 1));
                }
            }
            _ => print_counts(name, "steady (frame 150)", measured[150]),
        }
        let (b, p, d) = mean_counts(measured);
        println!(
            "{name:<28} {:<18} {b:>8.2} {p:>8.2} {d:>8.2}",
            "mean per frame"
        );
    }
}

/// The retained scenarios emit what the plan says, in both arms.
#[test]
fn retained_scenarios_emit_the_expected_commands() {
    // D: retained on, steady frames draw the page with no body commands.
    let on = run_d(true, 12);
    let steady = on.counts[WARMUP + 6];
    assert_eq!(steady.body, 0, "D steady frame paints no body: {steady:?}");
    assert_eq!(steady.draw_retained, 1, "D steady frame draws the page");
    assert_eq!(steady.push_retained, 0);
    assert!(on.counts[..WARMUP + 1].iter().any(|c| c.push_retained == 1));
    // D: retained off, the full body every frame and no retained commands.
    let off = run_d(false, 12);
    for c in &off.counts {
        assert!(
            c.body > 0 && c.push_retained == 0 && c.draw_retained == 0,
            "{c:?}"
        );
    }
    // F: a page at rest paints plain in both arms (no bracket, no page).
    for retained_on in [true, false] {
        let f = run_f(retained_on, 12);
        let c = f.counts[WARMUP + 6];
        assert!(c.body > 0, "F paints its body: {c:?}");
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
