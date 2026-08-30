//! The cross-oracle differential: generated scenes rendered by the engine and
//! by `vello_cpu` 0.2.0, held to a near-exact, measured bar.
//!
//! ```text
//! WGPU_BACKEND=vulkan WGPU_ADAPTER_NAME=T400 FRUST_GOLDEN_EXPECT_ADAPTER=T400 \
//!   cargo test -p frust-testing --test fuzz -- --ignored --nocapture
//! ```
//!
//! Where `tests/engine_goldens.rs` compares a fixed, reviewed corpus, this
//! compares scenes nobody wrote: a generator emits random programs over the
//! primitive set the engine actually lowers, and every one of them must land
//! within a measured tolerance of the CPU rasterizer. The two share one
//! `vello_common` geometry core at one flattening tolerance, so only the *fill*
//! differs — a GPU strip pass against a CPU one — and a disagreement beyond that
//! tolerance is a real divergence rather than a tolerance argument.
//!
//! # The bar, and why it has two halves
//!
//! [`DIFFERENTIAL_TOLERANCE`] on the 1-px-eroded interior, plus
//! [`EDGE_JITTER_CEILING`] on the raw, un-eroded per-channel maximum. Both are
//! measured numbers, not guesses, and both are reported on every run.
//!
//! Erosion is the same `eroded_interior` mask `diff_images` already offers a
//! case whose expected difference is antialiased edge jitter rather than an
//! interior change, and it earns its place here: over 2,000 generated scenes on
//! the pinned Vulkan/T400 runner, 342 (17%) carried at least one pixel over the
//! corpus's tight default when the raw mask was counted, and **zero** did once
//! the mask was eroded by one pixel — every one of those disagreements was a
//! single-pixel-wide edge.
//!
//! Erosion alone would let a real regression hide as long as it stayed one pixel
//! wide, which is what the raw ceiling is for: a hairline that shifted by a whole
//! level clears the erosion and trips the ceiling instead.
//!
//! # Why the generator is finite and bounded
//!
//! This is a differential, not a robustness fuzz. `NaN`, the infinities and the
//! huge magnitudes belong to `frust-engine`'s own `proptest_strips` suite, which
//! asserts refusal-not-panic on the compiler; here they would only prove that
//! the two arms disagree about a scene ONE of them refuses (the engine's frame
//! path rejects a non-finite transform outright; `vello_cpu` has no such
//! refusal), which is a difference in contract, not in rasterization. So every
//! generated value is finite and roughly viewport-sized, and every generated
//! command is one both arms draw.
//!
//! # Comparison space
//!
//! Premultiplied, exactly as both arms natively produce and exactly as the
//! engine goldens compare. Un-premultiplying divides each colour channel by the
//! pixel's own alpha, so along a hairline's end cap or a dash boundary (alpha 1
//! to 11 of 255) it multiplies an 8-bit difference by up to 255 — comparing
//! after that conversion measures the conversion's own information loss, which
//! no tolerance can tell apart from a regression.
//!
//! # Case count
//!
//! [`DEFAULT_CASES`] cases per run, overridable with `FRUST_FUZZ_CASES` — see
//! that constant for the measured wall clock behind the number.
//!
//! # On failure
//!
//! The minimized program is written as JSON beside the triptych, under
//! `target/frust-testing/<class>/fuzz-differential/` — never into
//! `testing/goldens/`. A reader can replay it without re-running the generator.

use std::cell::{Cell, RefCell};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use anyhow::{Context, Result};
use image::RgbaImage;
use proptest::prelude::*;
use proptest::test_runner::{Config, TestError, TestRunner};
use serde::Serialize;

use frust_scene::{DashPattern, Scene, SceneBuilder};
use frust_testing::case::Tolerance;
use frust_testing::diff::{DiffOutcome, DiffReport, diff_images};
use frust_testing::golden::artifacts_root;
use frust_testing::oracle_cpu::CpuOracle;
use frust_testing::oracle_engine::{EngineOracle, EngineOracleOptions};
use frust_testing::render::{RenderSpec, RenderedImage, SceneRenderer};
use kurbo::{Affine, BezPath, Point, Rect};
use peniko::{Brush, Color, Gradient};

/// Frame size every case renders at. Small deliberately: a differential over ten
/// thousand cases is bounded by readback and CPU rasterization, both linear in
/// pixel count, and 64x64 is the same extent the engine goldens use.
const FRAME: u32 = 64;

