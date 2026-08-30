//! Property coverage over [`SceneCompiler`]: whatever display list reaches it,
//! the frame it produces is well-formed or refused, never wrong and never a
//! panic.
//!
//! Four properties, all over the public seam — a scene recorded through
//! `frust_scene::SceneBuilder` and handed to [`SceneCompiler::compile`]:
//!
//! - **Refusal, not panic.** Any command sequence, with geometry drawn from a
//!   pool of the float values that break naive arithmetic, either compiles or
//!   comes back as one of the three errors the frame path is allowed to report.
//!   The commands the compiler does not lower yet are skipped, so a frame draws
//!   less, never wrong — which is also why the generator emits them.
//! - **Allocation bound.** A frame's allocation stays inside a bound linear in
//!   the command count, measured by a counting global allocator. The point is
//!   not the constant, which carries deliberate headroom, but the *shape*: a
//!   compiler that started allocating per tile per command, or that leaked a
//!   scratch buffer per draw, would leave the line.
//! - **Idempotent re-compile.** The same scene compiled twice — into the same
//!   compiler, whose generator resets per call, and into a fresh one — yields
//!   byte-identical strips, alphas and draws. A compiler carrying state across
//!   frames would diverge on the second pass.
//! - **Strip packing.** Every strip lands inside the tile-snapped viewport, no
//!   two strips of one draw overlap within a row, every draw's run ends in the
//!   sentinel the renderer's `windows(2)` walk depends on, and every alpha index
//!   plus its own coverage width stays inside the frame's alpha buffer.
//!
//! # Why some pools are bounded and others are not
//!
//! Every pool carries the degenerate values — the zeros, the subnormals, `NaN`
//! and the infinities. A non-finite one costs nothing wherever it appears: the
//! compiler refuses such a frame from its up-front walk, before a single strip
//! is generated, so the case is decided by a scan over the display list rather
//! than by whatever the flattener would have made of the number.
//!
//! What separates the pools is MAGNITUDE, and only magnitude. Rect corners,
//! line endpoints and transform translations carry the huge finite values too
//! (`1e18`, `f64::MAX`): the compiler either clamps them to the viewport, culls
//! them, or refuses the frame, and a line's own flattening is exact regardless
//! of how far apart its ends are. Stroke widths, corner radii, path points and
//! the linear part of a transform stop at `256`, because each of those scales
//! the amount of geometry the *flattener and stroker* produce before any
//! culling happens — a round cap of radius `1e18`, or a dash period of `0.1`
//! over a path `1e18` long, is a generator that never returns, and no up-front
//! finiteness check can refuse it because every number in it is finite. Those
//! magnitudes still reach the compiler, through the unbounded pools above,
//! where they cost nothing.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::OnceLock;

use frust_engine::{CompiledFrame, EngineError, SceneCompiler};
use frust_scene::{
    CornerRadii, DashPattern, FontHandle, Glyph, GlyphRun, Scene, SceneBuilder, ShaderProgram,
};
use kurbo::{Affine, BezPath, Point, Rect};
use peniko::color::palette::css::{BLUE, GREEN, RED};
use peniko::{Blob, Brush, Color, FontData, Gradient, ImageAlphaType, ImageData, ImageFormat};
use proptest::prelude::*;
use proptest::test_runner::TestRunner;
use vello_common::strip::Strip;
use vello_common::tile::Tile;

/// Viewport every case compiles against. Small on purpose: the properties are
/// about packing and bounds, not about coverage area, and a small frame keeps a
/// few hundred generated cases inside the workspace gate's time budget.
const VIEWPORT: (u16, u16) = (64, 64);

/// Cases per property.
const CASES: u32 = 128;

/// Cases for the allocation property, which measures rather than merely asserts
/// and is therefore the most expensive of the four.
const ALLOCATION_CASES: u32 = 64;

/// Longest generated scene.
const MAX_OPS: usize = 16;

/// The smallest positive subnormal — exponent field zero, mantissa one bit.
const SMALLEST_SUBNORMAL: f64 = f64::from_bits(1);

/// Coordinates the compiler either clamps, culls or refuses, so their magnitude
/// costs nothing: rect corners, line endpoints, transform translations.
static HOSTILE_COORDS: [f64; 13] = [
    0.0,
    -0.0,
    f64::NAN,
    f64::INFINITY,
    f64::NEG_INFINITY,
    f64::MIN_POSITIVE,
    -f64::MIN_POSITIVE,
    SMALLEST_SUBNORMAL,
    -SMALLEST_SUBNORMAL,
    1e18,
    -1e18,
    f64::MAX,
    f64::MIN,
];

/// The subset of [`HOSTILE_COORDS`] that costs the flattener and the stroker
/// nothing — every degenerate shape, none of the magnitudes that would make
/// either subdivide in proportion to the number.
///
/// The two infinities are cheap for the same reason `NaN` is: non-finite input
/// is refused by the frame path's up-front walk, so it never reaches either.
/// Only the huge FINITE values are missing, and they are the whole difference
/// between this pool and [`HOSTILE_COORDS`] (see the module docs).
///
/// This is the pool for everything measured against that walk: a transform's
/// linear part, corner radii, stroke widths, path points.
static CHEAP_HOSTILE_COORDS: [f64; 9] = [
    0.0,
    -0.0,
    f64::NAN,
    f64::INFINITY,
    f64::NEG_INFINITY,
    f64::MIN_POSITIVE,
    -f64::MIN_POSITIVE,
    SMALLEST_SUBNORMAL,
    -SMALLEST_SUBNORMAL,
];

// ---------------------------------------------------------------------------
// Counting allocator
// ---------------------------------------------------------------------------

thread_local! {
    /// Bytes this thread has asked the allocator for since the counter was last
    /// reset. Per thread, not global: the test harness runs several properties
    /// concurrently in this one binary, and a global counter would measure all
    /// of them at once.
    static ALLOCATED_BYTES: Cell<u64> = const { Cell::new(0) };
}

