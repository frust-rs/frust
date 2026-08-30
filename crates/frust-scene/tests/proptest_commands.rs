//! Property coverage over the display list itself: whatever a widget throws at
//! [`SceneBuilder`], the recording stays well-formed.
//!
//! Three properties, all driven through the public builder rather than by
//! constructing [`Command`]s by hand — a scene a widget could not have recorded
//! proves nothing about the recorder:
//!
//! - **Acceptance.** An arbitrary sequence of builder calls, with geometry drawn
//!   from a pool of the float values that break naive arithmetic (both zeroes,
//!   `NaN`, both infinities, both subnormal edges, `1e18`, `f64::MAX`/`MIN`),
//!   records without panicking. The builder validates nothing and refuses
//!   nothing; that is the contract, and this is what keeps it true.
//! - **Balance.** A properly nested sequence leaves the command stream properly
//!   nested — every `Pop*` matched, no counter driven below zero, the snapshot
//!   depth back at zero and the transform stack unwound to its base identity.
//!   An unmatched pop follows the recorded policy exactly: `PopClip`/`PopLayer`
//!   are *recorded* (a renderer ignores them), `pop_snapshot` records nothing at
//!   all, and `pop_transform` never pops the base identity.
//! - **Fingerprint invariance.** A body re-recorded under a whole-bracket
//!   translate/scale fingerprints EQUAL to the same body recorded at the
//!   identity, when each is hashed relative to its own bracket transform. That
//!   is the property a snapshot cache survives an animation frame on.
//!
//! Case counts are fixed constants rather than proptest's default, so this suite
//! stays inside the workspace gate's time budget; failure persistence is off so
//! that a red run writes nothing into the checked-out tree (the shrunk program
//! is printed in full by the failure itself).

use std::sync::OnceLock;

use frust_scene::{
    Command, CornerRadii, DashPattern, FontHandle, Glyph, GlyphRun, Scene, SceneBuilder,
    ShaderProgram, fingerprint_commands,
};
use kurbo::{Affine, BezPath, Point, Rect};
use peniko::color::palette::css::{BLUE, GREEN, RED};
use peniko::{Blob, Brush, Color, FontData, Gradient, ImageAlphaType, ImageData, ImageFormat};
use proptest::prelude::*;

/// Cases per property. Deliberately modest: this suite runs in the ordinary
/// `cargo test --workspace` gate, and recording is cheap enough that a few
/// hundred programs cover the variant space many times over.
const CASES: u32 = 256;

/// Longest generated program. Long enough for brackets to nest several deep,
/// short enough that a failing case shrinks to something a reader can follow.
const MAX_OPS: usize = 24;

/// The smallest positive subnormal — the float whose exponent field is zero and
/// whose mantissa is a single bit.
const SMALLEST_SUBNORMAL: f64 = f64::from_bits(1);

/// The coordinate values a generated command's geometry is drawn from when the
/// generator picks the hostile arm.
///
/// Every one of these breaks some plausible piece of naive geometry arithmetic:
/// the two zeroes differ in sign but compare equal, `NaN` compares false against
/// itself, the infinities survive multiplication but not subtraction from each
/// other, the subnormals lose precision under any scale, and the huge magnitudes
/// overflow to infinity when squared.
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

/// The `f32` twins of [`HOSTILE_COORDS`], for the fields the display list keeps
/// in single precision (glyph positions, alphas, shader time).
static HOSTILE_COORDS_F32: [f32; 9] = [
    0.0,
    -0.0,
    f32::NAN,
    f32::INFINITY,
    f32::NEG_INFINITY,
    f32::MIN_POSITIVE,
    -f32::MIN_POSITIVE,
    1e18,
    -1e18,
];