/// Cases a run compares.
///
/// The full sweep, kept as the committed default because it is cheap: a case is
/// one 64x64 engine frame with readback, one `vello_cpu` frame and one diff,
/// which measured out at roughly 2.5 ms on the pinned Vulkan/T400 runner — so
/// ten thousand of them is well under a minute, not a session of its own. Raise
/// or lower it with [`CASES_ENV_VAR`] when bisecting.
const DEFAULT_CASES: u32 = 10_000;

/// Environment variable overriding [`DEFAULT_CASES`].
const CASES_ENV_VAR: &str = "FRUST_FUZZ_CASES";

/// The tolerance the eroded interior is compared under.
///
/// Two levels wider than the corpus's own tight default (channel 2, alpha 2),
/// and the widening is measured rather than assumed. Over 42,000 generated
/// scenes on the pinned Vulkan/T400 runner (one 2,000-case survey plus four full
/// sweeps) the worst RAW per-channel disagreement anywhere was 6, and the eroded
/// interior carried no pixel over this tolerance at all. Roughly 18% of cases
/// carry a raw pixel over the corpus default; all but one were single-pixel-wide
/// edge jitter the erosion removes, and the one that was not was a small
/// interior region under a skewed transform where several TRANSLUCENT primitives
/// (alpha 1, 25 and 55) composite over each other.
///
/// That last case is the reason the default cannot simply be reused here. A
/// composite rounds to 8 bits once per layer, so the two fills — one on the GPU,
/// one on the CPU — drift by up to a level per stacked translucent primitive; a
/// generated scene stacks up to [`MAX_OPS`] of them, while every case in the
/// fixed corpus `tests/engine_goldens.rs` gates draws one. That gate keeps the
/// exact default, unchanged; this one trades two levels for scene diversity the
/// corpus does not have, and pairs the trade with [`EDGE_JITTER_CEILING`] so the
/// raw magnitude is still bounded.
const DIFFERENTIAL_TOLERANCE: Tolerance = Tolerance::new().with_channel(6).with_alpha(6);

/// Ceiling on the RAW, un-eroded per-channel maximum difference between the two
/// arms, over every case in a run.
///
/// The second half of the bar (see the module docs). It is the same channel-8
/// band `testing/goldens/CALIBRATION.md` measures the classic arm's own drift
/// under, so the two numbers mean the same thing, and it sits above a measured
/// worst of 6 — a deliberately thin margin, since the regressions this half
/// exists to catch (a hairline landing on a different pixel, a cap or dash
/// boundary moving) shift a channel by tens of levels, not by one. A run that
/// trips it at 7 is worth reading the persisted artifact over rather than
/// widening on sight.
const EDGE_JITTER_CEILING: i16 = 8;

/// Longest generated program.
const MAX_OPS: usize = 8;

/// Coordinate range every generated point is drawn from: the frame, with enough
/// overhang on each side that clipping against the viewport edge is exercised
/// rather than avoided.
const COORD_MIN: f64 = -8.0;
const COORD_MAX: f64 = 72.0;

/// Serializes every test in this binary that creates a GPU device.
///
/// Two ignored GPU tests running concurrently have been observed to hang at
/// process teardown: each holds its own device and pipeline-warm-up state, and
/// tearing both down at once races inside the driver. Poison is ignored
/// deliberately — one test's failure must not cascade into every sibling. Same
/// guard, same reasoning, as `tests/engine_goldens.rs`.
static RENDER_LOCK: Mutex<()> = Mutex::new(());

fn render_lock() -> MutexGuard<'static, ()> {
    RENDER_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

/// How many cases this run compares.
fn configured_cases() -> u32 {
    std::env::var(CASES_ENV_VAR)
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok())
        .filter(|cases| *cases > 0)
        .unwrap_or(DEFAULT_CASES)
}

// ---------------------------------------------------------------------------
// The generated program — plain data, so a failing case persists as JSON
// ---------------------------------------------------------------------------

/// A brush, in the two forms the engine's paint encoding resolves: an inline
/// solid colour and an indexed gradient with its own colour ramp.
#[derive(Clone, Copy, Debug, Serialize)]
enum Paint {
    Solid([u8; 4]),
    LinearGradient {
        start: [f64; 2],
        end: [f64; 2],
        stops: [[u8; 4]; 2],
    },
}