/// A pass-through allocator that adds up what it hands out.
///
/// Growth only: a `realloc` counts the difference, and `dealloc` counts nothing,
/// so the total answers "how many bytes did this frame ask for", which is the
/// question the bound is stated in. Peak residency would answer a different one
/// and would hide a compiler that churned a buffer per draw.
struct CountingAllocator;

// SAFETY: every method forwards to `System` with the pointer and layout it was
// given, unchanged; the counter is a thread-local `Cell<u64>` with no
// destructor, so accounting can neither allocate nor observe a partially
// initialized thread. `GlobalAlloc`'s contract is therefore exactly `System`'s.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count(layout.size());
        // SAFETY: `layout` is forwarded verbatim.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count(layout.size());
        // SAFETY: `layout` is forwarded verbatim.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr`/`layout` are forwarded verbatim, and this allocator
        // returned `ptr` from `System` in the first place.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        count(new_size.saturating_sub(layout.size()));
        // SAFETY: `ptr`/`layout`/`new_size` are forwarded verbatim, and this
        // allocator returned `ptr` from `System` in the first place.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

fn count(bytes: usize) {
    // `try_with` rather than `with`: an allocation on a thread whose locals are
    // already torn down must not panic inside the allocator.
    let _ = ALLOCATED_BYTES.try_with(|counter| counter.set(counter.get() + bytes as u64));
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// Runs `body`, returning its value and the bytes it allocated on this thread.
fn measure_allocation<T>(body: impl FnOnce() -> T) -> (T, u64) {
    let before = ALLOCATED_BYTES.with(Cell::get);
    let value = body();
    let after = ALLOCATED_BYTES.with(Cell::get);
    (value, after - before)
}

// ---------------------------------------------------------------------------
// Shared resource identities
// ---------------------------------------------------------------------------

fn shared_font() -> FontHandle {
    static FONT: OnceLock<FontHandle> = OnceLock::new();
    FONT.get_or_init(|| FontHandle::new(FontData::new(Blob::from(vec![1_u8, 2, 3, 4]), 0)))
        .clone()
}

fn shared_image() -> ImageData {
    static IMAGE: OnceLock<ImageData> = OnceLock::new();
    IMAGE
        .get_or_init(|| ImageData {
            data: Blob::from(vec![0_u8; 2 * 2 * 4]),
            format: ImageFormat::Rgba8,
            alpha_type: ImageAlphaType::Alpha,
            width: 2,
            height: 2,
        })
        .clone()
}

fn shared_shader() -> ShaderProgram {
    static SHADER: OnceLock<ShaderProgram> = OnceLock::new();
    SHADER
        .get_or_init(|| ShaderProgram::new("fn main() {}"))
        .clone()
}

// ---------------------------------------------------------------------------
// The generated scene
// ---------------------------------------------------------------------------

/// One builder call, covering all sixteen `frust_scene::Command` variants: the
/// four the compiler lowers, and the twelve it recognises and skips — a skip is
/// part of the contract under test, so the generator has to reach it.
#[derive(Clone, Debug)]
enum Op {
    FillRect {
        rect: Rect,
        brush: Brush,
    },
    RoundedRect {
        rect: Rect,
        radii: CornerRadii,
        brush: Brush,
    },
    Line {
        p0: Point,
        p1: Point,
        width: f64,
        brush: Brush,
    },
    FillPath {
        path: BezPath,
        brush: Brush,
    },
    StrokePath {
        path: BezPath,
        width: f64,
        dash: Option<DashPattern>,
        brush: Brush,
    },
    GlyphRun {
        font_size: f32,
        glyphs: Vec<Glyph>,
        brush: Brush,
    },
    Image {
        dest: Rect,
    },
    BlurredRoundedRect {
        rect: Rect,
        radii: CornerRadii,
        std_dev: f64,
        color: Color,
    },
    ClearRect {
        rect: Rect,
    },
    ShaderQuad {
        dest: Rect,
        time: f32,
    },
    PushClip {
        rect: Rect,
    },
    PushClipRounded {
        rect: Rect,
        radii: CornerRadii,
    },
    PopClip,
    PushLayer {
        rect: Rect,
        alpha: f32,
    },
    PopLayer,
    PushSnapshot {
        key: u64,
        rect: Rect,
        alpha: f32,
        scale: f64,
    },
    PopSnapshot,
    PushTransform(Affine),
    PopTransform,
}

/// Records `ops` into a fresh scene.
fn scene_of(ops: &[Op]) -> Scene {
    let mut scene = Scene::new();
    let mut builder = SceneBuilder::new(&mut scene);
    for op in ops {
        match op {
            Op::FillRect { rect, brush } => builder.fill_rect(*rect, brush.clone()),
            Op::RoundedRect { rect, radii, brush } => {
                builder.fill_rounded_rect_radii(*rect, *radii, brush.clone());
            }
            Op::Line {
                p0,
                p1,
                width,
                brush,
            } => builder.stroke_line(*p0, *p1, *width, brush.clone()),
            Op::FillPath { path, brush } => builder.fill_path(path.clone(), brush.clone()),
            Op::StrokePath {
                path,
                width,
                dash,
                brush,
            } => match dash {
                Some(dash) => {
                    builder.stroke_path_dashed(path.clone(), *width, *dash, brush.clone());
                }
                None => builder.stroke_path(path.clone(), *width, brush.clone()),
            },
            Op::GlyphRun {
                font_size,
                glyphs,
                brush,
            } => builder.draw_glyph_run(GlyphRun {
                font: shared_font(),
                font_size: *font_size,
                brush: brush.clone(),
                transform: Affine::IDENTITY,
                glyphs: glyphs.clone(),
            }),
            Op::Image { dest } => builder.draw_image(&shared_image(), *dest),
            Op::BlurredRoundedRect {
                rect,
                radii,
                std_dev,
                color,
            } => builder.draw_blurred_rounded_rect_radii(*rect, *radii, *std_dev, *color),
            Op::ClearRect { rect } => builder.clear_rect(*rect),
            Op::ShaderQuad { dest, time } => builder.draw_shader(&shared_shader(), *dest, *time),
            Op::PushClip { rect } => builder.push_clip(*rect),
            Op::PushClipRounded { rect, radii } => builder.push_clip_rounded_radii(*rect, *radii),
            Op::PopClip => builder.pop_clip(),
            Op::PushLayer { rect, alpha } => builder.push_layer(*rect, *alpha),
            Op::PopLayer => builder.pop_layer(),
            Op::PushSnapshot {
                key,
                rect,
                alpha,
                scale,
            } => builder.push_snapshot(*key, *rect, *alpha, *scale),
            Op::PopSnapshot => builder.pop_snapshot(),
            Op::PushTransform(transform) => builder.push_transform(*transform),
            Op::PopTransform => builder.pop_transform(),
        }
    }
    scene
}

// ---------------------------------------------------------------------------
// Strategies
// ---------------------------------------------------------------------------

/// A coordinate the compiler clamps, culls or refuses — the full hostile pool.
fn coord() -> impl Strategy<Value = f64> {
    prop_oneof![
        7 => -256.0_f64..256.0,
        3 => prop::sample::select(&HOSTILE_COORDS[..]),
    ]
}

/// A coordinate that reaches the flattener or the stroker, where magnitude buys
/// subdivision rather than coverage (see the module docs).
fn bounded_coord() -> impl Strategy<Value = f64> {
    prop_oneof![
        7 => -256.0_f64..256.0,
        3 => prop::sample::select(&CHEAP_HOSTILE_COORDS[..]),
    ]
}

fn small_f32() -> impl Strategy<Value = f32> {
    prop_oneof![
        7 => -256.0_f32..256.0,
        3 => prop::sample::select(vec![0.0_f32, -0.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY]),
    ]
}

fn rect() -> impl Strategy<Value = Rect> {
    (coord(), coord(), coord(), coord()).prop_map(|(x0, y0, x1, y1)| Rect::new(x0, y0, x1, y1))
}

fn point() -> impl Strategy<Value = Point> {
    (coord(), coord()).prop_map(|(x, y)| Point::new(x, y))
}

fn bounded_point() -> impl Strategy<Value = Point> {
    (bounded_coord(), bounded_coord()).prop_map(|(x, y)| Point::new(x, y))
}

/// Corner radii: bounded, because a rounded rect lowers through
/// `path_elements`, whose corner arcs subdivide in proportion to the radius.
///
/// Non-finite radii are in the pool and cost nothing, because the compiler
/// refuses a frame carrying one before the lowering runs. That refusal is what
/// this pool waited on: a `NaN` radius on a rect of unbounded extent used to
/// make `SceneCompiler::compile` never return — not a panic and not a refusal,
/// simply a call that did not terminate, which on a UI thread is a frozen
/// application. [`the_documented_non_terminating_rounded_rect_is_refused`]
/// holds the exact case.
fn radii() -> impl Strategy<Value = CornerRadii> {
    (
        bounded_coord(),
        bounded_coord(),
        bounded_coord(),
        bounded_coord(),
    )
        .prop_map(|(tl, tr, br, bl)| CornerRadii::new(tl, tr, br, bl))
}

/// Stroke width: bounded for the same reason as [`radii`] — round caps and
/// joins are arcs of that width — and degenerate all the way to `NaN`, which
/// the frame path refuses before the stroker sees it.
///
/// That refusal is what this pool waited on too. Measured on this rig, one
/// dashed quadratic — `MoveTo (0, 0)`, `QuadTo (-82.5525294392934, 0), (0,
/// 122.89384759607046)`, dash `on 1 / off 1` — at `width: NaN` cost 420 ms and
/// 4.8 MB and emitted ZERO strips, against 0.18 ms and 81 KB at `width: 2.0`;
/// a solid stroke at `NaN` still cost 5 ms and 131 KB for no strips. Longer
/// generated paths scaled that into minutes. Zero, negative and subnormal
/// widths are in the pool on their own merit — each is cheap, each compiles,
/// and each is a real degenerate a widget can record.
fn stroke_width() -> impl Strategy<Value = f64> {
    bounded_coord()
}

/// A transform: a bounded linear part (it multiplies every flattened
/// coordinate) over an unbounded translation, with a hostile coefficient often
/// enough to exercise the frame path's non-finite refusal.
fn transform() -> impl Strategy<Value = Affine> {
    prop_oneof![
        7 => (
            -4.0_f64..4.0,
            -4.0_f64..4.0,
            -4.0_f64..4.0,
            -4.0_f64..4.0,
            coord(),
            coord(),
        )
            .prop_map(|(a, b, c, d, e, f)| Affine::new([a, b, c, d, e, f])),
        3 => (
            prop::sample::select(&CHEAP_HOSTILE_COORDS[..]),
            -4.0_f64..4.0,
            -4.0_f64..4.0,
            prop::sample::select(&CHEAP_HOSTILE_COORDS[..]),
            coord(),
            coord(),
        )
            .prop_map(|(a, b, c, d, e, f)| Affine::new([a, b, c, d, e, f])),
    ]
}

fn color() -> impl Strategy<Value = Color> {
    prop::sample::select(vec![RED, GREEN, BLUE, Color::TRANSPARENT])
}

fn brush() -> impl Strategy<Value = Brush> {
    prop_oneof![
        3 => color().prop_map(Brush::Solid),
        1 => (bounded_point(), bounded_point()).prop_map(|(start, end)| Brush::Gradient(
            Gradient::new_linear(start, end).with_stops([RED, BLUE])
        )),
    ]
}

/// A dash pattern: either none, an effective one whose period is large enough
/// that expanding it over a bounded path stays bounded, or a degenerate one that
/// `DashPattern::is_effective` refuses and the compiler strokes solid.
fn dash() -> impl Strategy<Value = Option<DashPattern>> {
    prop_oneof![
        3 => Just(None),
        2 => (1.0_f64..32.0, 1.0_f64..32.0, -32.0_f64..32.0)
            .prop_map(|(on, off, phase)| Some(DashPattern::new(on, off).with_phase(phase))),
        1 => (
            prop::sample::select(&HOSTILE_COORDS[..]),
            prop::sample::select(&HOSTILE_COORDS[..]),
            prop::sample::select(&HOSTILE_COORDS[..]),
        )
            .prop_map(|(on, off, phase)| Some(DashPattern::new(on, off).with_phase(phase))),
    ]
}

/// A point for a path, filled or stroked alike — one pool for both, now that
/// neither rasterizing arm ever sees a non-finite one.
///
/// Filling and stroking used to need different pools. The flattener bails on a
/// non-finite path cheaply, but the stroker does not: measured on this rig, a
/// single `NaN`-control-point quadratic cost about 6 ms and 130 KB to stroke
/// and emitted ZERO strips, against 0.3 ms and 26 KB for the same curve with a
/// subnormal control point, while FILLING the same `NaN` path cost 14 us.
/// Refusing the frame up front costs a scan of the display list either way.
fn path_point() -> impl Strategy<Value = Point> {
    bounded_point()
}

/// A path over `vertex`, always opened with a `MoveTo` — `BezPath::push` asserts
/// it, so a generator that emitted anything else would be testing kurbo's debug
/// assertion rather than the compiler.
fn bez_path(vertex: BoxedStrategy<Point>) -> impl Strategy<Value = BezPath> {
    let element = prop_oneof![
        vertex.clone().prop_map(PathElement::MoveTo),
        vertex.clone().prop_map(PathElement::LineTo),
        (vertex.clone(), vertex.clone()).prop_map(|(p0, p1)| PathElement::QuadTo(p0, p1)),
        (vertex.clone(), vertex.clone(), vertex.clone())
            .prop_map(|(p0, p1, p2)| PathElement::CurveTo(p0, p1, p2)),
        Just(PathElement::ClosePath),
    ];
    (vertex, prop::collection::vec(element, 0..6)).prop_map(|(start, elements)| {
        let mut path = BezPath::new();
        path.move_to(start);
        for element in elements {
            element.push_onto(&mut path);
        }
        path
    })
}

#[derive(Clone, Copy, Debug)]
enum PathElement {
    MoveTo(Point),
    LineTo(Point),
    QuadTo(Point, Point),
    CurveTo(Point, Point, Point),
    ClosePath,
}

impl PathElement {
    fn push_onto(self, path: &mut BezPath) {
        match self {
            Self::MoveTo(p) => path.move_to(p),
            Self::LineTo(p) => path.line_to(p),
            Self::QuadTo(p0, p1) => path.quad_to(p0, p1),
            Self::CurveTo(p0, p1, p2) => path.curve_to(p0, p1, p2),
            Self::ClosePath => path.close_path(),
        }
    }
}

fn glyphs() -> impl Strategy<Value = Vec<Glyph>> {
    prop::collection::vec(
        (any::<u32>(), small_f32(), small_f32()).prop_map(|(id, x, y)| Glyph { id, x, y }),
        0..4,
    )
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (rect(), brush()).prop_map(|(rect, brush)| Op::FillRect { rect, brush }),
        (rect(), radii(), brush()).prop_map(|(rect, radii, brush)| Op::RoundedRect {
            rect,
            radii,
            brush
        }),
        (point(), point(), stroke_width(), brush()).prop_map(|(p0, p1, width, brush)| Op::Line {
            p0,
            p1,
            width,
            brush
        }),
        (bez_path(path_point().boxed()), brush())
            .prop_map(|(path, brush)| Op::FillPath { path, brush }),
        (
            bez_path(path_point().boxed()),
            stroke_width(),
            dash(),
            brush()
        )
            .prop_map(|(path, width, dash, brush)| Op::StrokePath {
                path,
                width,
                dash,
                brush,
            }),
        (small_f32(), glyphs(), brush()).prop_map(|(font_size, glyphs, brush)| Op::GlyphRun {
            font_size,
            glyphs,
            brush
        }),
        rect().prop_map(|dest| Op::Image { dest }),
        (rect(), radii(), coord(), color()).prop_map(|(rect, radii, std_dev, color)| {
            Op::BlurredRoundedRect {
                rect,
                radii,
                std_dev,
                color,
            }
        }),
        rect().prop_map(|rect| Op::ClearRect { rect }),
        (rect(), small_f32()).prop_map(|(dest, time)| Op::ShaderQuad { dest, time }),
        rect().prop_map(|rect| Op::PushClip { rect }),
        (rect(), radii()).prop_map(|(rect, radii)| Op::PushClipRounded { rect, radii }),
        Just(Op::PopClip),
        (rect(), small_f32()).prop_map(|(rect, alpha)| Op::PushLayer { rect, alpha }),
        Just(Op::PopLayer),
        (any::<u64>(), rect(), small_f32(), coord()).prop_map(|(key, rect, alpha, scale)| {
            Op::PushSnapshot {
                key,
                rect,
                alpha,
                scale,
            }
        }),
        Just(Op::PopSnapshot),
        transform().prop_map(Op::PushTransform),
        Just(Op::PopTransform),
    ]
}