/// The one font every generated glyph run shares.
///
/// Identity matters: `peniko::Blob::id()` is minted per *construction*, not per
/// content, and the fingerprint hashes that id — two independently built fonts
/// would never fingerprint equal even with identical bytes, which would make the
/// invariance property fail for a reason that is not the fingerprint's.
fn shared_font() -> FontHandle {
    static FONT: OnceLock<FontHandle> = OnceLock::new();
    FONT.get_or_init(|| FontHandle::new(FontData::new(Blob::from(vec![1_u8, 2, 3, 4]), 0)))
        .clone()
}

/// The one image every generated draw shares — same identity argument as
/// [`shared_font`].
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

/// The one shader program every generated quad shares — `ShaderProgram::new`
/// mints a fresh process-unique id per call, and the fingerprint hashes it.
fn shared_shader() -> ShaderProgram {
    static SHADER: OnceLock<ShaderProgram> = OnceLock::new();
    SHADER
        .get_or_init(|| ShaderProgram::new("fn main() {}"))
        .clone()
}

// ---------------------------------------------------------------------------
// The generated program
// ---------------------------------------------------------------------------

/// Which stack an op opens or closes, for the balance repair below.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Bracket {
    Clip,
    Layer,
    Snapshot,
    Transform,
}

/// One builder call. The set covers all sixteen `Command` variants — `Path`
/// twice, once filled and once stroked, since the two reach it through
/// different builder methods — plus the transform stack, which records no
/// command of its own but is the other thing a balanced program has to unwind.
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
    GlyphRun {
        font_size: f32,
        transform: Affine,
        brush: Brush,
        glyphs: Vec<Glyph>,
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

impl Op {
    /// The stack this op opens, if any.
    fn opens(&self) -> Option<Bracket> {
        match self {
            Self::PushClip { .. } | Self::PushClipRounded { .. } => Some(Bracket::Clip),
            Self::PushLayer { .. } => Some(Bracket::Layer),
            Self::PushSnapshot { .. } => Some(Bracket::Snapshot),
            Self::PushTransform(_) => Some(Bracket::Transform),
            _ => None,
        }
    }

    /// The stack this op closes, if any.
    fn closes(&self) -> Option<Bracket> {
        match self {
            Self::PopClip => Some(Bracket::Clip),
            Self::PopLayer => Some(Bracket::Layer),
            Self::PopSnapshot => Some(Bracket::Snapshot),
            Self::PopTransform => Some(Bracket::Transform),
            _ => None,
        }
    }

    /// The op that closes `bracket`.
    fn closing(bracket: Bracket) -> Self {
        match bracket {
            Bracket::Clip => Self::PopClip,
            Bracket::Layer => Self::PopLayer,
            Bracket::Snapshot => Self::PopSnapshot,
            Bracket::Transform => Self::PopTransform,
        }
    }
}

/// A generated program plus whether it was repaired into a properly nested one.
#[derive(Clone, Debug)]
struct Program {
    ops: Vec<Op>,
    balanced: bool,
}

/// Rewrites `ops` into a properly nested program: a pop that does not match the
/// innermost open bracket is dropped (it would cross the nesting), and whatever
/// is still open at the end is closed in reverse order.
fn balance(ops: Vec<Op>) -> Vec<Op> {
    let mut open: Vec<Bracket> = Vec::new();
    let mut out: Vec<Op> = Vec::with_capacity(ops.len());

    for op in ops {
        match (op.opens(), op.closes()) {
            (Some(bracket), _) => {
                open.push(bracket);
                out.push(op);
            }
            (_, Some(bracket)) => {
                if open.last() == Some(&bracket) {
                    open.pop();
                    out.push(op);
                }
            }
            _ => out.push(op),
        }
    }

    while let Some(bracket) = open.pop() {
        out.push(Op::closing(bracket));
    }
    out
}

/// Replays `ops` through `builder`.
fn record(ops: &[Op], builder: &mut SceneBuilder<'_>) {
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
            Op::GlyphRun {
                font_size,
                transform,
                brush,
                glyphs,
            } => builder.draw_glyph_run(GlyphRun {
                font: shared_font(),
                font_size: *font_size,
                brush: brush.clone(),
                transform: *transform,
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
}

// ---------------------------------------------------------------------------
// Strategies
// ---------------------------------------------------------------------------

/// A coordinate: an ordinary value most of the time, a hostile one otherwise.
fn coord() -> impl Strategy<Value = f64> {
    prop_oneof![
        7 => -1_000.0_f64..1_000.0,
        3 => prop::sample::select(&HOSTILE_COORDS[..]),
    ]
}

fn coord_f32() -> impl Strategy<Value = f32> {
    prop_oneof![
        7 => -1_000.0_f32..1_000.0,
        3 => prop::sample::select(&HOSTILE_COORDS_F32[..]),
    ]
}

fn rect() -> impl Strategy<Value = Rect> {
    (coord(), coord(), coord(), coord()).prop_map(|(x0, y0, x1, y1)| Rect::new(x0, y0, x1, y1))
}

fn point() -> impl Strategy<Value = Point> {
    (coord(), coord()).prop_map(|(x, y)| Point::new(x, y))
}

fn radii() -> impl Strategy<Value = CornerRadii> {
    (coord(), coord(), coord(), coord())
        .prop_map(|(tl, tr, br, bl)| CornerRadii::new(tl, tr, br, bl))
}

fn affine() -> impl Strategy<Value = Affine> {
    (coord(), coord(), coord(), coord(), coord(), coord())
        .prop_map(|(a, b, c, d, e, f)| Affine::new([a, b, c, d, e, f]))
}

/// A transform whose composition with [`bracket_transform`] — and the division
/// back out by that bracket's inverse — is *exact* in binary floating point:
/// the linear part is a small power of two, the translation a small integer.
///
/// This is what the fingerprint property's bodies carry instead of [`affine`],
/// and the restriction is load-bearing twice over. A full-range coefficient
/// re-rounds through the bracket's inverse, so the relative transform comes back
/// a few ulps off and the invariance would only hold to within the
/// fingerprint's quantization step rather than bit-for-bit. Worse, a coefficient
/// near `f64::MAX` OVERFLOWS to infinity once the bracket's own scale is applied
/// — and the inverse then multiplies that infinity by the transform's own signed
/// zero, poisoning the relative transform with `NaN` and taking the mapped
/// geometry with it. Neither is an invariance the hash owes a caller: both are
/// degenerate transforms, the same class the engine's frame path refuses
/// outright.
fn exact_affine() -> impl Strategy<Value = Affine> {
    let coefficient =
        prop::sample::select(vec![-4.0_f64, -2.0, -1.0, -0.5, 0.0, 0.5, 1.0, 2.0, 4.0]);
    let translation = (-1024_i32..1024).prop_map(f64::from);
    (
        coefficient.clone(),
        coefficient.clone(),
        coefficient.clone(),
        coefficient,
        translation.clone(),
        translation,
    )
        .prop_map(|(a, b, c, d, e, f)| Affine::new([a, b, c, d, e, f]))
}

fn color() -> impl Strategy<Value = Color> {
    prop::sample::select(vec![RED, GREEN, BLUE, Color::TRANSPARENT])
}

fn brush() -> impl Strategy<Value = Brush> {
    prop_oneof![
        3 => color().prop_map(Brush::Solid),
        1 => (point(), point()).prop_map(|(start, end)| Brush::Gradient(
            Gradient::new_linear(start, end).with_stops([RED, BLUE])
        )),
    ]
}

fn dash() -> impl Strategy<Value = Option<DashPattern>> {
    prop_oneof![
        2 => Just(None),
        1 => (coord(), coord(), coord())
            .prop_map(|(on, off, phase)| Some(DashPattern::new(on, off).with_phase(phase))),
    ]
}

/// A path that always begins with a `MoveTo` — `BezPath::push` asserts it, so a
/// generator that emitted anything else would be testing kurbo's debug
/// assertion rather than the display list.
fn bez_path() -> impl Strategy<Value = BezPath> {
    let element = prop_oneof![
        point().prop_map(PathElement::MoveTo),
        point().prop_map(PathElement::LineTo),
        (point(), point()).prop_map(|(p0, p1)| PathElement::QuadTo(p0, p1)),
        (point(), point(), point()).prop_map(|(p0, p1, p2)| PathElement::CurveTo(p0, p1, p2)),
        Just(PathElement::ClosePath),
    ];
    (point(), prop::collection::vec(element, 0..6)).prop_map(|(start, elements)| {
        let mut path = BezPath::new();
        path.move_to(start);
        for element in elements {
            element.push_onto(&mut path);
        }
        path
    })
}

/// A path element in a form the generator can shrink, kept separate from
/// `kurbo::PathEl` only so the `MoveTo`-first rule above is enforced by
/// construction.
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
        (any::<u32>(), coord_f32(), coord_f32()).prop_map(|(id, x, y)| Glyph { id, x, y }),
        0..4,
    )
}

/// Every op the generator can emit, uniformly weighted so no variant is rare.
fn op() -> impl Strategy<Value = Op> {
    op_with_glyph_transform(affine().boxed())
}

/// [`op`] with the transform a generated glyph run carries of its own supplied
/// by the caller — the one field of the op set whose value is composed with the
/// enclosing bracket rather than merely recorded under it, and therefore the one
/// the fingerprint property has to constrain (see [`exact_affine`]).
fn op_with_glyph_transform(glyph_transform: BoxedStrategy<Affine>) -> impl Strategy<Value = Op> {
    prop_oneof![
        (rect(), brush()).prop_map(|(rect, brush)| Op::FillRect { rect, brush }),
        (rect(), radii(), brush()).prop_map(|(rect, radii, brush)| Op::RoundedRect {
            rect,
            radii,
            brush
        }),
        (point(), point(), coord(), brush()).prop_map(|(p0, p1, width, brush)| Op::Line {
            p0,
            p1,
            width,
            brush
        }),
        (coord_f32(), glyph_transform, brush(), glyphs()).prop_map(
            |(font_size, transform, brush, glyphs)| Op::GlyphRun {
                font_size,
                transform,
                brush,
                glyphs,
            }
        ),
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
        (bez_path(), brush()).prop_map(|(path, brush)| Op::FillPath { path, brush }),
        (bez_path(), coord(), dash(), brush()).prop_map(|(path, width, dash, brush)| {
            Op::StrokePath {
                path,
                width,
                dash,
                brush,
            }
        }),
        (rect(), coord_f32()).prop_map(|(dest, time)| Op::ShaderQuad { dest, time }),
        rect().prop_map(|rect| Op::PushClip { rect }),
        (rect(), radii()).prop_map(|(rect, radii)| Op::PushClipRounded { rect, radii }),
        Just(Op::PopClip),
        (rect(), coord_f32()).prop_map(|(rect, alpha)| Op::PushLayer { rect, alpha }),
        Just(Op::PopLayer),
        (any::<u64>(), rect(), coord_f32(), coord()).prop_map(|(key, rect, alpha, scale)| {
            Op::PushSnapshot {
                key,
                rect,
                alpha,
                scale,
            }
        }),
        Just(Op::PopSnapshot),
        affine().prop_map(Op::PushTransform),
        Just(Op::PopTransform),
    ]
}

/// A program, properly nested roughly seven times out of ten and deliberately
/// crossed or over-popped the rest of the time.
fn program() -> impl Strategy<Value = Program> {
    (
        prop::collection::vec(op(), 0..MAX_OPS),
        prop_oneof![7 => Just(true), 3 => Just(false)],
    )
        .prop_map(|(ops, balanced)| Program {
            ops: if balanced { balance(ops) } else { ops },
            balanced,
        })
}

/// A body for the fingerprint property: the same ops minus the transform stack,
/// which the bracket's own transform stands in for, and with a glyph run's own
/// transform restricted to one the bracket's inverse divides back out exactly
/// (see [`exact_affine`]).
fn body() -> impl Strategy<Value = Vec<Op>> {
    prop::collection::vec(
        op_with_glyph_transform(exact_affine().boxed())
            .prop_filter("the bracket owns the transform stack", |op| {
                !matches!(op, Op::PushTransform(_) | Op::PopTransform)
            }),
        0..MAX_OPS,
    )
}

/// The transform a fingerprint body's whole bracket sits under.
///
/// A translate composed with a power-of-two scale, both bounded well inside the
/// exponent range. Both are exactly invertible in binary floating point, so
/// `base.inverse() * transform` is *exactly* the identity on either side of the
/// comparison and the property is asserted bit-for-bit rather than within a
/// quantization step — which is what keeps a few hundred generated bodies from
/// producing an occasional false red on a coordinate that happens to land on a
/// rounding boundary. A non-power-of-two scale is covered by this module's
/// sibling unit test in `src/fingerprint.rs`.
fn bracket_transform() -> impl Strategy<Value = Affine> {
    (
        -1e6_f64..1e6,
        -1e6_f64..1e6,
        prop::sample::select(vec![0.25_f64, 0.5, 1.0, 2.0, 4.0]),
    )
        .prop_map(|(dx, dy, scale)| Affine::translate((dx, dy)) * Affine::scale(scale))
}

// ---------------------------------------------------------------------------
// Command-stream bookkeeping
// ---------------------------------------------------------------------------

/// How many of each bracket the command stream opens and closes, and whether
/// any close ran ahead of its open.
#[derive(Debug, Default)]
struct StreamBalance {
    clips_open: usize,
    clips_closed: usize,
    layers_open: usize,
    layers_closed: usize,
    snapshots_open: usize,
    snapshots_closed: usize,
    /// Set when a `PopSnapshot` appeared with no bracket open — the one pop the
    /// builder is documented to swallow rather than record.
    snapshot_underflow: bool,
}

fn walk(commands: &[Command]) -> StreamBalance {
    let mut balance = StreamBalance::default();
    let mut snapshot_depth = 0_isize;

    for command in commands {
        match command {
            Command::PushClip { .. } | Command::PushClipRounded { .. } => balance.clips_open += 1,
            Command::PopClip => balance.clips_closed += 1,
            Command::PushLayer { .. } => balance.layers_open += 1,
            Command::PopLayer => balance.layers_closed += 1,
            Command::PushSnapshot { .. } => {
                balance.snapshots_open += 1;
                snapshot_depth += 1;
            }
            Command::PopSnapshot => {
                balance.snapshots_closed += 1;
                snapshot_depth -= 1;
                if snapshot_depth < 0 {
                    balance.snapshot_underflow = true;
                }
            }
            _ => {}
        }
    }
    balance
}

/// Records `ops` into a fresh scene, returning it alongside the builder's
/// transform after `extra_pops` further `pop_transform` calls.
fn record_scene(ops: &[Op], extra_pops: usize) -> (Scene, Affine) {
    let mut scene = Scene::new();
    let transform = {
        let mut builder = SceneBuilder::new(&mut scene);
        record(ops, &mut builder);
        for _ in 0..extra_pops {
            builder.pop_transform();
        }
        builder.current_transform()
    };
    (scene, transform)
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: CASES,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    /// The builder accepts any sequence of calls, with any geometry, without
    /// panicking — including `NaN`, both infinities, both signed zeroes and the
    /// subnormal edges. It refuses nothing and validates nothing, so every
    /// recorded op has to reach the command stream.
    #[test]
    fn a_builder_records_any_program_without_panicking(program in program()) {
        let (scene, _) = record_scene(&program.ops, 0);

        // Every op but an ignored `pop_snapshot` and the two transform-stack ops
        // records exactly one command, so the stream can never be shorter than
        // the ops that must reach it nor longer than the ops that could.
        let stack_ops = program
            .ops
            .iter()
            .filter(|op| {
                matches!(op, Op::PushTransform(_) | Op::PopTransform | Op::PopSnapshot)
            })
            .count();
        prop_assert!(scene.commands().len() >= program.ops.len() - stack_ops);
        prop_assert!(scene.commands().len() <= program.ops.len());
    }

    /// A properly nested program records a properly nested command stream: the
    /// counts match, no close ran ahead of its open, the snapshot depth is back
    /// at zero and the transform stack has unwound to its base identity.
    #[test]
    fn a_balanced_program_balances(program in program()) {
        prop_assume!(program.balanced);

        let (scene, transform) = record_scene(&program.ops, 0);
        let balance = walk(scene.commands());

        prop_assert_eq!(balance.clips_open, balance.clips_closed);
        prop_assert_eq!(balance.layers_open, balance.layers_closed);
        prop_assert_eq!(balance.snapshots_open, balance.snapshots_closed);
        prop_assert!(!balance.snapshot_underflow);
        prop_assert_eq!(scene.snapshot_depth(), 0);
        prop_assert_eq!(transform, Affine::IDENTITY);
    }

    /// Unmatched pops follow the recorded policy, whatever the program.
    ///
    /// The three stacks deliberately differ: an unmatched `PopClip`/`PopLayer`
    /// IS recorded (a renderer ignores it when it unwinds), while an unmatched
    /// `pop_snapshot` records nothing at all and never drives the depth below
    /// zero. `pop_transform` never pops the base identity, so a program with
    /// more pops than pushes ends exactly where it started.
    #[test]
    fn unmatched_pops_follow_the_recorded_policy(program in program()) {
        let extra_pops = program.ops.len() + 1;
        let (scene, transform) = record_scene(&program.ops, extra_pops);
        let balance = walk(scene.commands());

        let pop_clips = program.ops.iter().filter(|op| matches!(op, Op::PopClip)).count();
        let pop_layers = program.ops.iter().filter(|op| matches!(op, Op::PopLayer)).count();
        prop_assert_eq!(balance.clips_closed, pop_clips);
        prop_assert_eq!(balance.layers_closed, pop_layers);

        prop_assert!(!balance.snapshot_underflow);
        prop_assert_eq!(
            scene.snapshot_depth(),
            balance.snapshots_open - balance.snapshots_closed
        );

        // More pops than the program could ever have pushed, and the builder is
        // still sitting on the identity it started from.
        prop_assert_eq!(transform, Affine::IDENTITY);
    }

    /// A body fingerprints identically whether its whole bracket sits at the
    /// identity or under a translate/scale, as long as each is hashed relative
    /// to its own bracket transform — the property a snapshot cache lives on.
    #[test]
    fn a_fingerprint_survives_its_brackets_own_translate_and_scale(
        body in body(),
        outer in bracket_transform(),
    ) {
        let fingerprint = |outer: Affine| {
            let mut scene = Scene::new();
            let base = {
                let mut builder = SceneBuilder::new(&mut scene);
                builder.push_transform(outer);
                let base = builder.current_transform();
                record(&body, &mut builder);
                base
            };
            (fingerprint_commands(scene.commands(), base), scene.commands().len())
        };

        let (under_outer, outer_len) = fingerprint(outer);
        let (at_identity, identity_len) = fingerprint(Affine::IDENTITY);

        prop_assert_eq!(outer_len, identity_len);
        prop_assert_eq!(under_outer, at_identity);
    }
}

/// The balance repair really does produce a properly nested program — the
/// generator's own tripwire, since every balance assertion above is only as
/// good as this.
#[test]
fn the_balance_repair_produces_a_properly_nested_program() {
    let crossed = vec![
        Op::PushClip {
            rect: Rect::new(0.0, 0.0, 1.0, 1.0),
        },
        Op::PushLayer {
            rect: Rect::new(0.0, 0.0, 1.0, 1.0),
            alpha: 0.5,
        },
        // Crosses the nesting: the innermost open bracket is the layer.
        Op::PopClip,
        Op::PopSnapshot,
    ];

    let repaired = balance(crossed);
    let kinds: Vec<Option<Bracket>> = repaired
        .iter()
        .map(|op| op.opens().or_else(|| op.closes()))
        .collect();
    assert_eq!(
        kinds,
        vec![
            Some(Bracket::Clip),
            Some(Bracket::Layer),
            Some(Bracket::Layer),
            Some(Bracket::Clip),
        ],
        "the crossed pops are dropped and the open brackets closed in reverse"
    );
}

/// Every `Command` variant is reachable from the op set — a variant nothing
/// generates is a hole in every property above, and the display list is
/// expected to grow.
#[test]
fn every_command_variant_is_reachable_from_the_op_set() {
    let rect = Rect::new(0.0, 0.0, 4.0, 4.0);
    let brush = Brush::Solid(RED);
    let mut path = BezPath::new();
    path.move_to((0.0, 0.0));
    path.line_to((1.0, 1.0));

    let ops = vec![
        Op::FillRect {
            rect,
            brush: brush.clone(),
        },
        Op::RoundedRect {
            rect,
            radii: CornerRadii::uniform(1.0),
            brush: brush.clone(),
        },
        Op::Line {
            p0: Point::ORIGIN,
            p1: Point::new(1.0, 1.0),
            width: 1.0,
            brush: brush.clone(),
        },
        Op::GlyphRun {
            font_size: 12.0,
            transform: Affine::IDENTITY,
            brush: brush.clone(),
            glyphs: vec![Glyph {
                id: 1,
                x: 0.0,
                y: 0.0,
            }],
        },
        Op::Image { dest: rect },
        Op::BlurredRoundedRect {
            rect,
            radii: CornerRadii::uniform(1.0),
            std_dev: 1.0,
            color: RED,
        },
        Op::ClearRect { rect },
        Op::FillPath {
            path: path.clone(),
            brush: brush.clone(),
        },
        Op::StrokePath {
            path,
            width: 1.0,
            dash: None,
            brush: brush.clone(),
        },
        Op::ShaderQuad {
            dest: rect,
            time: 0.0,
        },
        Op::PushClip { rect },
        Op::PopClip,
        Op::PushClipRounded {
            rect,
            radii: CornerRadii::uniform(1.0),
        },
        Op::PopClip,
        Op::PushLayer { rect, alpha: 0.5 },
        Op::PopLayer,
        Op::PushSnapshot {
            key: 1,
            rect,
            alpha: 0.5,
            scale: 1.0,
        },
        Op::PopSnapshot,
    ];

    let (scene, _) = record_scene(&ops, 0);
    let mut seen: Vec<&'static str> = scene
        .commands()
        .iter()
        .map(|command| match command {
            Command::FillRect { .. } => "FillRect",
            Command::RoundedRect { .. } => "RoundedRect",
            Command::Line { .. } => "Line",
            Command::GlyphRun(_) => "GlyphRun",
            Command::PushClip { .. } => "PushClip",
            Command::PushClipRounded { .. } => "PushClipRounded",
            Command::PopClip => "PopClip",
            Command::Image { .. } => "Image",
            Command::BlurredRoundedRect { .. } => "BlurredRoundedRect",
            Command::PushLayer { .. } => "PushLayer",
            Command::PopLayer => "PopLayer",
            Command::ClearRect { .. } => "ClearRect",
            Command::Path { .. } => "Path",
            Command::ShaderQuad { .. } => "ShaderQuad",
            Command::PushSnapshot { .. } => "PushSnapshot",
            Command::PopSnapshot => "PopSnapshot",
        })
        .collect();
    seen.sort_unstable();
    seen.dedup();

    let mut expected = vec![
        "BlurredRoundedRect",
        "ClearRect",
        "FillRect",
        "GlyphRun",
        "Image",
        "Line",
        "Path",
        "PopClip",
        "PopLayer",
        "PopSnapshot",
        "PushClip",
        "PushClipRounded",
        "PushLayer",
        "PushSnapshot",
        "RoundedRect",
        "ShaderQuad",
    ];
    expected.sort_unstable();
    assert_eq!(seen, expected, "one op set, all sixteen command variants");
}