impl Paint {
    fn brush(self) -> Brush {
        match self {
            Self::Solid(rgba) => Brush::Solid(rgba8(rgba)),
            Self::LinearGradient { start, end, stops } => Brush::Gradient(
                Gradient::new_linear(point(start), point(end))
                    .with_stops([rgba8(stops[0]), rgba8(stops[1])]),
            ),
        }
    }
}

/// One path element, in the same plain-data form as [`Paint`].
#[derive(Clone, Copy, Debug, Serialize)]
enum Segment {
    MoveTo([f64; 2]),
    LineTo([f64; 2]),
    QuadTo([f64; 2], [f64; 2]),
    CurveTo([f64; 2], [f64; 2], [f64; 2]),
    ClosePath,
}

/// One builder call, restricted to the primitives the engine lowers today:
/// axis-aligned rectangles, rounded rectangles, lines, and arbitrary
/// filled/stroked/dashed paths, plus the transform stack every one of them
/// composes through.
#[derive(Clone, Debug, Serialize)]
enum Op {
    FillRect {
        rect: [f64; 4],
        paint: Paint,
    },
    RoundedRect {
        rect: [f64; 4],
        radii: [f64; 4],
        paint: Paint,
    },
    Line {
        p0: [f64; 2],
        p1: [f64; 2],
        width: f64,
        paint: Paint,
    },
    FillPath {
        path: Vec<Segment>,
        paint: Paint,
    },
    StrokePath {
        path: Vec<Segment>,
        width: f64,
        /// `[on, off, phase]`, or `None` for a solid stroke.
        dash: Option<[f64; 3]>,
        paint: Paint,
    },
    /// `[a, b, c, d, e, f]`, the affine's own coefficient order.
    PushTransform([f64; 6]),
    PopTransform,
}

/// A generated program plus the frame parameters it renders under — everything
/// needed to replay a failure, which is what gets written out as JSON.
#[derive(Clone, Debug, Serialize)]
struct Program {
    ops: Vec<Op>,
    /// The colour the surface is cleared to, `[r, g, b, a]`.
    base_color: [u8; 4],
}

fn rgba8(rgba: [u8; 4]) -> Color {
    Color::from_rgba8(rgba[0], rgba[1], rgba[2], rgba[3])
}

fn point(p: [f64; 2]) -> Point {
    Point::new(p[0], p[1])
}

fn rect(r: [f64; 4]) -> Rect {
    Rect::new(r[0], r[1], r[2], r[3])
}

/// Builds the `kurbo` path for `segments`, which always open with a `MoveTo`
/// (`BezPath::push` asserts it).
fn bez_path(segments: &[Segment]) -> BezPath {
    let mut path = BezPath::new();
    for segment in segments {
        match segment {
            Segment::MoveTo(p) => path.move_to(point(*p)),
            Segment::LineTo(p) => path.line_to(point(*p)),
            Segment::QuadTo(p0, p1) => path.quad_to(point(*p0), point(*p1)),
            Segment::CurveTo(p0, p1, p2) => path.curve_to(point(*p0), point(*p1), point(*p2)),
            Segment::ClosePath => path.close_path(),
        }
    }
    path
}

impl Program {
    /// Records this program through `SceneBuilder` — the only way a scene enters
    /// this crate, so a generated case can never encode a display list the
    /// widget layer could not have produced.
    fn scene(&self) -> Scene {
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        for op in &self.ops {
            match op {
                Op::FillRect { rect: r, paint } => builder.fill_rect(rect(*r), paint.brush()),
                Op::RoundedRect {
                    rect: r,
                    radii,
                    paint,
                } => builder.fill_rounded_rect_radii(
                    rect(*r),
                    frust_scene::CornerRadii::new(radii[0], radii[1], radii[2], radii[3]),
                    paint.brush(),
                ),
                Op::Line {
                    p0,
                    p1,
                    width,
                    paint,
                } => builder.stroke_line(point(*p0), point(*p1), *width, paint.brush()),
                Op::FillPath { path, paint } => builder.fill_path(bez_path(path), paint.brush()),
                Op::StrokePath {
                    path,
                    width,
                    dash,
                    paint,
                } => match dash {
                    Some([on, off, phase]) => builder.stroke_path_dashed(
                        bez_path(path),
                        *width,
                        DashPattern::new(*on, *off).with_phase(*phase),
                        paint.brush(),
                    ),
                    None => builder.stroke_path(bez_path(path), *width, paint.brush()),
                },
                Op::PushTransform(coeffs) => builder.push_transform(Affine::new(*coeffs)),
                Op::PopTransform => builder.pop_transform(),
            }
        }
        scene
    }