fn ops() -> impl Strategy<Value = Vec<Op>> {
    prop::collection::vec(op(), 0..MAX_OPS)
}

/// The root transform a frame is compiled under — the same shape a shell's
/// device scale composes, plus the non-finite arm the frame path refuses.
fn root() -> impl Strategy<Value = Affine> {
    prop_oneof![
        3 => Just(Affine::IDENTITY),
        4 => (0.5_f64..3.0, coord(), coord())
            .prop_map(|(scale, dx, dy)| Affine::translate((dx, dy)) * Affine::scale(scale)),
        1 => transform(),
    ]
}

// ---------------------------------------------------------------------------
// Allocation bound
// ---------------------------------------------------------------------------

/// The constant part of the allocation bound: the buffers one frame needs
/// regardless of what it draws (the recorder's own tables, the strip storage's
/// initial growth).
const ALLOCATION_BASE_BYTES: u64 = 512 * 1024;

/// The per-command part. A command's own frame cost is bounded by the viewport
/// — a draw cannot emit more strips than the tile-snapped frame has tile
/// columns, nor more coverage than those strips address — so this is a viewport
/// constant, generously rounded up, not a measurement of any particular scene.
const ALLOCATION_PER_COMMAND_BYTES: u64 = 256 * 1024;

fn allocation_bound(command_count: usize) -> u64 {
    ALLOCATION_BASE_BYTES + (command_count as u64) * ALLOCATION_PER_COMMAND_BYTES
}

