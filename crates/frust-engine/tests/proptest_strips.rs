//! Property coverage over [`SceneCompiler`]: whatever display list reaches it,
//! the frame it produces is well-formed or refused, never wrong and never a
//! panic.
//!
//! Four properties, all over the public seam — a scene recorded through
//! `frust_scene::SceneBuilder` and handed to [`SceneCompiler::compile`]:
//!
//! - **Refusal, not panic.** Any command sequence, with geometry drawn from a
//!   pool of the float values that break naive arithmetic, either compiles or
//!   comes back as one of the two errors the frame path is allowed to report.
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
//! Rect corners, line endpoints and transform translations carry the whole
//! hostile pool: the compiler either clamps them to the viewport, culls them, or
//! refuses the frame. Stroke widths, corner radii, path points, dash periods and
//! the linear part of a transform are bounded instead, because each of those
//! scales the amount of geometry the *flattener and stroker* produce before any
//! culling happens — a round cap of radius `1e18` or a dash period of `0.1` over
//! a path `1e18` long is a generator that never returns, not a compiler defect.
//! Those magnitudes reach the compiler through the unbounded pools above, where
//! they cost nothing.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::OnceLock;

use frust_engine::{CompiledFrame, EngineError, SceneCompiler};
use frust_scene::{
    CornerRadii, DashPattern, FontHandle, Glyph, GlyphRun, Scene, SceneBuilder, ShaderProgram,
};
use kurbo::{Affine, BezPath, PathEl, Point, Rect};
use peniko::color::palette::css::{BLUE, GREEN, RED};
use peniko::{Blob, Brush, Color, FontData, Gradient, ImageAlphaType, ImageData, ImageFormat};
use proptest::prelude::*;
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
static CHEAP_HOSTILE_COORDS: [f64; 7] = [
    0.0,
    -0.0,
    f64::NAN,
    f64::MIN_POSITIVE,
    -f64::MIN_POSITIVE,
    SMALLEST_SUBNORMAL,
    -SMALLEST_SUBNORMAL,
];