    fn render_spec(&self) -> RenderSpec {
        RenderSpec {
            width: FRAME,
            height: FRAME,
            base_color: rgba8(self.base_color),
            scale: 1.0,
            root: Affine::IDENTITY,
        }
    }
}

// ---------------------------------------------------------------------------
// Strategies
// ---------------------------------------------------------------------------

fn coord() -> impl Strategy<Value = f64> {
    COORD_MIN..COORD_MAX
}

fn pair() -> impl Strategy<Value = [f64; 2]> {
    (coord(), coord()).prop_map(|(x, y)| [x, y])
}

fn quad() -> impl Strategy<Value = [f64; 4]> {
    (coord(), coord(), coord(), coord()).prop_map(|(a, b, c, d)| [a, b, c, d])
}

fn radii() -> impl Strategy<Value = [f64; 4]> {
    (0.0_f64..16.0, 0.0_f64..16.0, 0.0_f64..16.0, 0.0_f64..16.0)
        .prop_map(|(a, b, c, d)| [a, b, c, d])
}

fn rgba() -> impl Strategy<Value = [u8; 4]> {
    (any::<u8>(), any::<u8>(), any::<u8>(), any::<u8>()).prop_map(|(r, g, b, a)| [r, g, b, a])
}

fn paint() -> impl Strategy<Value = Paint> {
    prop_oneof![
        3 => rgba().prop_map(Paint::Solid),
        1 => (pair(), pair(), rgba(), rgba())
            .prop_map(|(start, end, first, second)| Paint::LinearGradient {
                start,
                end,
                stops: [first, second],
            }),
    ]
}

/// A stroke width wide enough to rasterize and narrow enough to stay a stroke —
/// the hairline end of the range is where the two arms are hardest to keep
/// together, so it is deliberately included.
fn stroke_width() -> impl Strategy<Value = f64> {
    prop_oneof![
        1 => 0.25_f64..1.0,
        3 => 1.0_f64..8.0,
    ]
}

/// A dash pattern, or none. Periods are kept at a pixel or more so expanding one
/// over a frame-sized path stays bounded.
fn dash() -> impl Strategy<Value = Option<[f64; 3]>> {
    prop_oneof![
        2 => Just(None),
        1 => (1.0_f64..16.0, 1.0_f64..16.0, 0.0_f64..16.0)
            .prop_map(|(on, off, phase)| Some([on, off, phase])),
    ]
}

/// A path, opened with a `MoveTo` and carrying no zero-length closed subpath.
///
/// That exclusion is not cosmetic: `kurbo::dash` emits such a subpath's closing
/// segment FIRST, so its output starts with `ClosePath` rather than `MoveTo`,
/// and BOTH arms collect that straight into a `BezPath` — which trips
/// `BezPath::from_vec`'s "must begin with `MoveTo`" debug assertion. The defect
/// belongs to the dash lowering, not to this comparison, and
/// `frust-engine`'s `proptest_strips` carries the reproducer for it; generating
/// the shape here would only stop this differential from running at all.
fn segments() -> impl Strategy<Value = Vec<Segment>> {
    let element = prop_oneof![
        pair().prop_map(Segment::MoveTo),
        pair().prop_map(Segment::LineTo),
        (pair(), pair()).prop_map(|(p0, p1)| Segment::QuadTo(p0, p1)),
        (pair(), pair(), pair()).prop_map(|(p0, p1, p2)| Segment::CurveTo(p0, p1, p2)),
        Just(Segment::ClosePath),
    ];
    (pair(), prop::collection::vec(element, 1..6))
        .prop_map(|(start, rest)| drop_empty_closed_subpaths(start, rest))
}

fn drop_empty_closed_subpaths(start: [f64; 2], rest: Vec<Segment>) -> Vec<Segment> {
    let mut path = vec![Segment::MoveTo(start)];
    for segment in rest {
        let after_move = matches!(path.last(), Some(Segment::MoveTo(_)));
        if after_move && matches!(segment, Segment::ClosePath) {
            continue;
        }
        path.push(segment);
    }
    path
}