// ---------------------------------------------------------------------------
// Strip-packing invariants
// ---------------------------------------------------------------------------

/// The extent strips are addressed in: the viewport snapped up to whole tiles,
/// which is what the strip generator itself rounds to.
fn snapped_viewport() -> (u32, u32) {
    (
        u32::from(VIEWPORT.0.next_multiple_of(Tile::WIDTH)),
        u32::from(VIEWPORT.1.next_multiple_of(Tile::HEIGHT)),
    )
}

/// Every way a frame's strip packing can be malformed, as review-ready messages.
///
/// Reported all at once rather than asserted one at a time so a failing case
/// names every broken invariant, not merely the first.
fn strip_packing_failures(frame: &CompiledFrame) -> Vec<String> {
    let (width, height) = snapped_viewport();
    let strips = frame.strip_buf();
    let alphas = frame.alphas().len() as u32;
    let mut failures = Vec::new();

    for (index, draw) in frame.draws().iter().enumerate() {
        let range = draw.strip_range.clone();
        let Some(run) = strips.get(range.clone()) else {
            failures.push(format!(
                "draw {index}: strip range {range:?} is outside the frame's {} strips",
                strips.len()
            ));
            continue;
        };
        if run.len() < 2 {
            failures.push(format!(
                "draw {index}: a recorded draw needs at least one strip plus its sentinel, got {}",
                run.len()
            ));
            continue;
        }
        if !run[run.len() - 1].is_sentinel() {
            failures.push(format!(
                "draw {index}: the run does not end in a sentinel — the renderer's pairwise walk \
                 reads a strip's extent off the one after it"
            ));
        }

        let mut previous: Option<(&Strip, u32)> = None;
        for pair in run.windows(2) {
            let (strip, next) = (&pair[0], &pair[1]);
            if strip.is_sentinel() {
                failures.push(format!(
                    "draw {index}: a sentinel at ({}, {}) is not the last strip of the run",
                    strip.x, strip.y
                ));
                continue;
            }
            let strip_width = u32::from(strip.width_to(next));
            let x0 = u32::from(strip.x);

            if x0 + strip_width > width {
                failures.push(format!(
                    "draw {index}: strip at ({}, {}) spans {strip_width}px, past the {width}px \
                     tile-snapped viewport",
                    strip.x, strip.y
                ));
            }
            if u32::from(strip.y) >= height {
                failures.push(format!(
                    "draw {index}: strip row {} is past the {height}px tile-snapped viewport",
                    strip.y
                ));
            }
            if u32::from(strip.y) % u32::from(Tile::HEIGHT) != 0 {
                failures.push(format!(
                    "draw {index}: strip row {} is not tile-aligned",
                    strip.y
                ));
            }

            let coverage = strip_width * u32::from(Tile::HEIGHT);
            if strip.alpha_idx().saturating_add(coverage) > alphas {
                failures.push(format!(
                    "draw {index}: strip at ({}, {}) reads coverage {}..{} out of the frame's \
                     {alphas}-byte alpha buffer",
                    strip.x,
                    strip.y,
                    strip.alpha_idx(),
                    strip.alpha_idx().saturating_add(coverage)
                ));
            }

            if let Some((previous, previous_end)) = previous
                && previous.y == strip.y
                && x0 < previous_end
            {
                failures.push(format!(
                    "draw {index}: strip at ({}, {}) starts inside the one before it, which ended \
                     at {previous_end}",
                    strip.x, strip.y
                ));
            }
            previous = Some((strip, x0 + strip_width));
        }
    }
    failures
}