/// The hostile values a transform's LINEAR part is drawn from.
///
/// [`CHEAP_HOSTILE_COORDS`] plus the two infinities, and pointedly without the
/// huge finite magnitudes: a non-finite coefficient is refused by the frame path
/// before a single strip is generated, so it is free, while a finite `1e18`
/// passes that check and then multiplies every coordinate the flattener
/// subdivides — the flattener's cost, not the compiler's contract. Those
/// magnitudes still reach the compiler, through the geometry and translation
/// pools where they cost nothing.
static HOSTILE_LINEAR_COEFFS: [f64; 9] = [
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
/// `path_elements`, whose corner arcs subdivide in proportion to the radius —
/// and finite, because of an open defect this suite found.
///
/// **A `NaN` corner radius on a rect of unbounded extent makes
/// `SceneCompiler::compile` never return.** It is not a panic and not a
/// refusal; the call simply does not terminate, which on a UI thread is a frozen
/// application. The exact case, reproduced deterministically:
///
/// ```text
/// rect  = Rect::new(f64::MAX, f64::INFINITY, 205.5012069531332, 209.20270810843607)
/// radii = CornerRadii::new(-2.9228123760991815, -5e-324, f64::NAN, 85.76283149609571)
/// root  = Affine::new([2.192344744008792, 0.0, 0.0, 2.192344744008792,
///                      169.1676407022096, 142.6733979215257])
/// ```
///
/// Neither half alone is enough: a `NaN` radius on an ordinary rect compiles,
/// and an unbounded rect with finite radii compiles. There is deliberately NO
/// runnable reproducer for this the way the dash defect has one
/// ([`a_dashed_zero_length_closed_subpath_panics_the_dash_lowering`]) — a test
/// that never returns cannot be `#[ignore]`d into safety, it would hang whoever
/// ran the ignored set. Write one once the lowering bounds its own walk, and
/// widen this pool back to [`bounded_coord`] in the same change.
fn radii() -> impl Strategy<Value = CornerRadii> {
    let radius = prop_oneof![
        7 => -256.0_f64..256.0,
        3 => prop::sample::select(vec![
            0.0_f64,
            -0.0,
            f64::MIN_POSITIVE,
            -f64::MIN_POSITIVE,
            SMALLEST_SUBNORMAL,
            -SMALLEST_SUBNORMAL,
        ]),
    ];
    (radius.clone(), radius.clone(), radius.clone(), radius)
        .prop_map(|(tl, tr, br, bl)| CornerRadii::new(tl, tr, br, bl))
}

/// Stroke width: bounded for the same reason as [`radii`] (round caps and joins
/// are arcs of that width), and finite because of an open defect this suite
/// found.
///
/// **A `NaN` stroke width makes the stroker do unbounded work for nothing.**
/// Measured on this rig, one dashed quadratic — `MoveTo (0, 0)`, `QuadTo
/// (-82.5525294392934, 0), (0, 122.89384759607046)`, dash `on 1 / off 1` — at
/// `width: NaN` costs 420 ms and 4.8 MB and emits ZERO strips, against 0.18 ms
/// and 81 KB at `width: 2.0`; a solid stroke at `NaN` still costs 5 ms and
/// 131 KB for no strips. Longer generated paths scale it into minutes, which is
/// why the pool excludes it rather than the allocation property absorbing it
/// into a bound nobody could read. Zero, negative and subnormal widths all stay
/// — each is cheap, and each is a real degenerate a widget can record.
fn stroke_width() -> impl Strategy<Value = f64> {
    prop_oneof![
        7 => -256.0_f64..256.0,
        3 => prop::sample::select(vec![
            0.0_f64,
            -0.0,
            f64::MIN_POSITIVE,
            -f64::MIN_POSITIVE,
            SMALLEST_SUBNORMAL,
            -SMALLEST_SUBNORMAL,
        ]),
    ]
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
            prop::sample::select(&HOSTILE_LINEAR_COEFFS[..]),
            -4.0_f64..4.0,
            -4.0_f64..4.0,
            prop::sample::select(&HOSTILE_LINEAR_COEFFS[..]),
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

/// A point for a path that will be FILLED. Carries the whole cheap-hostile pool,
/// `NaN` included: the flattener detects a non-finite path and drops it, at a
/// cost of microseconds.
fn fill_point() -> impl Strategy<Value = Point> {
    bounded_point()
}

/// A point for a path that will be STROKED — finite, because of the same open
/// defect [`stroke_width`] documents, seen from its other side.
///
/// The stroker does not bail on non-finite input either: measured on this rig, a
/// single `NaN`-control-point quadratic costs about 6 ms and 130 KB to stroke
/// and emits ZERO strips, against 0.3 ms and 26 KB for the same curve with a
/// subnormal control point, while FILLING the same `NaN` path costs 14 us. Times
/// a generated scene's commands and this suite's case count, that alone is tens
/// of seconds of the workspace gate spent proving nothing. The `NaN` path still
/// reaches the compiler on every filled arm above, where it is free.
fn stroke_point() -> impl Strategy<Value = Point> {
    let value = prop_oneof![
        7 => -256.0_f64..256.0,
        3 => prop::sample::select(vec![
            0.0_f64,
            -0.0,
            f64::MIN_POSITIVE,
            -f64::MIN_POSITIVE,
            SMALLEST_SUBNORMAL,
            -SMALLEST_SUBNORMAL,
        ]),
    ];
    (value.clone(), value).prop_map(|(x, y)| Point::new(x, y))
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

/// Drops every `ClosePath` that directly follows a `MoveTo`, i.e. every
/// zero-length closed subpath.
///
/// Applied to a DASHED stroke's path only, and it is a deliberate hole in this
/// suite's coverage rather than a tidy-up: such a path makes the compiler's dash
/// lowering panic in a debug build today, which
/// [`a_dashed_zero_length_closed_subpath_panics_the_dash_lowering`] reproduces
/// in full. Sanitizing here keeps the four properties measuring everything else
/// instead of re-finding that one defect on every run; remove this call when the
/// lowering is fixed.
fn without_empty_closed_subpaths(path: BezPath) -> BezPath {
    let mut out = BezPath::new();
    let mut after_move = false;
    for element in path.elements() {
        if after_move && matches!(element, PathEl::ClosePath) {
            continue;
        }
        after_move = matches!(element, PathEl::MoveTo(_));
        out.push(*element);
    }
    out
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
        (bez_path(fill_point().boxed()), brush())
            .prop_map(|(path, brush)| Op::FillPath { path, brush }),
        (
            bez_path(stroke_point().boxed()),
            stroke_width(),
            dash(),
            brush()
        )
            .prop_map(|(path, width, dash, brush)| Op::StrokePath {
                path: match dash {
                    Some(_) => without_empty_closed_subpaths(path),
                    None => path,
                },
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
    /// one of the two errors the frame path is allowed to raise — never a
    /// panic, and never a third error nobody documented.
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
            Err(EngineError::InvalidTransform | EngineError::TargetTooLarge) => {}
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

/// An open refusal-contract gap, found by this suite and reproduced here in
/// full: a DASHED stroke over a path whose first subpath is a zero-length closed
/// one panics instead of compiling or being refused.
///
/// `kurbo::dash` emits that empty subpath's closing segment first, so its output
/// begins with `ClosePath` rather than `MoveTo`; the compiler's dash lowering
/// collects that straight into a `BezPath`, and `BezPath::from_vec`'s "must
/// begin with `MoveTo`" debug assertion fires. A release build has the assertion
/// compiled out and strokes a malformed path instead — neither outcome is the
/// documented one, which is an `EngineError` on every frame path.
///
/// Reachable from ordinary widget code: a dashed arc at zero sweep records
/// exactly this path. The generator above sanitizes the shape away
/// ([`without_empty_closed_subpaths`]) so the four properties can measure
/// everything else; un-ignore this the moment the lowering handles it.
#[test]
#[ignore = "reproduces an open defect in the compiler's dash lowering (a dashed zero-length \
            closed subpath panics rather than compiling or being refused) — run with `cargo test \
            -p frust-engine --test proptest_strips -- --ignored` to confirm it is still open"]
fn a_dashed_zero_length_closed_subpath_panics_the_dash_lowering() {
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
        compiled.is_ok() || matches!(compiled, Err(EngineError::InvalidTransform)),
        "the frame path must compile this or refuse it, never panic"
    );
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