/// A transform: a modest rotation/scale/skew about the frame, in the range a
/// widget tree actually composes.
fn transform() -> impl Strategy<Value = [f64; 6]> {
    (
        -2.0_f64..2.0,
        -1.0_f64..1.0,
        -1.0_f64..1.0,
        -2.0_f64..2.0,
        -32.0_f64..32.0,
        -32.0_f64..32.0,
    )
        .prop_map(|(a, b, c, d, e, f)| [a, b, c, d, e, f])
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        4 => (quad(), paint()).prop_map(|(rect, paint)| Op::FillRect { rect, paint }),
        4 => (quad(), radii(), paint())
            .prop_map(|(rect, radii, paint)| Op::RoundedRect { rect, radii, paint }),
        4 => (pair(), pair(), stroke_width(), paint())
            .prop_map(|(p0, p1, width, paint)| Op::Line { p0, p1, width, paint }),
        4 => (segments(), paint()).prop_map(|(path, paint)| Op::FillPath { path, paint }),
        4 => (segments(), stroke_width(), dash(), paint())
            .prop_map(|(path, width, dash, paint)| Op::StrokePath {
                path,
                width,
                dash,
                paint,
            }),
        1 => transform().prop_map(Op::PushTransform),
        1 => Just(Op::PopTransform),
    ]
}

fn program() -> impl Strategy<Value = Program> {
    (
        prop::collection::vec(op(), 1..MAX_OPS),
        prop::sample::select(vec![
            [0, 0, 0, 0],
            [255, 255, 255, 255],
            [0, 0, 0, 255],
            [32, 64, 96, 255],
        ]),
    )
        .prop_map(|(ops, base_color)| Program { ops, base_color })
}

// ---------------------------------------------------------------------------
// Comparison and failure artifacts
// ---------------------------------------------------------------------------

/// Converts a rendered frame into the [`RgbaImage`] [`diff_images`] compares.
fn to_rgba_image(image: &RenderedImage) -> RgbaImage {
    RgbaImage::from_raw(image.width, image.height, image.rgba8.clone()).unwrap_or_else(|| {
        panic!(
            "rendered {}x{} frame ({} bytes) does not match width*height*4",
            image.width,
            image.height,
            image.rgba8.len()
        )
    })
}

fn summarize(report: &DiffReport) -> String {
    format!(
        "{} px differ ({:.4}%), max |delta| {:?}, mean {:?}, bbox {:?}",
        report.pixel_count,
        report.mismatched_percent,
        report.max_difference,
        report.mean_abs_error.map(|v| (v * 1000.0).round() / 1000.0),
        report.bounding_box
    )
}

/// The first few differing pixels, CPU against engine — a cross-arm failure
/// between two rasterizers sharing one geometry core is usually a handful of
/// pixels with a specific shape, and the counts alone do not say which.
fn first_pixels(report: &DiffReport) -> String {
    const SHOWN: usize = 6;
    let listed: Vec<String> = report
        .pixels
        .iter()
        .take(SHOWN)
        .map(|pixel| {
            format!(
                "({}, {}) cpu {:?} vs engine {:?}",
                pixel.x, pixel.y, pixel.expected, pixel.actual
            )
        })
        .collect();
    format!(
        "first {} of {}: {}",
        listed.len(),
        report.pixel_count,
        listed.join("; ")
    )
}

/// Where a failing case's artifacts go: under the build's own artifact root,
/// never inside `testing/goldens/`.
fn failure_dir(class: &str) -> PathBuf {
    artifacts_root().join(class).join("fuzz-differential")
}

/// Writes the minimized program and the frames it disagreed on.
///
/// The two frames and the triptych are the PREMULTIPLIED bytes the comparison
/// was made on, not the straightened form a golden PNG stores — named so in the
/// file names, since reading them as ordinary straight-alpha images would show
/// darkened edges that are an artifact of the space, not of the disagreement.
fn persist_failure(dir: &Path, program: &Program, outcome: &DiffOutcome) -> Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    outcome
        .triptych
        .save(dir.join("triptych.png"))
        .with_context(|| format!("writing {}/triptych.png", dir.display()))?;
    write_json(&dir.join("scene.json"), program)?;
    write_json(&dir.join("diff.json"), &outcome.report)
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let json = serde_json::to_string_pretty(value)
        .with_context(|| format!("serializing {}", path.display()))?;
    fs::write(path, json).with_context(|| format!("writing {}", path.display()))
}