/// Whether two compiles produced the same frame, byte for byte.
fn frames_differ(first: &CompiledFrame, second: &CompiledFrame) -> Option<String> {
    if first.strip_buf() != second.strip_buf() {
        return Some(format!(
            "strips differ: {} vs {}",
            first.strip_buf().len(),
            second.strip_buf().len()
        ));
    }
    if first.alphas() != second.alphas() {
        return Some(format!(
            "alpha coverage differs: {} vs {} bytes",
            first.alphas().len(),
            second.alphas().len()
        ));
    }
    if first.draws().len() != second.draws().len() {
        return Some(format!(
            "draw count differs: {} vs {}",
            first.draws().len(),
            second.draws().len()
        ));
    }
    for (index, (a, b)) in first.draws().iter().zip(second.draws()).enumerate() {
        if a.paint != b.paint || a.depth != b.depth || a.strip_range != b.strip_range {
            return Some(format!("draw {index} differs: {a:?} vs {b:?}"));
        }
    }
    if first.lut_requests != second.lut_requests {
        return Some("gradient ramp requests differ".to_string());
    }
    if first.encoded_paints.len() != second.encoded_paints.len() {
        return Some(format!(
            "encoded paint count differs: {} vs {}",
            first.encoded_paints.len(),
            second.encoded_paints.len()
        ));
    }
    if first.fast_rect_draws != second.fast_rect_draws {
        return Some(format!(
            "fast-rectangle draw count differs: {} vs {}",
            first.fast_rect_draws, second.fast_rect_draws
        ));
    }
    None
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: CASES,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    /// Compiling any scene under any root transform either succeeds or reports
    /// one of the three errors the frame path is allowed to raise — never a
    /// panic, and never a fourth error nobody documented.
    #[test]
    fn compiling_any_scene_refuses_rather_than_panicking(ops in ops(), root in root()) {
        let scene = scene_of(&ops);
        let mut compiler = SceneCompiler::new(VIEWPORT.0, VIEWPORT.1);

        match compiler.compile(&scene, root, VIEWPORT) {
            Ok(frame) => {
                // A recorded draw always owns strips: the compiler drops a
                // generation that produced none rather than recording an empty
                // draw, which is what keeps depths dense over real geometry.
                for draw in frame.draws() {
                    prop_assert!(!draw.strip_range.is_empty());
                }
                prop_assert!(frame.fast_rect_draws as usize <= frame.draws().len());
            }
            Err(
                EngineError::InvalidTransform
                | EngineError::InvalidGeometry
                | EngineError::TargetTooLarge,
            ) => {}
            Err(other) => prop_assert!(
                false,
                "the frame path reported an error outside its documented set: {other:?}"
            ),
        }
    }

    /// The same scene compiled twice is byte-identical, into the same compiler
    /// and into a fresh one alike — the compiler holds no per-frame state.
    #[test]
    fn compiling_the_same_scene_twice_is_byte_identical(ops in ops(), root in root()) {
        let scene = scene_of(&ops);
        let mut compiler = SceneCompiler::new(VIEWPORT.0, VIEWPORT.1);

        let Ok(first) = compiler.compile(&scene, root, VIEWPORT) else {
            return Ok(());
        };
        let second = compiler
            .compile(&scene, root, VIEWPORT)
            .expect("a scene that compiled once compiles again");
        let third = SceneCompiler::new(VIEWPORT.0, VIEWPORT.1)
            .compile(&scene, root, VIEWPORT)
            .expect("a scene that compiled once compiles in a fresh compiler");

        prop_assert!(
            frames_differ(&first, &second).is_none(),
            "the same compiler produced a different frame the second time: {}",
            frames_differ(&first, &second).unwrap_or_default()
        );
        prop_assert!(
            frames_differ(&first, &third).is_none(),
            "a fresh compiler produced a different frame: {}",
            frames_differ(&first, &third).unwrap_or_default()
        );
    }

    /// Every strip a frame emits is addressable: inside the tile-snapped
    /// viewport, tile-aligned, non-overlapping within its row, and pointing at
    /// coverage the frame's own alpha buffer actually holds.
    #[test]
    fn strip_packing_stays_inside_the_frame(ops in ops(), root in root()) {
        let scene = scene_of(&ops);
        let mut compiler = SceneCompiler::new(VIEWPORT.0, VIEWPORT.1);

        let Ok(frame) = compiler.compile(&scene, root, VIEWPORT) else {
            return Ok(());
        };
        let failures = strip_packing_failures(&frame);
        prop_assert!(
            failures.is_empty(),
            "{} malformed strip(s):\n{}",
            failures.len(),
            failures.join("\n")
        );
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: ALLOCATION_CASES,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    /// A frame's allocation stays inside a bound linear in the command count.
    #[test]
    fn frame_allocation_stays_inside_a_linear_bound(ops in ops(), root in root()) {
        let scene = scene_of(&ops);
        let commands = scene.commands().len();
        // Built outside the measured window: this is the retained scratch a
        // caller creates once per surface, not part of a frame's cost.
        let mut compiler = SceneCompiler::new(VIEWPORT.0, VIEWPORT.1);

        let (frame, allocated) = measure_allocation(|| compiler.compile(&scene, root, VIEWPORT));
        prop_assume!(frame.is_ok());

        let bound = allocation_bound(commands);
        prop_assert!(
            allocated <= bound,
            "a {commands}-command frame allocated {allocated} bytes, past the {bound}-byte bound \
             ({ALLOCATION_BASE_BYTES} + {commands} x {ALLOCATION_PER_COMMAND_BYTES})"
        );
    }
}

/// The counting allocator actually counts — the allocation property is worth
/// exactly as much as this.
#[test]
fn the_counting_allocator_observes_an_allocation() {
    let ((), allocated) = measure_allocation(|| {
        let buffer = vec![0_u8; 64 * 1024];
        // Kept alive across the measurement so the compiler cannot elide it.
        assert_eq!(buffer.len(), 64 * 1024);
    });
    assert!(
        allocated >= 64 * 1024,
        "a 64 KiB allocation was measured as {allocated} bytes"
    );

    let ((), quiet) = measure_allocation(|| {});
    assert_eq!(quiet, 0, "an empty body allocated {quiet} bytes");
}

/// The empty scene is the boundary case every property above steps over: it
/// compiles, records nothing, and still satisfies the packing invariants
/// vacuously.
#[test]
fn an_empty_scene_compiles_to_an_empty_frame() {
    let scene = Scene::new();
    let frame = SceneCompiler::new(VIEWPORT.0, VIEWPORT.1)
        .compile(&scene, Affine::IDENTITY, VIEWPORT)
        .expect("the empty scene compiles");

    assert!(frame.draws().is_empty());
    assert!(frame.strip_buf().is_empty());
    assert!(strip_packing_failures(&frame).is_empty());
}

/// The shape that used to break the compiler's dash lowering: a DASHED stroke
/// over a path whose first subpath is a zero-length closed one.
///
/// `kurbo::dash` emits that empty subpath's closing segment first, so its output
/// begins with `ClosePath` rather than `MoveTo`; collecting that straight into a
/// `BezPath` trips the "must begin with `MoveTo`" debug assertion, and a release
/// build with the assertion compiled out strokes a malformed path instead —
/// neither outcome is the documented one, which is a frame or an `EngineError`.
///
/// Reachable from ordinary widget code: a dashed arc at zero sweep records
/// exactly this path, which is why the generator above now emits the shape
/// freely rather than sanitizing it away.
#[test]
fn a_dashed_zero_length_closed_subpath_compiles_rather_than_panicking() {
    let mut path = BezPath::new();
    path.move_to((0.0, 0.0));
    path.close_path();

    let scene = scene_of(&[Op::StrokePath {
        path,
        width: 1.0,
        dash: Some(DashPattern::new(13.0, 1.0)),
        brush: Brush::Solid(RED),
    }]);

    let compiled =
        SceneCompiler::new(VIEWPORT.0, VIEWPORT.1).compile(&scene, Affine::IDENTITY, VIEWPORT);
    assert!(
        compiled.is_ok(),
        "an effective dash pattern over this shape must compile, never panic: {compiled:?}"
    );
}

/// The same shape wherever a path can carry it, dashed and solid alike: leading,
/// trailing, doubled, and with nothing but closes.
///
/// The leading case is the one that panicked, but it is the *class* that has to
/// be closed — a lowering that special-cased position alone would leave the
/// next arrangement of the same degenerate open.
#[test]
fn zero_length_closed_subpaths_compile_wherever_they_sit_in_a_path() {
    let point = Point::new(8.0, 8.0);
    let elsewhere = Point::new(40.0, 24.0);

    let shapes: Vec<(&str, Vec<PathElement>)> = vec![
        ("close only", vec![PathElement::ClosePath]),
        (
            "doubled close",
            vec![PathElement::ClosePath, PathElement::ClosePath],
        ),
        (
            "leading, then a real subpath",
            vec![
                PathElement::ClosePath,
                PathElement::MoveTo(elsewhere),
                PathElement::LineTo(point),
                PathElement::ClosePath,
            ],
        ),
        (
            "trailing, after a real subpath",
            vec![
                PathElement::LineTo(elsewhere),
                PathElement::ClosePath,
                PathElement::MoveTo(point),
                PathElement::ClosePath,
            ],
        ),
        (
            "close following a closed subpath",
            vec![
                PathElement::LineTo(elsewhere),
                PathElement::ClosePath,
                PathElement::ClosePath,
            ],
        ),
        (
            "between two real subpaths",
            vec![
                PathElement::LineTo(elsewhere),
                PathElement::MoveTo(point),
                PathElement::ClosePath,
                PathElement::MoveTo(elsewhere),
                PathElement::QuadTo(point, elsewhere),
            ],
        ),
    ];

    for (name, elements) in shapes {
        let mut path = BezPath::new();
        path.move_to(point);
        for element in elements {
            element.push_onto(&mut path);
        }

        for dash in [
            Some(DashPattern::new(13.0, 1.0)),
            Some(DashPattern::new(0.5, 0.5).with_phase(0.25)),
            None,
        ] {
            let scene = scene_of(&[Op::StrokePath {
                path: path.clone(),
                width: 2.0,
                dash,
                brush: Brush::Solid(RED),
            }]);

            let compiled = SceneCompiler::new(VIEWPORT.0, VIEWPORT.1).compile(
                &scene,
                Affine::IDENTITY,
                VIEWPORT,
            );
            assert!(
                compiled.is_ok(),
                "{name} with dash {dash:?} must compile, never panic: {compiled:?}"
            );
        }
    }
}

/// The rounded rect that used to make `SceneCompiler::compile` never return,
/// reproduced exactly: an unbounded extent and a `NaN` corner radius.
///
/// Neither half alone was enough — a `NaN` radius on an ordinary rect compiled,
/// and an unbounded rect with finite radii compiled — which is why this pair is
/// spelled out rather than left to the generator to rediscover. It is a runnable
/// test only because the refusal is now decided before the lowering runs; while
/// the walk was unbounded there was nothing to assert, since a test that never
/// returns cannot be `#[ignore]`d into safety.
#[test]
fn the_documented_non_terminating_rounded_rect_is_refused() {
    let scene = scene_of(&[Op::RoundedRect {
        rect: Rect::new(
            f64::MAX,
            f64::INFINITY,
            205.5012069531332,
            209.20270810843607,
        ),
        radii: CornerRadii::new(-2.9228123760991815, -5e-324, f64::NAN, 85.76283149609571),
        brush: Brush::Solid(RED),
    }]);
    let root = Affine::new([
        2.192344744008792,
        0.0,
        0.0,
        2.192344744008792,
        169.1676407022096,
        142.6733979215257,
    ]);

    assert!(matches!(
        SceneCompiler::new(VIEWPORT.0, VIEWPORT.1).compile(&scene, root, VIEWPORT),
        Err(EngineError::InvalidGeometry)
    ));
}

/// The dash pattern that used to make `kurbo::dash`'s own catch-up loop spin
/// forever, reproduced exactly: `on = off = f64::MAX`, `phase = -1.0`.
///
/// Every field is individually finite, and [`DashPattern::is_effective`]
/// reads it as effective — `on` and `off` are both positive and their sum,
/// `+inf`, still compares `>=` the epsilon. The period only breaks once it is
/// *derived*: `on + off` overflows to `+inf`, and `phase.rem_euclid(+inf)`
/// overflows with it, so kurbo's setup loop (run before it ever pulls a
/// `PathEl` from the source path) adds an infinite step to a value that never
/// converges. This is a runnable test rather than a documented hang for the
/// same reason as the rounded-rect one above: the refusal is decided before
/// `kurbo::dash` ever runs.
#[test]
fn the_documented_non_terminating_dash_cycle_is_refused() {
    let scene = scene_of(&[Op::StrokePath {
        path: diamond(),
        width: 2.0,
        dash: Some(DashPattern::new(f64::MAX, f64::MAX).with_phase(-1.0)),
        brush: Brush::Solid(RED),
    }]);

    assert!(matches!(
        SceneCompiler::new(VIEWPORT.0, VIEWPORT.1).compile(&scene, Affine::IDENTITY, VIEWPORT),
        Err(EngineError::InvalidGeometry)
    ));
}

/// Every geometry field the compiler lowers is refused when it is non-finite,
/// and refused as geometry rather than as a transform — a caller chasing a
/// blank frame is told which half of the input they recorded wrong.
#[test]
fn non_finite_geometry_is_refused_field_by_field() {
    let ok = Rect::new(0.0, 0.0, 16.0, 16.0);

    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let mut bad_path = BezPath::new();
        bad_path.move_to((0.0, 0.0));
        bad_path.quad_to((bad, 4.0), (8.0, 8.0));

        let cases: Vec<(&str, Op)> = vec![
            (
                "rect extent",
                Op::FillRect {
                    rect: Rect::new(0.0, 0.0, bad, 16.0),
                    brush: Brush::Solid(RED),
                },
            ),
            (
                "corner radius",
                Op::RoundedRect {
                    rect: ok,
                    radii: CornerRadii::new(2.0, bad, 2.0, 2.0),
                    brush: Brush::Solid(RED),
                },
            ),
            (
                "line endpoint",
                Op::Line {
                    p0: Point::new(0.0, bad),
                    p1: Point::new(8.0, 8.0),
                    width: 1.0,
                    brush: Brush::Solid(RED),
                },
            ),
            (
                "line width",
                Op::Line {
                    p0: Point::new(0.0, 0.0),
                    p1: Point::new(8.0, 8.0),
                    width: bad,
                    brush: Brush::Solid(RED),
                },
            ),
            (
                "stroke width",
                Op::StrokePath {
                    path: diamond(),
                    width: bad,
                    dash: None,
                    brush: Brush::Solid(RED),
                },
            ),
            (
                "dash phase",
                Op::StrokePath {
                    path: diamond(),
                    width: 2.0,
                    dash: Some(DashPattern::new(4.0, 2.0).with_phase(bad)),
                    brush: Brush::Solid(RED),
                },
            ),
            (
                "dash length",
                Op::StrokePath {
                    path: diamond(),
                    width: 2.0,
                    dash: Some(DashPattern::new(bad, 2.0)),
                    brush: Brush::Solid(RED),
                },
            ),
            (
                "filled path point",
                Op::FillPath {
                    path: bad_path.clone(),
                    brush: Brush::Solid(RED),
                },
            ),
            (
                "stroked path point",
                Op::StrokePath {
                    path: bad_path.clone(),
                    width: 2.0,
                    dash: None,
                    brush: Brush::Solid(RED),
                },
            ),
        ];

        for (name, op) in cases {
            let scene = scene_of(&[op]);
            assert!(
                matches!(
                    SceneCompiler::new(VIEWPORT.0, VIEWPORT.1).compile(
                        &scene,
                        Affine::IDENTITY,
                        VIEWPORT
                    ),
                    Err(EngineError::InvalidGeometry)
                ),
                "a {bad} {name} must be refused as geometry"
            );
        }
    }
}