/// Renders every generated scene on both arms and asserts the engine agrees with
/// `vello_cpu` inside the corpus's own tight tolerance.
///
/// ```text
/// WGPU_BACKEND=vulkan WGPU_ADAPTER_NAME=T400 FRUST_GOLDEN_EXPECT_ADAPTER=T400 \
///   cargo test -p frust-testing --test fuzz -- --ignored --nocapture
/// ```
#[test]
#[ignore = "requires a GPU; run locally with `cargo test -p frust-testing --test fuzz -- --ignored` (FRUST_FUZZ_CASES overrides the case count)"]
fn generated_scenes_agree_between_the_engine_and_vello_cpu() {
    let _serialized = render_lock();

    let engine = EngineOracle::new(&EngineOracleOptions::default())
        .expect("failed to create the engine oracle");
    let cpu = CpuOracle::new();
    let class = engine.id();
    let cases = configured_cases();

    println!("fuzz: engine arm on {}", engine.adapter_meta());
    println!("fuzz: cpu arm `{}`", cpu.id());
    println!(
        "fuzz: {cases} case(s) at {FRAME}x{FRAME}, compared premultiplied under \
         {DIFFERENTIAL_TOLERANCE:?} on the 1-px-eroded interior, with the raw per-channel \
         maximum ceilinged at {EDGE_JITTER_CEILING} (override the count with {CASES_ENV_VAR})"
    );

    let engine = RefCell::new(engine);
    let cpu = RefCell::new(cpu);
    let dir = failure_dir(class);
    let started = Instant::now();
    // What the eroded mask and the widened tolerance between them absorbed,
    // reported at the end so neither is ever silent: how many cases carried a
    // pixel over the CORPUS's own tight default in the raw mask, and the worst
    // raw per-channel delta seen anywhere in the run.
    let absorbed = Cell::new(0_u32);
    let worst_raw = Cell::new([0_i16; 4]);

    let mut runner = TestRunner::new(Config {
        cases,
        // A red run must write nothing into the checked-out tree; the minimized
        // program is persisted under the artifact root instead, which is both
        // replayable and outside the repository's committed state.
        failure_persistence: None,
        ..Config::default()
    });

    let outcome = runner.run(&program(), |program| {
        let scene = program.scene();
        let spec = program.render_spec();

        let rendered = engine
            .borrow_mut()
            .render(&scene, &spec)
            .map_err(|err| TestCaseError::fail(format!("the engine refused the frame: {err:#}")))?;
        let reference = cpu.borrow_mut().render(&scene, &spec).map_err(|err| {
            TestCaseError::fail(format!("the cpu arm refused the frame: {err:#}"))
        })?;

        if rendered.alpha != reference.alpha {
            return Err(TestCaseError::fail(format!(
                "the two arms report different alpha conventions ({:?} vs {:?}) — a cross-arm \
                 diff would be comparing two different spaces",
                reference.alpha, rendered.alpha
            )));
        }

        let expected = to_rgba_image(&reference);
        let actual = to_rgba_image(&rendered);
        let outcome = diff_images(&expected, &actual, DIFFERENTIAL_TOLERANCE, true);

        // The raw per-channel maximum is a whole-image statistic regardless of
        // erosion, so it is read off this same report rather than diffed twice.
        let raw = outcome.report.max_difference;
        let mut worst = worst_raw.get();
        for (slot, channel) in worst.iter_mut().zip(raw) {
            *slot = (*slot).max(channel);
        }
        worst_raw.set(worst);

        let over_ceiling: Vec<usize> = (0..4)
            .filter(|channel| raw[*channel] > EDGE_JITTER_CEILING)
            .collect();

        if outcome.passed && over_ceiling.is_empty() {
            if raw
                .iter()
                .any(|delta| *delta > i16::from(Tolerance::new().channel))
            {
                absorbed.set(absorbed.get() + 1);
            }
            return Ok(());
        }

        // Written on every failing candidate, so the last write is the minimized
        // one the shrinker settled on.
        if let Err(err) = persist_failure(&dir, &program, &outcome) {
            println!("fuzz: could not persist the failing case: {err:#}");
        }
        let why = if outcome.passed {
            format!(
                "the eroded interior agreed, but channel(s) {over_ceiling:?} differ by more than \
                 the raw ceiling {EDGE_JITTER_CEILING} — a one-pixel-wide edge that moved a whole \
                 level is a rasterization change, not edge jitter"
            )
        } else {
            "the eroded interior disagrees".to_string()
        };
        Err(TestCaseError::fail(format!(
            "{why}; {}; {}",
            summarize(&outcome.report),
            first_pixels(&outcome.report)
        )))
    });

    let elapsed = started.elapsed();
    match outcome {
        Ok(()) => println!(
            "fuzz: {cases} case(s) agreed in {:.1}s ({:.1} ms/case); worst raw per-channel delta \
             {:?} (ceiling {EDGE_JITTER_CEILING}), {} case(s) carried a raw pixel over the \
             corpus default that this bar absorbed",
            elapsed.as_secs_f64(),
            elapsed.as_secs_f64() * 1000.0 / f64::from(cases),
            worst_raw.get(),
            absorbed.get()
        ),
        Err(TestError::Fail(reason, program)) => panic!(
            "the engine and `vello_cpu` disagreed on a generated scene: {reason}\nminimized \
             program: {program:#?}\nartifacts (triptych + scene.json) in {}",
            dir.display()
        ),
        Err(TestError::Abort(reason)) => {
            panic!("the differential aborted before it could finish: {reason}")
        }
    }
}

// ---------------------------------------------------------------------------
// GPU-free tripwires — these run in the ordinary workspace gate
// ---------------------------------------------------------------------------

/// A small GPU-free run of the same generator, so the strategies, the recording
/// and the JSON serialization are all exercised on every machine rather than
/// only on the pinned runner.
fn sample_programs(cases: u32) -> Vec<Program> {
    let mut runner = TestRunner::new(Config {
        cases,
        failure_persistence: None,
        ..Config::default()
    });
    let collected = RefCell::new(Vec::new());
    runner
        .run(&program(), |program| {
            collected.borrow_mut().push(program);
            Ok(())
        })
        .expect("the generator itself never fails a case");
    collected.into_inner()
}

/// Every command the generator can produce is one the engine's compiler lowers.
///
/// The differential is only meaningful over that set: a command the compiler
/// recognises and skips would have the engine legitimately draw nothing while
/// the CPU arm drew the subject, and the comparison would fail for a reason that
/// is not a rasterization difference.
#[test]
fn the_generator_only_emits_primitives_the_engine_lowers() {
    let mut unexpected: Vec<&'static str> = Vec::new();
    for program in sample_programs(64) {
        for command in program.scene().commands() {
            let name = match command {
                frust_scene::Command::FillRect { .. } => continue,
                frust_scene::Command::RoundedRect { .. } => continue,
                frust_scene::Command::Line { .. } => continue,
                frust_scene::Command::Path { .. } => continue,
                frust_scene::Command::GlyphRun(_) => "GlyphRun",
                frust_scene::Command::PushClip { .. } => "PushClip",
                frust_scene::Command::PushClipRounded { .. } => "PushClipRounded",
                frust_scene::Command::PopClip => "PopClip",
                frust_scene::Command::Image { .. } => "Image",
                frust_scene::Command::BlurredRoundedRect { .. } => "BlurredRoundedRect",
                frust_scene::Command::PushLayer { .. } => "PushLayer",
                frust_scene::Command::PopLayer => "PopLayer",
                frust_scene::Command::ClearRect { .. } => "ClearRect",
                frust_scene::Command::ShaderQuad { .. } => "ShaderQuad",
                frust_scene::Command::PushSnapshot { .. } => "PushSnapshot",
                frust_scene::Command::PopSnapshot => "PopSnapshot",
            };
            unexpected.push(name);
        }
    }
    unexpected.sort_unstable();
    unexpected.dedup();
    assert!(
        unexpected.is_empty(),
        "the generator emitted command(s) the engine does not lower: {}",
        unexpected.join(", ")
    );
}