/// A command the compiler recognises but does not lower carries no geometry
/// into the frame, so a non-finite number in one is skipped with the command
/// rather than refusing the whole frame.
///
/// The distinction is the difference between drawing less and drawing nothing:
/// an image the compiler has not implemented yet must not be able to blank a
/// frame the geometry beside it would have rendered.
///
/// A clip is deliberately not the case here any more, and neither is a layer,
/// a snapshot bracket or a clear: the compiler lowers all four now, so each
/// one's own numbers are geometry the frame is refused for, pinned by
/// `tests/clips.rs` and `tests/layers.rs`.
#[test]
fn non_finite_geometry_in_a_skipped_command_does_not_refuse_the_frame() {
    let scene = scene_of(&[
        Op::Image {
            dest: Rect::new(f64::NAN, 0.0, f64::INFINITY, 16.0),
        },
        Op::FillRect {
            rect: Rect::new(0.0, 0.0, 16.0, 16.0),
            brush: Brush::Solid(RED),
        },
    ]);

    let frame = SceneCompiler::new(VIEWPORT.0, VIEWPORT.1)
        .compile(&scene, Affine::IDENTITY, VIEWPORT)
        .expect("a skipped command's geometry does not refuse the frame");
    assert_eq!(frame.draws().len(), 1);
}