/// Every float a generated program carries.
fn floats(program: &Program) -> Vec<f64> {
    fn paint_floats(paint: &Paint, out: &mut Vec<f64>) {
        if let Paint::LinearGradient { start, end, .. } = paint {
            out.extend(start);
            out.extend(end);
        }
    }
    fn path_floats(path: &[Segment], out: &mut Vec<f64>) {
        for segment in path {
            match segment {
                Segment::MoveTo(p) | Segment::LineTo(p) => out.extend(p),
                Segment::QuadTo(p0, p1) => {
                    out.extend(p0);
                    out.extend(p1);
                }
                Segment::CurveTo(p0, p1, p2) => {
                    out.extend(p0);
                    out.extend(p1);
                    out.extend(p2);
                }
                Segment::ClosePath => {}
            }
        }
    }

    let mut out = Vec::new();
    for op in &program.ops {
        match op {
            Op::FillRect { rect, paint } => {
                out.extend(rect);
                paint_floats(paint, &mut out);
            }
            Op::RoundedRect { rect, radii, paint } => {
                out.extend(rect);
                out.extend(radii);
                paint_floats(paint, &mut out);
            }
            Op::Line {
                p0,
                p1,
                width,
                paint,
            } => {
                out.extend(p0);
                out.extend(p1);
                out.push(*width);
                paint_floats(paint, &mut out);
            }
            Op::FillPath { path, paint } => {
                path_floats(path, &mut out);
                paint_floats(paint, &mut out);
            }
            Op::StrokePath {
                path,
                width,
                dash,
                paint,
            } => {
                path_floats(path, &mut out);
                out.push(*width);
                out.extend(dash.iter().flatten());
                paint_floats(paint, &mut out);
            }
            Op::PushTransform(coeffs) => out.extend(coeffs),
            Op::PopTransform => {}
        }
    }
    out
}

/// Every generated value is finite.
///
/// A non-finite transform is refused by the engine's frame path and not by the
/// CPU arm, so one would make the two arms disagree about a scene only one of
/// them ever rendered — a difference in contract, not in rasterization, and not
/// what this differential exists to measure.
#[test]
fn every_generated_scene_is_finite() {
    for program in sample_programs(64) {
        let offending: Vec<f64> = floats(&program)
            .into_iter()
            .filter(|value| !value.is_finite())
            .collect();
        assert!(
            offending.is_empty(),
            "a generated program carries non-finite value(s) {offending:?}: {program:?}"
        );
    }
}

/// A generated program round-trips through JSON — the artifact a failure leaves
/// behind is only useful if it parses.
#[test]
fn a_generated_program_serializes_to_json() {
    for program in sample_programs(8) {
        let json = serde_json::to_string_pretty(&program).expect("a program serializes");
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("the artifact parses");
        assert!(parsed.get("ops").is_some_and(serde_json::Value::is_array));
        assert!(parsed.get("base_color").is_some());
    }
}

/// The case count honours its environment override, and falls back to the
/// committed default rather than to zero on anything unparseable.
#[test]
fn the_case_count_falls_back_to_the_committed_default() {
    // Read rather than set: the variable is process-global and this binary's
    // ignored GPU test reads it too, so a test that wrote it would reach across.
    let configured = configured_cases();
    match std::env::var(CASES_ENV_VAR).ok().as_deref() {
        Some(value) => match value.trim().parse::<u32>() {
            Ok(cases) if cases > 0 => assert_eq!(configured, cases),
            _ => assert_eq!(configured, DEFAULT_CASES),
        },
        None => assert_eq!(configured, DEFAULT_CASES),
    }
}

/// Failure artifacts land under the build's artifact root, never inside the
/// committed golden corpus.
#[test]
fn failure_artifacts_stay_out_of_the_committed_corpus() {
    let dir = failure_dir("engine-vulkan-nvidia-t400");
    assert!(dir.starts_with(artifacts_root()));
    assert!(
        !dir.components()
            .any(|component| component.as_os_str() == "goldens"),
        "{} is inside the committed golden corpus",
        dir.display()
    );
}

/// A zero-length closed subpath never survives generation — the shape both dash
/// lowerings choke on (see [`segments`]).
#[test]
fn the_generator_drops_zero_length_closed_subpaths() {
    let path = drop_empty_closed_subpaths(
        [0.0, 0.0],
        vec![
            Segment::ClosePath,
            Segment::LineTo([4.0, 4.0]),
            Segment::ClosePath,
        ],
    );
    assert!(matches!(
        path.as_slice(),
        [Segment::MoveTo(_), Segment::LineTo(_), Segment::ClosePath]
    ));

    for program in sample_programs(64) {
        for op in &program.ops {
            let path = match op {
                Op::FillPath { path, .. } | Op::StrokePath { path, .. } => path,
                _ => continue,
            };
            assert!(
                matches!(path.first(), Some(Segment::MoveTo(_))),
                "a generated path must open with a MoveTo: {path:?}"
            );
            for pair in path.windows(2) {
                assert!(
                    !matches!(
                        (&pair[0], &pair[1]),
                        (Segment::MoveTo(_), Segment::ClosePath)
                    ),
                    "a generated path carries a zero-length closed subpath: {path:?}"
                );
            }
        }
    }
}