/// The generator still reaches the compiler often enough for the properties
/// above to mean anything.
///
/// Three of the four return early on a refused frame — they are statements
/// about a frame, and a refusal produced none — so a pool hostile enough to
/// refuse nearly every case would leave them passing over nothing at all, with
/// no test failing to say so. The floor sits far below the measured rate rather
/// than pinned to it — eight samples of 256 scenes each compiled 51 to 73 of
/// their 256, which is what hostile pools cost once the compiler refuses what
/// is in them — so this guards against a collapse, not against ordinary drift
/// in the pools.
///
/// The rate falls each time the compiler starts lowering another command,
/// because a lowered command's own numbers become numbers the frame is refused
/// for: it was around 97 of 256 while layers, snapshot brackets and clears were
/// still skipped, and each of those three refuses roughly a quarter of the
/// rectangles this generator hands it. That is the refusal working, not the
/// generator decaying, so the floor is re-measured rather than the refusal
/// narrowed.
#[test]
fn the_generator_still_produces_frames_that_compile() {
    const SAMPLES: u32 = 256;

    let mut runner = TestRunner::new(ProptestConfig {
        cases: SAMPLES,
        failure_persistence: None,
        ..ProptestConfig::default()
    });
    let compiled = Cell::new(0_u32);

    runner
        .run(&(ops(), root()), |(ops, root)| {
            let scene = scene_of(&ops);
            if SceneCompiler::new(VIEWPORT.0, VIEWPORT.1)
                .compile(&scene, root, VIEWPORT)
                .is_ok()
            {
                compiled.set(compiled.get() + 1);
            }
            Ok(())
        })
        .expect("the generator itself never fails a case");

    let compiled = compiled.get();
    assert!(
        compiled * 8 >= SAMPLES,
        "only {compiled} of {SAMPLES} generated scenes compiled — the properties that return \
         early on a refusal are close to vacuous"
    );
}

/// A four-point closed diamond well inside the viewport — geometry that
/// compiles on its own, so a case built from it fails only for the field it set
/// out to test.
fn diamond() -> BezPath {
    let mut path = BezPath::new();
    path.move_to((16.0, 4.0));
    path.line_to((28.0, 16.0));
    path.line_to((16.0, 28.0));
    path.line_to((4.0, 16.0));
    path.close_path();
    path
}

/// A non-finite root transform is refused before any strip is generated, so a
/// rejected frame never leaves half its draws recorded.
#[test]
fn a_non_finite_root_transform_is_refused_whole() {
    let scene = scene_of(&[Op::FillRect {
        rect: Rect::new(0.0, 0.0, 16.0, 16.0),
        brush: Brush::Solid(RED),
    }]);

    for root in [
        Affine::new([f64::NAN, 0.0, 0.0, 1.0, 0.0, 0.0]),
        Affine::new([1.0, 0.0, 0.0, 1.0, f64::INFINITY, 0.0]),
    ] {
        assert!(matches!(
            SceneCompiler::new(VIEWPORT.0, VIEWPORT.1).compile(&scene, root, VIEWPORT),
            Err(EngineError::InvalidTransform)
        ));
    }
}
