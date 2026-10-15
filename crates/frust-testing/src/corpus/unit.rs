//! The UNIT corpus: one named golden case per [`frust_scene::Command`]
//! variant, plus the variants whose interesting behaviour needs more than one
//! case.
//!
//! # What this corpus is for
//!
//! Every later phase of the render-testing plan regresses against these
//! cases, so each one is deliberately SMALL and SINGLE-PURPOSE: a 64x64
//! frame exercising one command's contract, with geometry chosen so a
//! reviewer can name what changed by looking at the diff. Nothing here draws
//! a widget, a page, or a theme — those belong to the widget/page corpus.
//!
//! # Recording rule
//!
//! Every case records through [`SceneBuilder`] and nothing else. Hand-built
//! [`frust_scene::Command`] values are not used even where they would be
//! shorter, because a corpus that can encode a display list the widget layer
//! could never produce is a corpus that regresses against fiction.
//!
//! # Coverage
//!
//! `tests::every_command_variant_is_covered` is the guard: it walks every
//! case's recorded scene and asserts all 18 `Command` variants appear. A new
//! variant fails that test until a case exists for it.
//!
//! # The two promoted cases
//!
//! `unit-clear-rect` and `unit-snapshot-bracket` are lifted from
//! `crates/frust-render/tests/gpu_smoke.rs` with their geometry and their
//! assertions intact — see each case's own doc comment for the defects they
//! were written to catch, and [`super::Probe`] for why the assertions travel
//! with the case instead of being replaced by a baseline.

use frust_scene::{CornerRadii, DashPattern, Scene, SceneBuilder, ShaderProgram};
use frust_text::{FontFamily, TextStyle};
use kurbo::{Affine, BezPath, Point, Rect, Shape};
use peniko::color::DynamicColor;
use peniko::color::palette::css::{BLACK, BLUE, GREEN, RED, WHITE, YELLOW};
use peniko::{Blob, Brush, Color, ColorStop, Gradient, ImageAlphaType, ImageData, ImageFormat};

use super::{CorpusCase, Expect, Probe};
use crate::case::{BackendSet, CaseSpec, Tolerance};
use crate::fonts::register_test_fonts;
use crate::frame::SAMPLE_TEXT;
use crate::oracle_cpu::ORACLE_ID;

/// Every unit case renders into a `SIZE`x`SIZE` frame.
///
/// 64 is the promoted `gpu_smoke` cases' own size, and their pixel
/// expectations are stated in terms of it (`SIZE / 2 + 1` is one pixel past a
/// 16-px tile boundary), so it is fixed here rather than chosen per case —
/// and it keeps every promoted PNG comfortably inside
/// `tests/corpus_budget.rs`'s 64 KB per-file cap.
const SIZE: u32 = 64;

/// [`SIZE`] as the float the geometry below is written in.
const EDGE: f64 = SIZE as f64;

/// The default case shape: a [`SIZE`]x[`SIZE`] transparent-based frame at the
/// crate's tight default tolerance.
fn case(name: &'static str) -> CaseSpec {
    CaseSpec::new(name).with_size(SIZE, SIZE)
}

/// An opaque black backdrop filling the frame — what makes a composite
/// arithmetic expectation exact (blending against a known opaque colour)
/// rather than an observation.
fn black_backdrop(builder: &mut SceneBuilder<'_>) {
    builder.fill_rect(Rect::new(0.0, 0.0, EDGE, EDGE), Brush::Solid(BLACK));
}

/// Every case in the unit corpus, in the order a reviewer reads them: the
/// paint primitives, then the grouping commands, then the two promoted
/// pixel-exact cases and the snapshot pair, then the paint-class frames
/// (see [`paint_class_cases`]).
#[must_use]
pub fn unit_cases() -> Vec<CorpusCase> {
    vec![
        fill_rect(),
        rounded_rect(),
        stroke_line(),
        glyph_run(),
        clip_rect(),
        clip_rounded(),
        clip_path_star(),
        clip_path_concave(),
        clip_balance(),
        image(),
        blur_rrect(),
        backdrop_blur(),
        layer_alpha(),
        layer_balance(),
        layer_sibling_fan(),
        layer_nested_pair(),
        clear_rect(),
        path_fill(),
        path_stroke(),
        path_dashed(),
        shader_quad(),
        snapshot_bracket(),
        snapshot_balance(),
        paint_class_mix(),
        paint_class_short_run(),
        paint_class_layer_over_gradient(),
    ]
}

/// `Command::FillRect` on both sides of the pixel grid: an integer-aligned
/// rect (whose every edge lands on a device pixel boundary, so it must
/// rasterize with no antialiased edge at all) and the same rect offset by
/// half a pixel (whose four edges are all 50% coverage). A regression that
/// shifts geometry by half a pixel is invisible in the first and obvious in
/// the second.
fn fill_rect() -> CorpusCase {
    fn record(scene: &mut Scene) {
        let mut builder = SceneBuilder::new(scene);
        builder.fill_rect(Rect::new(4.0, 4.0, 28.0, 28.0), Brush::Solid(RED));
        builder.fill_rect(Rect::new(36.5, 4.5, 60.5, 28.5), Brush::Solid(BLUE));
    }
    CorpusCase {
        spec: case("unit-fill-rect"),
        record,
        probes: &[
            Probe {
                x: 16,
                y: 16,
                expect: Expect::Exact([255, 0, 0, 255]),
                why: "the interior of an integer-aligned fill is the brush colour exactly",
            },
            Probe {
                x: 2,
                y: 2,
                expect: Expect::Exact([0, 0, 0, 0]),
                why: "nothing outside either rect is painted",
            },
        ],
        eroded_interior: false,
        about: "integer-aligned and half-pixel-offset solid fills side by side",
    }
}

/// `Command::RoundedRect` in both of its spellings: the uniform radius
/// `fill_rounded_rect` records, and the per-corner [`CornerRadii`]
/// `fill_rounded_rect_radii` records (including a zero corner, which must
/// stay square).
fn rounded_rect() -> CorpusCase {
    fn record(scene: &mut Scene) {
        let mut builder = SceneBuilder::new(scene);
        builder.fill_rounded_rect(Rect::new(4.0, 4.0, 28.0, 28.0), 8.0, Brush::Solid(RED));
        builder.fill_rounded_rect_radii(
            Rect::new(36.0, 36.0, 60.0, 60.0),
            CornerRadii::new(0.0, 12.0, 4.0, 8.0),
            Brush::Solid(BLUE),
        );
    }
    CorpusCase {
        spec: case("unit-rounded-rect"),
        record,
        probes: &[Probe {
            x: 37,
            y: 37,
            expect: Expect::Exact([0, 0, 255, 255]),
            why: "a zero top-left radius must stay a square corner, fully covered",
        }],
        eroded_interior: false,
        about: "uniform-radius and per-corner rounded fills, one corner square",
    }
}

/// `Command::Line` across the width range that actually distinguishes stroke
/// implementations: a 1-px stroke on a pixel boundary, a hairline well below
/// one pixel (which must still paint SOMETHING rather than vanishing), a 45°
/// diagonal (the fully antialiased case), and a zero-length segment.
///
/// The zero-length segment is here as a no-panic/no-artifact case: both
/// backends stroke with `kurbo::Stroke::new(width)`'s default butt caps, so a
/// degenerate segment has no extent to paint and correctly paints nothing.
/// The golden records that emptiness — a backend that started emitting a dot
/// or a NaN-driven smear there would fail this case.
fn stroke_line() -> CorpusCase {
    fn record(scene: &mut Scene) {
        let mut builder = SceneBuilder::new(scene);
        builder.stroke_line(
            Point::new(8.0, 8.5),
            Point::new(56.0, 8.5),
            1.0,
            Brush::Solid(RED),
        );
        builder.stroke_line(
            Point::new(8.0, 16.5),
            Point::new(56.0, 16.5),
            0.1,
            Brush::Solid(GREEN),
        );
        builder.stroke_line(
            Point::new(8.0, 24.0),
            Point::new(40.0, 56.0),
            2.0,
            Brush::Solid(BLUE),
        );
        builder.stroke_line(
            Point::new(52.0, 52.0),
            Point::new(52.0, 52.0),
            4.0,
            Brush::Solid(YELLOW),
        );
    }
    CorpusCase {
        spec: case("unit-stroke-line"),
        record,
        probes: &[
            Probe {
                x: 30,
                y: 8,
                expect: Expect::Exact([255, 0, 0, 255]),
                why: "a 1-px stroke centred on y = 8.5 covers row 8 exactly, with no ramp",
            },
            Probe {
                x: 30,
                y: 16,
                expect: Expect::AtLeast { channel: 3, min: 1 },
                why: "a sub-pixel hairline must still paint partial coverage, not vanish",
            },
            Probe {
                x: 52,
                y: 52,
                expect: Expect::Exact([0, 0, 0, 0]),
                why: "a zero-length butt-capped stroke has no extent and paints nothing",
            },
        ],
        eroded_interior: false,
        about: "1-px, hairline, 45-degree and zero-length strokes",
    }
}

/// `Command::GlyphRun`, shaped against the BUNDLED test fonts only.
///
/// The text is shaped through [`frust_text::TextContext`] with a family stack
/// naming exactly the four faces [`register_test_fonts`] registers, so no
/// host font can participate (`docs/TESTING.md`'s Deterministic Inputs — a
/// system-font golden is runner-local, not a portable reference). The string
/// is [`SAMPLE_TEXT`] rather than an arbitrary word: every one of its
/// codepoints is in the bundled Latin face's subset
/// (`testing/fonts/LICENSES.md`), so parley never needs to reach past the
/// registered stack for a glyph — a word outside that subset (the previous
/// `"Frust"`) sent fontique to a HOST font instead, making the promoted
/// baseline runner-local (`crate::frame::foreign_font_runs` is the check that
/// now catches this class of regression). This case pins the GlyphRun
/// COMMAND's lowering, not the shaper's script coverage, which `fonts.rs`'s
/// own test already covers.
fn glyph_run() -> CorpusCase {
    fn record(scene: &mut Scene) {
        let mut cx = frust_text::TextContext::new();
        let registered = register_test_fonts(&mut cx);
        let style = TextStyle {
            family: FontFamily::stack(registered.iter().map(|f| f.name.clone())),
            ..TextStyle::new(24.0, BLACK)
        };
        let layout = cx.layout(SAMPLE_TEXT, &style, None);
        let mut builder = SceneBuilder::new(scene);
        // An opaque white backdrop: black-on-transparent text would store its
        // whole antialiased ramp in the alpha channel alone, where a colour
        // regression could hide.
        builder.fill_rect(Rect::new(0.0, 0.0, EDGE, EDGE), Brush::Solid(WHITE));
        for run in layout.to_scene_runs(Point::new(4.0, 16.0)) {
            builder.draw_glyph_run(run);
        }
    }
    CorpusCase {
        spec: case("unit-glyph-run"),
        record,
        probes: &[Probe {
            x: 62,
            y: 2,
            expect: Expect::Exact([255, 255, 255, 255]),
            why: "the backdrop outside the text block is untouched white",
        }],
        eroded_interior: false,
        about: "a short Latin run shaped against the bundled test fonts",
    }
}

/// `Command::PushClip`/`Command::PopClip` with a fill deliberately
/// STRADDLING the clip boundary on two sides, so the case fails if the clip
/// is dropped (the fill would reach the frame edge) or applied in the wrong
/// space (the surviving corner would move).
fn clip_rect() -> CorpusCase {
    fn record(scene: &mut Scene) {
        let mut builder = SceneBuilder::new(scene);
        builder.push_clip(Rect::new(16.0, 16.0, 48.0, 48.0));
        builder.fill_rect(Rect::new(8.0, 8.0, 40.0, 40.0), Brush::Solid(RED));
        builder.pop_clip();
    }
    CorpusCase {
        spec: case("unit-clip-rect"),
        record,
        probes: &[
            Probe {
                x: 20,
                y: 20,
                expect: Expect::Exact([255, 0, 0, 255]),
                why: "inside both the clip and the fill",
            },
            Probe {
                x: 12,
                y: 20,
                expect: Expect::Exact([0, 0, 0, 0]),
                why: "inside the fill but outside the clip — the clip must win",
            },
            Probe {
                x: 44,
                y: 44,
                expect: Expect::Exact([0, 0, 0, 0]),
                why: "inside the clip but outside the fill",
            },
        ],
        eroded_interior: false,
        about: "a fill straddling two edges of a rectangular clip",
    }
}

/// `Command::PushClipRounded` with PER-CORNER radii, popped by the same
/// [`SceneBuilder::pop_clip`] a rectangular clip uses (both share one stack).
/// A full-frame fill inside it makes the clip shape itself the whole image.
fn clip_rounded() -> CorpusCase {
    fn record(scene: &mut Scene) {
        let mut builder = SceneBuilder::new(scene);
        builder.push_clip_rounded_radii(
            Rect::new(8.0, 8.0, 56.0, 56.0),
            CornerRadii::new(20.0, 0.0, 20.0, 4.0),
        );
        builder.fill_rect(Rect::new(0.0, 0.0, EDGE, EDGE), Brush::Solid(BLUE));
        builder.pop_clip();
    }
    CorpusCase {
        spec: case("unit-clip-rounded"),
        record,
        probes: &[
            Probe {
                x: 54,
                y: 10,
                expect: Expect::Exact([0, 0, 255, 255]),
                why: "the zero-radius top-right corner of the clip stays square",
            },
            Probe {
                x: 10,
                y: 10,
                expect: Expect::Exact([0, 0, 0, 0]),
                why: "the 20-px top-left corner of the clip is rounded away",
            },
        ],
        eroded_interior: false,
        about: "per-corner radii on a clip, one corner square",
    }
}

/// A five-pointed star's outline: outer radius `outer`, inner radius `inner`,
/// centred on `(cx, cy)`, its first point straight up.
fn star_path(cx: f64, cy: f64, outer: f64, inner: f64) -> BezPath {
    let mut path = BezPath::new();
    for i in 0..10 {
        let radius = if i % 2 == 0 { outer } else { inner };
        let angle = -std::f64::consts::FRAC_PI_2 + f64::from(i) * std::f64::consts::PI / 5.0;
        let point = (cx + radius * angle.cos(), cy + radius * angle.sin());
        if i == 0 {
            path.move_to(point);
        } else {
            path.line_to(point);
        }
    }
    path.close_path();
    path
}

/// `Command::PushClipPath` with a five-pointed STAR over a four-colour
/// quadrant fill, popped by the same [`SceneBuilder::pop_clip`] every clip
/// uses. The star's diagonal edges are antialiased, so the case pins the
/// coverage mask's edge as well as its inside and outside; the four colours
/// make an arm drawn in the wrong space read as the wrong colour.
fn clip_path_star() -> CorpusCase {
    fn record(scene: &mut Scene) {
        let mut builder = SceneBuilder::new(scene);
        builder.push_clip_path(star_path(32.0, 32.0, 28.0, 11.0));
        builder.fill_rect(Rect::new(0.0, 0.0, 32.0, 32.0), Brush::Solid(RED));
        builder.fill_rect(Rect::new(32.0, 0.0, EDGE, 32.0), Brush::Solid(GREEN));
        builder.fill_rect(Rect::new(0.0, 32.0, 32.0, EDGE), Brush::Solid(BLUE));
        builder.fill_rect(Rect::new(32.0, 32.0, EDGE, EDGE), Brush::Solid(YELLOW));
        builder.pop_clip();
    }
    CorpusCase {
        spec: case("unit-clip-path-star"),
        record,
        probes: &[
            Probe {
                x: 30,
                y: 12,
                expect: Expect::Exact([255, 0, 0, 255]),
                why: "inside the star's top point, over the red quadrant",
            },
            Probe {
                x: 22,
                y: 44,
                expect: Expect::Exact([0, 0, 255, 255]),
                why: "inside the lower-left point, over the blue quadrant",
            },
            Probe {
                x: 36,
                y: 36,
                expect: Expect::Exact([255, 255, 0, 255]),
                why: "inside the star's body, over the yellow quadrant",
            },
            Probe {
                x: 50,
                y: 10,
                expect: Expect::Exact([0, 0, 0, 0]),
                why: "between the top and upper-right points — the fill is clipped away",
            },
            Probe {
                x: 2,
                y: 62,
                expect: Expect::Exact([0, 0, 0, 0]),
                why: "a frame corner, outside every point of the star",
            },
        ],
        eroded_interior: false,
        about: "a five-pointed star path clip over a four-colour quadrant fill",
    }
}

/// `Command::PushClipPath` with a CONCAVE outline — an L, whose reflex corner
/// is exactly the shape a convex-hull or bounding-box shortcut would get
/// wrong. A full-frame fill plus a stripe crossing the notch: the notch must
/// stay empty even where the stripe passes through it.
fn clip_path_concave() -> CorpusCase {
    fn record(scene: &mut Scene) {
        let mut builder = SceneBuilder::new(scene);
        let mut outline = BezPath::new();
        outline.move_to((8.0, 8.0));
        outline.line_to((28.0, 8.0));
        outline.line_to((28.0, 36.0));
        outline.line_to((56.0, 36.0));
        outline.line_to((56.0, 56.0));
        outline.line_to((8.0, 56.0));
        outline.close_path();
        builder.push_clip_path(outline);
        builder.fill_rect(Rect::new(0.0, 0.0, EDGE, EDGE), Brush::Solid(GREEN));
        builder.fill_rect(Rect::new(0.0, 16.0, EDGE, 24.0), Brush::Solid(RED));
        builder.pop_clip();
    }
    CorpusCase {
        spec: case("unit-clip-path-concave"),
        record,
        probes: &[
            Probe {
                x: 16,
                y: 30,
                expect: Expect::Exact([0, 128, 0, 255]),
                why: "inside the L's upright",
            },
            Probe {
                x: 50,
                y: 46,
                expect: Expect::Exact([0, 128, 0, 255]),
                why: "inside the L's foot",
            },
            Probe {
                x: 16,
                y: 20,
                expect: Expect::Exact([255, 0, 0, 255]),
                why: "the stripe survives where it crosses the upright",
            },
            Probe {
                x: 44,
                y: 20,
                expect: Expect::Exact([0, 0, 0, 0]),
                why: "the stripe is clipped away inside the notch — the reflex corner holds",
            },
            Probe {
                x: 44,
                y: 30,
                expect: Expect::Exact([0, 0, 0, 0]),
                why: "the notch stays empty below the stripe too",
            },
        ],
        eroded_interior: false,
        about: "a concave L-shaped path clip, a stripe crossing its notch",
    }
}

/// Clip-stack BALANCE: two nested clips, both popped, then one EXTRA
/// `pop_clip` with nothing left to pop, then a draw at root.
///
/// `Command::PopClip`'s documented policy is that an unbalanced pop is
/// IGNORED. The final root draw is the assertion: it must land unclipped, in
/// full, which it only can if the extra pop neither underflowed the stack nor
/// left it in a state that keeps clipping. The case exists because that path
/// is reachable from ordinary widget code (an early return between a push and
/// its pop) and because a backend that underflows here does not misrender, it
/// aborts.
fn clip_balance() -> CorpusCase {
    fn record(scene: &mut Scene) {
        let mut builder = SceneBuilder::new(scene);
        builder.push_clip(Rect::new(8.0, 8.0, 56.0, 56.0));
        builder.fill_rect(Rect::new(0.0, 0.0, EDGE, EDGE), Brush::Solid(RED));
        builder.push_clip(Rect::new(8.0, 8.0, 32.0, 32.0));
        builder.fill_rect(Rect::new(0.0, 0.0, EDGE, EDGE), Brush::Solid(GREEN));
        builder.pop_clip();
        builder.pop_clip();
        // The extra one: no clip is open, so this must be ignored.
        builder.pop_clip();
        builder.fill_rect(Rect::new(0.0, 56.0, 8.0, EDGE), Brush::Solid(BLUE));
    }
    CorpusCase {
        spec: case("unit-clip-balance"),
        record,
        probes: &[
            Probe {
                x: 4,
                y: 60,
                expect: Expect::Exact([0, 0, 255, 255]),
                why: "a draw after an unbalanced pop_clip lands at root, outside every clip",
            },
            Probe {
                x: 16,
                y: 16,
                expect: Expect::Exact([0, 128, 0, 255]),
                why: "the inner clip bounded the green fill while it was open",
            },
        ],
        eroded_interior: false,
        about: "balanced nested clips plus one extra pop, documented as ignored",
    }
}

/// The RGBA8 checkerboard [`image`] draws: `CELLS`x`CELLS` cells of `CELL`
/// pixels each, alternating opaque red and opaque blue.
///
/// Deliberately high-frequency: a scaled-down draw that samples it wrongly
/// produces a visibly different average, and a scaled-up draw makes the
/// sampler's filtering choice (nearest vs. bilinear) plain in the golden.
const CELL: u32 = 2;
/// Checkerboard cells per side (so the source image is `CELL * CELLS` px).
const CELLS: u32 = 8;

/// The checkerboard's decoded bytes, straight-alpha RGBA8.
fn checkerboard() -> ImageData {
    let side = CELL * CELLS;
    let mut data = Vec::with_capacity((side * side * 4) as usize);
    for y in 0..side {
        for x in 0..side {
            let dark = ((x / CELL) + (y / CELL)).is_multiple_of(2);
            let color = if dark {
                [255, 0, 0, 255]
            } else {
                [0, 0, 255, 255]
            };
            data.extend_from_slice(&color);
        }
    }
    ImageData {
        data: Blob::from(data),
        format: ImageFormat::Rgba8,
        alpha_type: ImageAlphaType::Alpha,
        width: side,
        height: side,
    }
}

/// `Command::Image` scaled BOTH ways out of one source: a 16x16 checkerboard
/// magnified into a 32x32 dest and minified into an 8x8 dest.
///
/// `frust-engine`'s image lowering (`compile/paint.rs`'s
/// `encode_image_command`) composes the natural-pixel-to-dest transform once,
/// used whichever direction the scale runs; this is the case that pins both
/// directions together. Magnifying and minifying the same bytes in one frame
/// pins the paint-transform composition in both directions, which is where an
/// off-by-one in the natural-size-to-dest mapping shows up.
fn image() -> CorpusCase {
    fn record(scene: &mut Scene) {
        let data = checkerboard();
        let mut builder = SceneBuilder::new(scene);
        builder.draw_image(&data, Rect::new(4.0, 4.0, 36.0, 36.0));
        builder.draw_image(&data, Rect::new(44.0, 44.0, 52.0, 52.0));
    }
    CorpusCase {
        spec: case("unit-image"),
        record,
        probes: &[
            Probe {
                x: 6,
                y: 6,
                expect: Expect::AtLeast {
                    channel: 3,
                    min: 255,
                },
                why: "a magnified opaque image stays opaque",
            },
            Probe {
                x: 40,
                y: 40,
                expect: Expect::Exact([0, 0, 0, 0]),
                why: "neither dest rect reaches the gap between them",
            },
        ],
        eroded_interior: false,
        about: "one RGBA8 checkerboard magnified and minified into two dests",
    }
}

/// Side of the source [`image_magnified`] draws, in pixels.
const MAGNIFIED_SOURCE: u32 = 8;

/// How much [`image_magnified`] magnifies its source by.
const MAGNIFICATION: f64 = 4.0;

/// The 8x8 source [`image_magnified`] draws: a one-pixel red checkerboard
/// over green and blue ramps, opaque, whose last column and last row repeat
/// the ones before them.
///
/// The red channel flips between 0 and 255 on every interior texel, which is
/// the largest step a bilinear filter can interpolate across — so it is where
/// a filter's weight precision shows most, and a 4x magnification puts three
/// interpolated pixels between every pair of texel centres.
///
/// The repeated edge is deliberate. The engine's padded bilinear clamps the
/// sample position to the image before taking its half-texel offset, so the
/// last texel of a magnified image blends half-way into the one before it,
/// where `vello_cpu` holds the edge texel; with the two equal, the case
/// measures the filter rather than that edge rule — the same reason
/// [`checkerboard`]'s cells are two pixels wide.
fn magnified_source() -> ImageData {
    let side = MAGNIFIED_SOURCE;
    let mut data = Vec::with_capacity((side * side * 4) as usize);
    for y in 0..side {
        for x in 0..side {
            let (x, y) = (x.min(side - 2), y.min(side - 2));
            let red = if (x + y).is_multiple_of(2) { 255 } else { 0 };
            data.extend_from_slice(&[red, (x * 36) as u8, (y * 36) as u8, 255]);
        }
    }
    ImageData {
        data: Blob::from(data),
        format: ImageFormat::Rgba8,
        alpha_type: ImageAlphaType::Alpha,
        width: side,
        height: side,
    }
}

/// `Command::Image` magnified 4x — the scale the S5 image stream draws at —
/// twice: on the pixel grid, and at a fractional offset, so every bilinear
/// weight a 4x filter produces is sampled.
///
/// Not part of [`unit_cases`]: it carries no reviewed baseline. It exists to
/// measure the engine's bilinear image filtering against the CPU oracle in
/// both of the engine's strip program families — the manual four-tap path and
/// the hardware sampler (`FRUST_ENGINE_IMAGE_SAMPLER`) — and
/// `tests::the_image_sampler_cases_hold_cpu_oracle_parity` holds it there at
/// the corpus default tolerance every image case is held to.
fn image_magnified() -> CorpusCase {
    fn record(scene: &mut Scene) {
        let data = magnified_source();
        let extent = f64::from(MAGNIFIED_SOURCE) * MAGNIFICATION;
        let mut builder = SceneBuilder::new(scene);
        black_backdrop(&mut builder);
        builder.draw_image(&data, Rect::new(0.0, 0.0, extent, extent));
        builder.draw_image(
            &data,
            Rect::new(
                extent - 0.375,
                extent - 0.625,
                2.0 * extent - 0.375,
                2.0 * extent - 0.625,
            ),
        );
    }
    CorpusCase {
        spec: case("unit-image-magnified"),
        record,
        probes: &[Probe {
            x: 2,
            y: 2,
            expect: Expect::AtLeast {
                channel: 3,
                min: 255,
            },
            why: "a magnified opaque image stays opaque",
        }],
        eroded_interior: false,
        about: "one 8x8 RGBA8 checkerboard-over-ramps magnified 4x, on and off the pixel grid",
    }
}

/// The cases that measure the engine's image filtering rather than pin a
/// command's contract — today [`image_magnified`] alone. Not registered in
/// [`unit_cases`] (no reviewed baseline); see that case's doc.
#[must_use]
pub fn image_sampler_cases() -> Vec<CorpusCase> {
    vec![image_magnified()]
}

/// Three glyph runs in one frame, each taking a different route through the
/// engine's text lowering: a Latin run the glyph atlas holds (drawn as glyph
/// quads once its glyphs are resident), the same word rotated (no whole-pixel
/// placement, so its outlines are rasterized every frame), and a COLRv1 emoji
/// (a colour face never reaches the atlas; its layers are drawn as shapes).
///
/// Not part of [`unit_cases`]: it carries no reviewed baseline. It exists to
/// hold the engine's glyph quads to the frame the same scene drew before its
/// glyphs were resident — rendered three times through one engine, the
/// second and third frames (quads) must read back byte-identical to the first
/// (every glyph drawn the general way, because it was being rasterized) — and
/// to the CPU oracle: `tests::the_glyph_quad_cases_hold_their_first_frame_and_cpu_oracle_parity`,
/// run once per program family and once under the Uber program.
///
/// Its channel tolerance is the hint-policy delta `unit-glyph-run`'s engine
/// gate row documents, measured on this case: a desktop-tier adapter hints
/// its outlines while the CPU oracle never does, so a glyph's antialiased
/// edge lands a fraction of a pixel apart on the two arms — max |delta|
/// [129,129,129,0] on the pinned T400, identical with glyph quads off
/// (`FRUST_ENGINE_NO_GLYPH_QUADS=1`), so none of it is the quads'. The byte
/// identity across the engine's own frames is the bar the quads are held to.
fn glyph_mixed_run() -> CorpusCase {
    fn record(scene: &mut Scene) {
        let mut cx = frust_text::TextContext::new();
        let registered = register_test_fonts(&mut cx);
        let family = FontFamily::stack(registered.iter().map(|f| f.name.clone()));
        let style = |size: f32| TextStyle {
            family: family.clone(),
            ..TextStyle::new(size, BLACK)
        };
        let mut builder = SceneBuilder::new(scene);
        builder.fill_rect(Rect::new(0.0, 0.0, EDGE, EDGE), Brush::Solid(WHITE));
        // Cached: one size, so the atlas takes the run on its first frame.
        let latin = cx.layout(SAMPLE_TEXT, &style(14.0), None);
        for run in latin.to_scene_runs(Point::new(3.3, 15.0)) {
            builder.draw_glyph_run(run);
        }
        // Outlines: the same word turned a quarter radian about its origin.
        for mut run in latin.to_scene_runs(Point::new(6.0, 30.0)) {
            run.transform = Affine::rotate_about(0.25, Point::new(6.0, 30.0)) * run.transform;
            builder.draw_glyph_run(run);
        }
        // Colour: U+1F600, the one codepoint the bundled emoji subset carries;
        // its glyph sits almost entirely below the baseline.
        let emoji = cx.layout("\u{1F600}", &style(20.0), None);
        for run in emoji.to_scene_runs(Point::new(36.0, 36.0)) {
            builder.draw_glyph_run(run);
        }
    }
    CorpusCase {
        spec: case("unit-glyph-mixed-run").with_tolerance(Tolerance::new().with_channel(129)),
        record,
        probes: &[Probe {
            x: 62,
            y: 2,
            expect: Expect::Exact([255, 255, 255, 255]),
            why: "the backdrop outside every run is untouched white",
        }],
        eroded_interior: false,
        about: "a cached Latin run, a rotated (outline) run and a COLRv1 emoji in one frame",
    }
}

/// The cases that hold the engine's glyph quads to the frame drawn before its
/// glyphs were resident — today [`glyph_mixed_run`] alone. Not registered in
/// [`unit_cases`] (no reviewed baseline); see that case's doc.
#[must_use]
pub fn glyph_quad_cases() -> Vec<CorpusCase> {
    vec![glyph_mixed_run()]
}

/// `Command::BlurredRoundedRect` across four `std_dev` steps, the last of
/// them with PER-CORNER radii.
///
/// Neither render tier has a per-corner blurred primitive, so a non-uniform
/// shadow lowers through [`CornerRadii::largest`] at encode time on EVERY
/// backend — the fourth cell's golden therefore encodes that approximation,
/// not the per-corner shape the recorder asked for. That is the point of
/// having it here: the downgrade is a contract, and a backend that silently
/// started honouring the small corners would fail this case rather than
/// quietly diverging from the others.
fn blur_rrect() -> CorpusCase {
    fn record(scene: &mut Scene) {
        let mut builder = SceneBuilder::new(scene);
        black_backdrop(&mut builder);
        let cells = [
            (0.0, 0.0, 0.5_f64),
            (32.0, 0.0, 1.5),
            (0.0, 32.0, 3.0),
            (32.0, 32.0, 6.0),
        ];
        for (i, (ox, oy, std_dev)) in cells.into_iter().enumerate() {
            let rect = Rect::new(ox + 8.0, oy + 8.0, ox + 24.0, oy + 24.0);
            if i == 3 {
                builder.draw_blurred_rounded_rect_radii(
                    rect,
                    CornerRadii::new(0.0, 8.0, 2.0, 4.0),
                    std_dev,
                    WHITE,
                );
            } else {
                builder.draw_blurred_rounded_rect(rect, 4.0, std_dev, WHITE);
            }
        }
    }
    CorpusCase {
        spec: case("unit-blur-rrect"),
        record,
        // The blur is a filtered primitive whose edge ramp is exactly where
        // two rasterizers legitimately differ, so the case widens its
        // channel tolerance ONCE, here, where it is reviewable — never in a
        // retry.
        probes: &[],
        eroded_interior: true,
        about: "four std_dev steps, the last with per-corner radii collapsed to the largest",
    }
}

/// `Command::BackdropBlur` under a rounded clip, over a backdrop of 4-px
/// vertical red/blue stripes with a yellow square, then a translucent white
/// wash painted over the blurred panel — the frosted-glass shape.
///
/// Three things are pinned at once: the backdrop under the panel is blurred
/// (the stripes mix, which an unblurred frame cannot show), everything outside
/// the panel — including the pixels inside its rectangle but outside the
/// rounded corner — is untouched, and what is recorded after the backdrop
/// lands on top of it. The σ (1.5) is one the blur planner convolves directly,
/// with no decimation, so the engine's filter passes and the CPU oracle's
/// `vello_cpu` filter run the same discrete kernel (see `oracle_cpu`'s gap
/// list).
fn backdrop_blur() -> CorpusCase {
    fn record(scene: &mut Scene) {
        let mut builder = SceneBuilder::new(scene);
        black_backdrop(&mut builder);
        for stripe in 0..16 {
            let x = f64::from(stripe) * 4.0;
            let colour = if stripe % 2 == 0 { RED } else { BLUE };
            builder.fill_rect(Rect::new(x, 0.0, x + 4.0, EDGE), Brush::Solid(colour));
        }
        builder.fill_rect(Rect::new(24.0, 12.0, 40.0, 24.0), Brush::Solid(YELLOW));
        let panel = Rect::new(8.0, 8.0, 56.0, 56.0);
        builder.push_clip_rounded(panel, 12.0);
        builder.backdrop_blur(panel, 1.5);
        builder.fill_rect(panel, Brush::Solid(WHITE.with_alpha(0.2)));
        builder.pop_clip();
    }
    CorpusCase {
        spec: case("unit-backdrop-blur"),
        record,
        probes: &[
            Probe {
                x: 2,
                y: 32,
                expect: Expect::Exact([255, 0, 0, 255]),
                why: "a stripe outside the panel is the backdrop untouched",
            },
            Probe {
                x: 9,
                y: 9,
                expect: Expect::Exact([255, 0, 0, 255]),
                why: "inside the panel's rectangle but outside its rounded corner, the clip masks                       the blur away",
            },
            Probe {
                x: 32,
                y: 40,
                expect: Expect::AtLeast {
                    channel: 2,
                    min: 90,
                },
                why: "the left column of a red stripe takes blue from the stripe beside it once                       blurred (unblurred, the wash alone leaves blue at 51)",
            },
            Probe {
                x: 32,
                y: 40,
                expect: Expect::AtLeast {
                    channel: 0,
                    min: 140,
                },
                why: "and keeps most of its own red",
            },
        ],
        // The rounded clip's antialiased rim is where the panel's blurred
        // pixels meet the unblurred stripes beside them, so a one-level
        // coverage rounding difference between two rasterizers' masks reads
        // as a several-level colour difference there — edge jitter one pixel
        // wide, which the comparator erodes, never an interior change.
        eroded_interior: true,
        about: "a frosted panel: backdrop blur under a rounded clip, a white wash over it",
    }
}

/// `Command::PushLayer`/`Command::PopLayer` at alpha 0.5, nested three deep
/// over an opaque black backdrop.
///
/// Each level paints its own band, so one frame shows 0.5, 0.25 and 0.125
/// effective opacity at once, and the probes state those as arithmetic over
/// black rather than as observations. A backend that flattened nested layers
/// (applying the innermost alpha only) or double-premultiplied one would miss
/// two of the three numbers.
fn layer_alpha() -> CorpusCase {
    fn record(scene: &mut Scene) {
        let mut builder = SceneBuilder::new(scene);
        black_backdrop(&mut builder);
        builder.push_layer(Rect::new(4.0, 4.0, 60.0, 60.0), 0.5);
        builder.fill_rect(Rect::new(4.0, 4.0, 60.0, 20.0), Brush::Solid(WHITE));
        builder.push_layer(Rect::new(4.0, 4.0, 60.0, 60.0), 0.5);
        builder.fill_rect(Rect::new(4.0, 24.0, 60.0, 40.0), Brush::Solid(WHITE));
        builder.push_layer(Rect::new(4.0, 4.0, 60.0, 60.0), 0.5);
        builder.fill_rect(Rect::new(4.0, 44.0, 60.0, 58.0), Brush::Solid(WHITE));
        builder.pop_layer();
        builder.pop_layer();
        builder.pop_layer();
    }
    CorpusCase {
        spec: case("unit-layer-alpha").with_base_color(BLACK),
        record,
        probes: &[
            Probe {
                x: 32,
                y: 12,
                expect: Expect::Channel {
                    channel: 0,
                    value: 128,
                    tolerance: 2,
                },
                why: "white under one 0.5 layer composites to 255 * 0.5 over black",
            },
            Probe {
                x: 32,
                y: 32,
                expect: Expect::Channel {
                    channel: 0,
                    value: 64,
                    tolerance: 2,
                },
                why: "two nested 0.5 layers compose to 0.25",
            },
            Probe {
                x: 32,
                y: 50,
                expect: Expect::Channel {
                    channel: 0,
                    value: 32,
                    tolerance: 2,
                },
                why: "three nested 0.5 layers compose to 0.125",
            },
        ],
        eroded_interior: false,
        about: "0.5 opacity layers nested three deep over an opaque backdrop",
    }
}

/// Layer-stack BALANCE, the `Command::PopLayer` counterpart of
/// [`clip_balance`]: balanced nested layers, then one extra pop that must be
/// ignored, then a root draw that must land at full opacity.
fn layer_balance() -> CorpusCase {
    fn record(scene: &mut Scene) {
        let mut builder = SceneBuilder::new(scene);
        black_backdrop(&mut builder);
        builder.push_layer(Rect::new(0.0, 0.0, EDGE, EDGE), 0.5);
        builder.fill_rect(Rect::new(4.0, 4.0, 60.0, 28.0), Brush::Solid(WHITE));
        builder.pop_layer();
        // The extra one: no layer is open, so this must be ignored.
        builder.pop_layer();
        builder.fill_rect(Rect::new(4.0, 36.0, 60.0, 60.0), Brush::Solid(WHITE));
    }
    CorpusCase {
        spec: case("unit-layer-balance").with_base_color(BLACK),
        record,
        probes: &[
            Probe {
                x: 32,
                y: 16,
                expect: Expect::Channel {
                    channel: 0,
                    value: 128,
                    tolerance: 2,
                },
                why: "the balanced layer still applies its own 0.5",
            },
            Probe {
                x: 32,
                y: 48,
                expect: Expect::Exact([255, 255, 255, 255]),
                why: "a draw after an unbalanced pop_layer is at root, fully opaque",
            },
        ],
        eroded_interior: false,
        about: "a balanced opacity layer plus one extra pop, documented as ignored",
    }
}

/// Sibling ISOLATED layers: four `Command::PushLayer`/`Command::PopLayer`
/// pairs opened and fully closed one after another — the SAME nesting depth,
/// never nested inside each other — each over its own non-overlapping band.
///
/// `frust_engine::schedule::mod`'s own module docs name this exact shape:
/// "isolated layers *beside* each other under one parent, a fan of any
/// width... a staggered list entrance fading five rows at once is served on
/// the same two pages". A fan wider than two forces the scheduler's
/// two-page ping-pong to [cut](frust_engine::schedule) the parent's round
/// early so the third and fourth siblings can each take a freed page in
/// turn — a path the scheduler's own unit tests already exercise, but which
/// no `Scene` a `SceneBuilder` could actually produce had pinned pixels for
/// before this case, despite being reachable from ordinary widget code (any
/// screen that fades in more than two rows/cards at once).
fn layer_sibling_fan() -> CorpusCase {
    fn record(scene: &mut Scene) {
        let mut builder = SceneBuilder::new(scene);
        black_backdrop(&mut builder);
        // Four half-opacity siblings, each opened and closed before the next
        // starts — one non-overlapping band each, so every probe reads a
        // single layer's own composite with no risk of a second sibling's
        // paint bleeding into it.
        let bands: [(f64, f64); 4] = [(4.0, 16.0), (18.0, 30.0), (32.0, 44.0), (46.0, 58.0)];
        for (top, bottom) in bands {
            let rect = Rect::new(4.0, top, 60.0, bottom);
            builder.push_layer(rect, 0.5);
            builder.fill_rect(rect, Brush::Solid(WHITE));
            builder.pop_layer();
        }
    }
    CorpusCase {
        spec: case("unit-layer-sibling-fan").with_base_color(BLACK),
        record,
        probes: &[
            Probe {
                x: 32,
                y: 10,
                expect: Expect::Channel {
                    channel: 0,
                    value: 128,
                    tolerance: 2,
                },
                why: "the first sibling's half-opacity white over black composites to 255 * 0.5",
            },
            Probe {
                x: 32,
                y: 24,
                expect: Expect::Channel {
                    channel: 0,
                    value: 128,
                    tolerance: 2,
                },
                why: "the second sibling composites identically to the first — same depth, \
                      not compounding",
            },
            Probe {
                x: 32,
                y: 38,
                expect: Expect::Channel {
                    channel: 0,
                    value: 128,
                    tolerance: 2,
                },
                why: "the third sibling — the one a two-page ping-pong must cut the parent's \
                      round to serve",
            },
            Probe {
                x: 32,
                y: 52,
                expect: Expect::Channel {
                    channel: 0,
                    value: 128,
                    tolerance: 2,
                },
                why: "the fourth sibling composites identically to every other one",
            },
            Probe {
                x: 2,
                y: 2,
                expect: Expect::Exact([0, 0, 0, 255]),
                why: "outside every band the opaque black backdrop is untouched",
            },
        ],
        eroded_interior: false,
        about: "four half-opacity sibling layers at the same nesting depth, never nested — the \
                staggered list-entrance shape a fan wider than two forces the scheduler to cut \
                a round to serve",
    }
}

/// A nested isolated PAIR: one isolated `PushLayer` (the parent) holding two
/// more isolated `PushLayer`s at the depth below it, siblings of EACH OTHER,
/// non-overlapping bands inside the parent's own rect.
///
/// `frust_engine::schedule::mod`'s own module docs name this exact
/// combination: "an isolated parent's *second* isolated child is served
/// while a chain hanging off it is not — the parent's page is one of the two
/// groups from its first cut onwards". [`layer_alpha`] already pins a pure
/// CHAIN three deep and [`layer_sibling_fan`] a pure fan with no isolated
/// ancestor; this case is the one that combines the two — nesting AND a
/// sibling fan under one isolated parent — the exact combination the
/// hoist+cutting diff newly serves with zero pixel verification before this
/// case.
fn layer_nested_pair() -> CorpusCase {
    fn record(scene: &mut Scene) {
        let mut builder = SceneBuilder::new(scene);
        black_backdrop(&mut builder);
        builder.push_layer(Rect::new(4.0, 4.0, 60.0, 60.0), 0.5);
        builder.push_layer(Rect::new(4.0, 4.0, 60.0, 30.0), 0.5);
        builder.fill_rect(Rect::new(4.0, 4.0, 60.0, 30.0), Brush::Solid(WHITE));
        builder.pop_layer();
        builder.push_layer(Rect::new(4.0, 32.0, 60.0, 60.0), 0.5);
        builder.fill_rect(Rect::new(4.0, 32.0, 60.0, 60.0), Brush::Solid(WHITE));
        builder.pop_layer();
        builder.pop_layer();
    }
    CorpusCase {
        spec: case("unit-layer-nested-pair").with_base_color(BLACK),
        record,
        probes: &[
            Probe {
                x: 32,
                y: 16,
                expect: Expect::Channel {
                    channel: 0,
                    value: 64,
                    tolerance: 2,
                },
                why: "the first isolated child composes its own 0.5 with the isolated parent's \
                      0.5: 255 * 0.5 * 0.5 over black",
            },
            Probe {
                x: 32,
                y: 46,
                expect: Expect::Channel {
                    channel: 0,
                    value: 64,
                    tolerance: 2,
                },
                why: "the second isolated child — the parent's page reused for its sibling — \
                      composites identically to the first",
            },
            Probe {
                x: 2,
                y: 2,
                expect: Expect::Exact([0, 0, 0, 255]),
                why: "outside the isolated parent's own rect the opaque black backdrop is \
                      untouched",
            },
        ],
        eroded_interior: false,
        about: "an isolated parent holding two isolated children as siblings of each other — \
                nesting combined with a sibling fan under one isolated ancestor",
    }
}

/// The right edge of [`clear_rect`]'s punch: one pixel PAST the 16-px tile
/// boundary at `SIZE / 2`, which is what makes the case a regression test
/// rather than a smoke test.
const PUNCH_EDGE: f64 = (SIZE / 2 + 1) as f64;

/// PROMOTED from `crates/frust-render/tests/gpu_smoke.rs`'s
/// `clear_rect_punches_pixel_exact_through_a_layer_group`, geometry and
/// assertions verbatim.
///
/// Two defects, both first caught on-device, are pinned here at once:
///
/// 1. A `Command::ClearRect` recorded INSIDE a clip/opacity group must still
///    erase an opaque backdrop painted OUTSIDE the group. A group-local erase
///    would be sealed in by the group composite, so the encode walk hoists
///    the punch to root.
/// 2. The erase must be PIXEL-EXACT at a 16-px-tile-UNALIGNED edge.
///    `vello_common`'s `Compose::Clear` bleeds to whole boundary tiles, which
///    is why the punch uses `Compose::DestOut` instead — the probes just past
///    `PUNCH_EDGE` are inside the SAME 16-px tile as the punch edge, and they
///    must still be opaque red.
///
/// Tolerance is [`Tolerance::exact`]: this case is an alpha and coverage
/// contract on integer-aligned geometry, so a single differing channel step
/// anywhere is a regression, not rounding.
fn clear_rect() -> CorpusCase {
    fn record(scene: &mut Scene) {
        let mut builder = SceneBuilder::new(scene);
        builder.fill_rect(Rect::new(0.0, 0.0, EDGE, EDGE), Brush::Solid(RED));
        builder.push_layer(Rect::new(0.0, 0.0, EDGE, EDGE), 1.0);
        builder.clear_rect(Rect::new(0.0, 0.0, PUNCH_EDGE, EDGE));
        builder.pop_layer();
    }
    CorpusCase {
        spec: case("unit-clear-rect").with_tolerance(Tolerance::exact()),
        record,
        probes: &[
            Probe {
                x: SIZE / 4,
                y: SIZE / 2,
                expect: Expect::Exact([0, 0, 0, 0]),
                why: "punch centre must be erased",
            },
            Probe {
                x: SIZE / 2,
                y: SIZE / 2,
                expect: Expect::Exact([0, 0, 0, 0]),
                why: "last px inside the unaligned edge must be erased",
            },
            Probe {
                x: SIZE / 2 + 1,
                y: SIZE / 2,
                expect: Expect::AtLeast {
                    channel: 0,
                    min: 201,
                },
                why: "backdrop in the same 16-px tile as the punch edge must stay opaque red",
            },
            Probe {
                x: SIZE / 2 + 1,
                y: SIZE / 2,
                expect: Expect::AtLeast {
                    channel: 3,
                    min: 201,
                },
                why: "backdrop in the same 16-px tile as the punch edge must stay opaque",
            },
            Probe {
                x: SIZE / 2 + 4,
                y: SIZE / 2,
                expect: Expect::AtLeast {
                    channel: 0,
                    min: 201,
                },
                why: "backdrop four px past the punch edge must stay opaque red",
            },
            Probe {
                x: SIZE / 2 + 4,
                y: SIZE / 2,
                expect: Expect::AtLeast {
                    channel: 3,
                    min: 201,
                },
                why: "backdrop four px past the punch edge must stay opaque",
            },
            Probe {
                x: SIZE / 2 + 14,
                y: SIZE / 2,
                expect: Expect::AtLeast {
                    channel: 0,
                    min: 201,
                },
                why: "backdrop at the far end of the punch edge's tile must stay opaque red",
            },
            Probe {
                x: SIZE / 2 + 14,
                y: SIZE / 2,
                expect: Expect::AtLeast {
                    channel: 3,
                    min: 201,
                },
                why: "backdrop at the far end of the punch edge's tile must stay opaque",
            },
        ],
        eroded_interior: false,
        about: "promoted: a hole punch erasing through a layer group, pixel-exact at an \
                unaligned tile edge",
    }
}

/// The arbitrary path both [`path_fill`] and [`path_stroke`]/[`path_dashed`]
/// use: a closed shape with a straight run, a cubic and a quadratic, so one
/// path exercises every segment kind the flattener handles.
fn sample_path() -> BezPath {
    let mut path = BezPath::new();
    path.move_to((8.0, 48.0));
    path.line_to((8.0, 20.0));
    path.curve_to((20.0, 4.0), (44.0, 4.0), (56.0, 20.0));
    path.quad_to((56.0, 40.0), (32.0, 56.0));
    path.close_path();
    path
}

/// `Command::Path` with [`frust_scene::PathStyle::Fill`] — the nonzero-winding
/// fill of an arbitrary path.
fn path_fill() -> CorpusCase {
    fn record(scene: &mut Scene) {
        let mut builder = SceneBuilder::new(scene);
        builder.fill_path(sample_path(), Brush::Solid(RED));
    }
    CorpusCase {
        spec: case("unit-path-fill"),
        record,
        probes: &[Probe {
            x: 32,
            y: 32,
            expect: Expect::Exact([255, 0, 0, 255]),
            why: "the path's interior is filled solid",
        }],
        eroded_interior: false,
        about: "a closed line/cubic/quadratic path, filled",
    }
}

/// `Command::Path` with `PathStyle::Stroke { dash: None }` — the same
/// geometry as [`path_fill`], stroked, so the two goldens read as a matched
/// pair.
fn path_stroke() -> CorpusCase {
    fn record(scene: &mut Scene) {
        let mut builder = SceneBuilder::new(scene);
        builder.stroke_path(sample_path(), 3.0, Brush::Solid(BLUE));
    }
    CorpusCase {
        spec: case("unit-path-stroke"),
        record,
        probes: &[Probe {
            x: 32,
            y: 32,
            expect: Expect::Exact([0, 0, 0, 0]),
            why: "a stroke paints the outline only — the interior stays empty",
        }],
        eroded_interior: false,
        about: "the same path, stroked at 3 px",
    }
}

/// `Command::Path` with an EFFECTIVE `DashPattern`.
///
/// A non-zero phase is part of the case on purpose: the pattern is
/// pre-flattened into sub-paths at encode time (neither backend's stroker
/// reads a `Stroke`'s dash fields), so a phase that were dropped somewhere in
/// that flattening would shift every dash along the path — visible against
/// this golden, invisible against a zero-phase one.
fn path_dashed() -> CorpusCase {
    fn record(scene: &mut Scene) {
        let mut builder = SceneBuilder::new(scene);
        builder.stroke_path_dashed(
            sample_path(),
            3.0,
            DashPattern::new(6.0, 4.0).with_phase(2.0),
            Brush::Solid(GREEN),
        );
    }
    CorpusCase {
        spec: case("unit-path-dashed"),
        record,
        probes: &[Probe {
            x: 32,
            y: 32,
            expect: Expect::Exact([0, 0, 0, 0]),
            why: "a dashed stroke paints the outline only — the interior stays empty",
        }],
        eroded_interior: false,
        about: "the same path, dashed 6-on/4-off at phase 2",
    }
}

/// A self-contained fullscreen-triangle WGSL module returning solid magenta —
/// the shape a `Command::ShaderQuad`'s program takes (a `vs_main` emitting a
/// fullscreen triangle from `@builtin(vertex_index)`, plus an `fs_main`
/// writing `Rgba8Unorm`), and opaque as `ShaderProgram`'s v1 contract
/// requires.
const SOLID_MAGENTA_WGSL: &str = r#"
struct VsOut { @builtin(position) pos: vec4<f32> };
@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
    var out: VsOut;
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    out.pos = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    return out;
}
@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return vec4<f32>(1.0, 0.0, 1.0, 1.0);
}
"#;

/// `Command::ShaderQuad`, rendered by the engine but scored by neither golden
/// arm — see `docs/LIMITATIONS.md` `engine-shader-quad-goldens-uncomparable`.
///
/// The engine renders the program in a pre-pass and draws the result as an
/// external texture, but the engine oracle encodes straight through the
/// renderer without driving that pre-pass, so a compiled GPU frame of this
/// case would legitimately be missing the very thing the case exists to pin;
/// the golden gate holds it out of its scored set for exactly that reason
/// rather than comparing against such a frame. There is also no shader
/// pre-pass without a GPU, so the CPU arm has nothing equivalent to lower
/// this to — its own opaque dark placeholder color exists for that gap, but
/// this case names [`ORACLE_ID`] in its own skip set, so the placeholder is
/// never even produced here, let alone compared. What this case pins, then,
/// is narrower than a rendered frame on either arm: that recording a
/// `Command::ShaderQuad` and handing the scene to a renderer does not panic.
/// Coverage of the command actually painting lives in the engine's own
/// device-backed shader-quad tests.
fn shader_quad() -> CorpusCase {
    fn record(scene: &mut Scene) {
        let program = ShaderProgram::new(SOLID_MAGENTA_WGSL);
        let mut builder = SceneBuilder::new(scene);
        builder.draw_shader(&program, Rect::new(8.0, 8.0, 56.0, 56.0), 0.0);
    }
    CorpusCase {
        spec: case("unit-shader-quad").with_skip(BackendSet::new([ORACLE_ID])),
        record,
        probes: &[],
        eroded_interior: false,
        about: "a shader quad on a GPU arm only — the CPU oracle has no shader pre-pass",
    }
}

/// PROMOTED from `crates/frust-render/tests/gpu_smoke.rs`'s
/// `snapshot_bracket_inline_emulation_matches_the_composite_arithmetic`,
/// geometry and assertions verbatim.
///
/// A `Command::PushRetained` bracket lowered INLINE — the only lowering this
/// framework has for one, on every render path and every tier — and the
/// absolute arithmetic it must hit:
///
/// - the opaque red block composites at the bracket's 0.75 alpha to 191;
/// - the half-alpha green band composites at `128 * 0.5 * 0.75` to 48 —
///   the number a doubly premultiplied page would halve again;
/// - a command recorded AFTER the bracket lands on top of it;
/// - content recorded BEFORE the bracket survives.
///
/// The geometry keeps every content edge on a whole device pixel, so nothing
/// here measures rasterizer subpixel coverage, and the tolerance is the
/// original's: one 8-bit rounding step, no more.
fn snapshot_bracket() -> CorpusCase {
    fn record(scene: &mut Scene) {
        // A 20x20 body in local space under a 2x device scale offset by
        // (8, 8) — device (8, 8)..(48, 48) — composited at 75% opacity.
        const ALPHA: f32 = 0.75;
        let bracket = kurbo::Affine::translate((8.0, 8.0)) * kurbo::Affine::scale(2.0);
        let rect = Rect::new(0.0, 0.0, 20.0, 20.0);
        let overlay = Color::from_rgba8(255, 255, 0, 255);

        let mut builder = SceneBuilder::new(scene);
        // An opaque black backdrop, which is what makes the composite
        // arithmetic exact, plus a marker outside the bracket.
        black_backdrop(&mut builder);
        builder.fill_rect(Rect::new(52.0, 52.0, 60.0, 60.0), Brush::Solid(BLUE));
        builder.push_transform(bracket);
        builder.push_retained(1, rect, ALPHA, 1.0);
        // An opaque red block, a half-alpha green band over NOTHING (so the
        // bracket really carries partial alpha — a double premultiply is
        // invisible where alpha is 1), and a black bar standing in for a line
        // of text.
        builder.fill_rect(Rect::new(0.0, 0.0, 20.0, 10.0), Brush::Solid(RED));
        builder.fill_rect(
            Rect::new(0.0, 10.0, 20.0, 15.0),
            Brush::Solid(GREEN.with_alpha(0.5)),
        );
        builder.fill_rect(Rect::new(5.0, 15.0, 15.0, 20.0), Brush::Solid(BLACK));
        builder.pop_retained();
        builder.pop_transform();
        // Recorded AFTER the bracket, so it must land on top of it.
        builder.fill_rect(Rect::new(20.0, 20.0, 30.0, 30.0), Brush::Solid(overlay));
    }
    CorpusCase {
        spec: case("unit-snapshot-bracket")
            .with_base_color(BLACK)
            .with_tolerance(Tolerance::new().with_channel(3).with_alpha(3)),
        record,
        probes: &[
            Probe {
                x: 40,
                y: 12,
                expect: Expect::Channel {
                    channel: 0,
                    value: 191,
                    tolerance: 3,
                },
                why: "the bracket's opaque block must composite at its alpha",
            },
            Probe {
                x: 40,
                y: 32,
                expect: Expect::Channel {
                    channel: 1,
                    value: 48,
                    tolerance: 3,
                },
                why: "the bracket's half-alpha band must composite at 128 * 0.5 * 0.75",
            },
            Probe {
                x: 24,
                y: 24,
                expect: Expect::Exact([255, 255, 0, 255]),
                why: "a command recorded after the bracket must land on top of it",
            },
            Probe {
                x: 56,
                y: 56,
                expect: Expect::AtLeast {
                    channel: 2,
                    min: 201,
                },
                why: "content recorded before the bracket must survive",
            },
        ],
        eroded_interior: false,
        about: "promoted: the inline lowering of a snapshot bracket, absolute composite \
                arithmetic",
    }
}

/// Snapshot NESTING: two brackets, the inner one carrying a different alpha
/// AND a different scale, both of which must be IGNORED.
///
/// `Command::PushRetained`'s contract is that syntactic nesting is allowed
/// but only the OUTERMOST bracket needs honouring. The probes are stated as
/// the outer bracket's 0.5 alone: if the inner bracket's 0.25 were composed
/// in as well, the green band would land at `128 * 0.5 * 0.25` (16) instead
/// of `128 * 0.5` (64) — a difference no tolerance absorbs.
fn snapshot_balance() -> CorpusCase {
    fn record(scene: &mut Scene) {
        let bracket = kurbo::Affine::translate((8.0, 8.0)) * kurbo::Affine::scale(2.0);
        let rect = Rect::new(0.0, 0.0, 20.0, 20.0);

        let mut builder = SceneBuilder::new(scene);
        black_backdrop(&mut builder);
        builder.push_transform(bracket);
        builder.push_retained(1, rect, 0.5, 1.0);
        builder.fill_rect(Rect::new(0.0, 0.0, 20.0, 10.0), Brush::Solid(WHITE));
        // Inner bracket: its own alpha and scale must not reach the pixels.
        builder.push_retained(2, rect, 0.25, 2.0);
        builder.fill_rect(Rect::new(0.0, 10.0, 20.0, 20.0), Brush::Solid(GREEN));
        builder.pop_retained();
        builder.pop_retained();
        builder.pop_transform();
    }
    CorpusCase {
        spec: case("unit-snapshot-balance")
            .with_base_color(BLACK)
            .with_tolerance(Tolerance::new().with_channel(3).with_alpha(3)),
        record,
        probes: &[
            Probe {
                x: 24,
                y: 16,
                expect: Expect::Channel {
                    channel: 0,
                    value: 128,
                    tolerance: 3,
                },
                why: "the outer bracket's 0.5 alpha over black",
            },
            Probe {
                x: 24,
                y: 38,
                expect: Expect::Channel {
                    channel: 1,
                    value: 64,
                    tolerance: 3,
                },
                why: "the inner bracket's own 0.25 alpha must be ignored — 128 * 0.5, not \
                      128 * 0.5 * 0.25",
            },
        ],
        eroded_interior: false,
        about: "nested snapshot brackets, only the outermost honoured",
    }
}

/// The paint-class cases: frames that put every strip paint class the engine
/// specialises its fragment program by — solid coverage, gradient, image,
/// blurred rectangle, layer composite — into ONE frame, in interleaved
/// painter order.
///
/// `frust-engine` draws each pass in runs of one class through that class's
/// own program, falling back to the whole (Uber) program for a run shorter than
/// its threshold. A per-class program that rendered any paint differently from
/// the whole one would show here, against the same `vello_cpu` reference every
/// unit case is held to: these frames are built so the classes alternate, a
/// run below the threshold sits between two long ones, and a layer composites
/// over a gradient.
///
/// Registered in [`unit_cases`] with their reviewed `cpu/` baselines, and
/// named in the engine golden gate's `PHASE_CASES`; the
/// `tests::the_paint_class_cases_hold_cpu_oracle_parity` test additionally
/// holds them to the CPU oracle directly, at each case's own tolerance.
#[must_use]
pub fn paint_class_cases() -> Vec<CorpusCase> {
    vec![
        paint_class_mix(),
        paint_class_short_run(),
        paint_class_layer_over_gradient(),
    ]
}

/// A two-stop gradient brush over `gradient`, from `a` to `b`.
fn two_stops(gradient: Gradient, a: Color, b: Color) -> Brush {
    Brush::Gradient(gradient.with_stops([
        ColorStop {
            offset: 0.0,
            color: DynamicColor::from_alpha_color(a),
        },
        ColorStop {
            offset: 1.0,
            color: DynamicColor::from_alpha_color(b),
        },
    ]))
}

/// Every class in one 64x64 frame, one quadrant each over an opaque
/// backdrop: a radial-gradient disc, a linear-gradient rounded rect, the
/// magnified checkerboard image, and a blurred rounded rect — then a solid
/// stroke crossing all four, so solid instances interleave with every other
/// class in painter order.
fn paint_class_mix() -> CorpusCase {
    fn record(scene: &mut Scene) {
        let mut builder = SceneBuilder::new(scene);
        black_backdrop(&mut builder);
        builder.fill_path(
            kurbo::Circle::new((16.0, 16.0), 13.0).to_path(0.1),
            two_stops(
                Gradient::new_radial((14.0, 14.0), 13.0),
                Color::from_rgb8(255, 220, 0),
                Color::from_rgb8(200, 0, 60),
            ),
        );
        builder.fill_rounded_rect(
            Rect::new(36.0, 4.0, 60.0, 28.0),
            5.0,
            two_stops(
                Gradient::new_linear((36.0, 4.0), (60.0, 28.0)),
                Color::from_rgb8(0, 200, 255),
                Color::from_rgb8(120, 0, 255),
            ),
        );
        builder.draw_image(&checkerboard(), Rect::new(4.0, 36.0, 28.0, 60.0));
        builder.draw_blurred_rounded_rect(Rect::new(38.0, 38.0, 58.0, 58.0), 4.0, 2.0, WHITE);
        builder.stroke_line(
            Point::new(2.0, 32.5),
            Point::new(62.0, 32.5),
            3.0,
            Brush::Solid(GREEN),
        );
        builder.stroke_line(
            Point::new(32.5, 2.0),
            Point::new(32.5, 62.0),
            3.0,
            Brush::Solid(GREEN),
        );
    }
    CorpusCase {
        spec: case("unit-paint-class-mix"),
        record,
        probes: &[
            Probe {
                x: 32,
                y: 10,
                expect: Expect::Exact([0, 128, 0, 255]),
                why: "the solid stroke is drawn over the gradient quadrants' gap in painter order",
            },
            Probe {
                x: 6,
                y: 38,
                expect: Expect::AtLeast {
                    channel: 3,
                    min: 255,
                },
                why: "the opaque image keeps its quadrant opaque",
            },
        ],
        eroded_interior: false,
        about: "radial + linear gradients, an image, a blurred rect and solid strokes interleaved",
    }
}

/// A run below the engine's split threshold between two long ones: a tall
/// sweep-gradient band, a handful of tiny solid and image marks, then a tall
/// linear-gradient band — the short middle run is drawn with the whole
/// program, the bands with the gradient program.
fn paint_class_short_run() -> CorpusCase {
    fn record(scene: &mut Scene) {
        let mut builder = SceneBuilder::new(scene);
        builder.fill_rect(
            Rect::new(2.5, 2.5, 30.5, 61.5),
            two_stops(
                Gradient::new_sweep((16.0, 32.0), 0.0, std::f32::consts::TAU),
                Color::from_rgb8(255, 0, 0),
                Color::from_rgba8(0, 0, 255, 160),
            ),
        );
        builder.fill_rect(Rect::new(31.0, 10.0, 33.0, 12.0), Brush::Solid(WHITE));
        builder.draw_image(&checkerboard(), Rect::new(30.0, 20.0, 34.0, 24.0));
        builder.draw_blurred_rounded_rect(Rect::new(30.0, 40.0, 34.0, 44.0), 1.0, 0.5, YELLOW);
        builder.fill_rect(
            Rect::new(33.5, 2.5, 61.5, 61.5),
            two_stops(
                Gradient::new_linear((33.0, 2.0), (61.0, 61.0)),
                Color::from_rgba8(0, 255, 0, 200),
                Color::from_rgb8(255, 0, 255),
            ),
        );
    }
    CorpusCase {
        spec: case("unit-paint-class-short-run"),
        record,
        probes: &[Probe {
            x: 1,
            y: 1,
            expect: Expect::Exact([0, 0, 0, 0]),
            why: "nothing is drawn outside the two bands",
        }],
        eroded_interior: false,
        about: "two long gradient runs around a short mixed run below the split threshold",
    }
}

/// A translucent layer composited over a gradient: the page round holds a
/// solid, a radial gradient and the image, and its composite quad — the layer
/// class — reads the page back over a linear-gradient backdrop.
fn paint_class_layer_over_gradient() -> CorpusCase {
    fn record(scene: &mut Scene) {
        let mut builder = SceneBuilder::new(scene);
        builder.fill_rect(
            Rect::new(0.0, 0.0, EDGE, EDGE),
            two_stops(
                Gradient::new_linear((0.0, 0.0), (EDGE, EDGE)),
                Color::from_rgb8(20, 200, 120),
                Color::from_rgb8(200, 20, 120),
            ),
        );
        builder.push_layer(Rect::new(8.0, 8.0, 56.0, 56.0), 0.5);
        builder.fill_rect(Rect::new(8.0, 8.0, 56.0, 32.0), Brush::Solid(WHITE));
        builder.fill_path(
            kurbo::Circle::new((32.0, 36.0), 16.0).to_path(0.1),
            two_stops(
                Gradient::new_radial((32.0, 36.0), 16.0),
                Color::from_rgb8(0, 0, 255),
                Color::from_rgb8(255, 255, 0),
            ),
        );
        builder.draw_image(&checkerboard(), Rect::new(40.0, 40.0, 56.0, 56.0));
        builder.pop_layer();
    }
    CorpusCase {
        spec: case("unit-paint-class-layer-over-gradient"),
        record,
        probes: &[Probe {
            x: 2,
            y: 2,
            expect: Expect::AtLeast {
                channel: 3,
                min: 255,
            },
            why: "the opaque gradient backdrop covers the frame outside the layer",
        }],
        eroded_interior: false,
        about: "a half-opacity layer of solid, gradient and image composited over a gradient",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_scene::Command;
    use std::collections::BTreeSet;

    /// The variant name of a recorded command — the vocabulary the coverage
    /// assertion below counts in.
    fn variant_name(command: &Command) -> &'static str {
        match command {
            Command::FillRect { .. } => "FillRect",
            Command::RoundedRect { .. } => "RoundedRect",
            Command::Line { .. } => "Line",
            Command::GlyphRun(_) => "GlyphRun",
            Command::PushClip { .. } => "PushClip",
            Command::PushClipRounded { .. } => "PushClipRounded",
            Command::PushClipPath { .. } => "PushClipPath",
            Command::PopClip => "PopClip",
            Command::Image { .. } => "Image",
            Command::BlurredRoundedRect { .. } => "BlurredRoundedRect",
            Command::PushLayer { .. } => "PushLayer",
            Command::PopLayer => "PopLayer",
            Command::ClearRect { .. } => "ClearRect",
            Command::Path { .. } => "Path",
            Command::ShaderQuad { .. } => "ShaderQuad",
            Command::PushRetained { .. } => "PushRetained",
            Command::PopRetained => "PopRetained",
            Command::DrawRetained { .. } => "DrawRetained",
            Command::SceneTexture { .. } => "SceneTexture",
            Command::BackdropBlur { .. } => "BackdropBlur",
        }
    }

    /// Every `Command` variant a corpus case records, as `variant_name` spells
    /// them. `DrawRetained` is deliberately absent: a corpus case is one frame,
    /// so no page can be resident when it is drawn, and the CPU oracle retains
    /// nothing at all — so a corpus scene never records one (see
    /// `no_corpus_case_records_a_draw_retained`). The engine's retained pages
    /// are pinned across frames by `engine_goldens`'s `retained_parity` case
    /// instead.
    const EVERY_VARIANT: &[&str] = &[
        "FillRect",
        "RoundedRect",
        "Line",
        "GlyphRun",
        "PushClip",
        "PushClipRounded",
        "PushClipPath",
        "PopClip",
        "Image",
        "BlurredRoundedRect",
        "PushLayer",
        "PopLayer",
        "ClearRect",
        "Path",
        "ShaderQuad",
        "PushRetained",
        "PopRetained",
        "BackdropBlur",
    ];

    #[test]
    fn every_command_variant_is_covered() {
        let mut seen = BTreeSet::new();
        for case in unit_cases() {
            for command in case.scene().commands() {
                seen.insert(variant_name(command));
            }
        }
        let missing: Vec<&&str> = EVERY_VARIANT
            .iter()
            .filter(|name| !seen.contains(**name))
            .collect();
        assert!(
            missing.is_empty(),
            "the unit corpus records no case for {missing:?} — every Command variant needs one"
        );
        assert_eq!(
            seen.len(),
            EVERY_VARIANT.len(),
            "variant_name reported a name outside EVERY_VARIANT: {seen:?}"
        );
    }

    #[test]
    fn no_corpus_case_records_a_draw_retained() {
        for case in unit_cases()
            .into_iter()
            .chain(crate::corpus::adversarial::adversarial_cases())
        {
            assert!(
                !case
                    .scene()
                    .commands()
                    .iter()
                    .any(|command| matches!(command, Command::DrawRetained { .. })),
                "`{}` records a DrawRetained, which no single-frame case can draw",
                case.spec.name
            );
        }
    }

    #[test]
    fn case_names_are_unique_and_prefixed() {
        let cases = unit_cases();
        let names: BTreeSet<&str> = cases.iter().map(|case| case.spec.name).collect();
        assert_eq!(
            names.len(),
            cases.len(),
            "two unit cases share a name — the name is the golden artifact's file stem"
        );
        for case in &cases {
            assert!(
                case.spec.name.starts_with("unit-"),
                "`{}` is in the unit corpus but is not `unit-`-prefixed",
                case.spec.name
            );
            assert!(
                !case.about.is_empty(),
                "`{}` has no `about` line",
                case.spec.name
            );
        }
    }

    #[test]
    fn every_case_records_at_least_one_command_at_the_declared_size() {
        for case in unit_cases() {
            assert!(
                !case.scene().commands().is_empty(),
                "`{}` records an empty scene",
                case.spec.name
            );
            assert_eq!(case.spec.width, SIZE, "`{}` width", case.spec.name);
            assert_eq!(case.spec.height, SIZE, "`{}` height", case.spec.name);
        }
    }

    #[test]
    fn the_shader_case_is_the_only_one_skipped_on_the_cpu_oracle() {
        let skipped: Vec<&str> = unit_cases()
            .iter()
            .filter(|case| case.spec.skip.contains(ORACLE_ID))
            .map(|case| case.spec.name)
            .collect();
        assert_eq!(skipped, vec!["unit-shader-quad"]);
    }

    #[test]
    fn the_promoted_cases_keep_their_original_thresholds() {
        let cases = unit_cases();
        let find = |name: &str| {
            cases
                .iter()
                .find(|case| case.spec.name == name)
                .unwrap_or_else(|| panic!("{name} missing from the corpus"))
        };

        // Promoted verbatim: the hole punch is an exact-match case.
        let clear = find("unit-clear-rect");
        assert_eq!(clear.spec.tolerance, Tolerance::exact());
        assert!(
            clear.probes.iter().any(|probe| {
                probe.expect == Expect::Exact([0, 0, 0, 0]) && probe.x == SIZE / 2
            })
        );

        // Promoted verbatim: 191 / 48 at a tolerance of one 8-bit step.
        let snapshot = find("unit-snapshot-bracket");
        assert_eq!(snapshot.spec.tolerance.channel, 3);
        assert!(snapshot.probes.iter().any(|probe| {
            probe.expect
                == Expect::Channel {
                    channel: 0,
                    value: 191,
                    tolerance: 3,
                }
        }));
        assert!(snapshot.probes.iter().any(|probe| {
            probe.expect
                == Expect::Channel {
                    channel: 1,
                    value: 48,
                    tolerance: 3,
                }
        }));
    }

    #[test]
    fn the_paint_class_cases_are_unit_shaped_and_registered_in_the_unit_corpus() {
        let baselined: BTreeSet<&str> = unit_cases().iter().map(|case| case.spec.name).collect();
        let cases = paint_class_cases();
        let names: BTreeSet<&str> = cases.iter().map(|case| case.spec.name).collect();
        assert_eq!(names.len(), cases.len());
        for case in &cases {
            assert!(case.spec.name.starts_with("unit-paint-class-"));
            assert!(baselined.contains(case.spec.name));
            assert!(!case.about.is_empty());
            assert!(!case.scene().commands().is_empty());
            assert_eq!((case.spec.width, case.spec.height), (SIZE, SIZE));
            assert!(
                case.spec.skip.is_empty(),
                "both oracles draw every paint class"
            );
        }
        // Together they draw every class the engine specialises by: solid,
        // gradient, image, blurred rect, and a layer composite.
        let mut seen = BTreeSet::new();
        for case in &cases {
            for command in case.scene().commands() {
                seen.insert(variant_name(command));
            }
        }
        for needed in [
            "FillRect",
            "Path",
            "Image",
            "BlurredRoundedRect",
            "PushLayer",
        ] {
            assert!(
                seen.contains(needed),
                "no paint-class case records {needed}"
            );
        }
    }

    /// The paint-class cases against the CPU oracle, at each case's own
    /// tolerance, plus their probes — the parity the engine's per-class
    /// programs are held to. GPU-gated; the pinned-runner recipe is
    /// `engine_goldens`'.
    #[test]
    #[ignore = "requires a GPU; run with `cargo test -p frust-testing --lib -- --ignored`"]
    fn the_paint_class_cases_hold_cpu_oracle_parity() {
        use crate::corpus::straighten_alpha;
        use crate::diff::diff_images;
        use crate::oracle_cpu::CpuOracle;
        use crate::oracle_engine::{EngineOracle, EngineOracleOptions};
        use crate::render::{RenderedImage, SceneRenderer};
        use image::RgbaImage;

        fn rgba(image: &RenderedImage) -> RgbaImage {
            RgbaImage::from_raw(image.width, image.height, image.rgba8.clone())
                .expect("a rendered frame is width*height*4 bytes")
        }

        let mut engine =
            EngineOracle::new(&EngineOracleOptions::default()).expect("an engine oracle");
        let mut cpu = CpuOracle::new();
        let mut failures = Vec::new();
        for case in paint_class_cases() {
            let name = case.spec.name;
            let scene = case.scene();
            let spec = case.render_spec();
            let gpu = engine
                .render(&scene, &spec)
                .expect("the engine renders the case");
            let reference = cpu
                .render(&scene, &spec)
                .expect("the CPU oracle renders the case");
            let outcome = diff_images(
                &rgba(&reference),
                &rgba(&gpu),
                case.spec.tolerance,
                case.eroded_interior,
            );
            println!(
                "paint-class parity: [{name}] {} px differ, max |delta| {:?}",
                outcome.report.pixel_count, outcome.report.max_difference
            );
            if !outcome.passed {
                failures.push(format!(
                    "[{name}] engine vs cpu beyond {:?}: {} px, max {:?}",
                    case.spec.tolerance, outcome.report.pixel_count, outcome.report.max_difference
                ));
            }
            for (arm, image) in [("engine", &gpu), ("cpu", &reference)] {
                for message in case.failed_probes(&straighten_alpha(image)) {
                    failures.push(format!("[{name}] {arm} probe: {message}"));
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    /// The image-filtering cases against the CPU oracle, at each case's own
    /// tolerance, plus their probes, through whichever strip program family
    /// the process configuration names — run it once plain (the sampler
    /// family) and once under `FRUST_ENGINE_IMAGE_SAMPLER=0` (the manual one)
    /// to hold both. GPU-gated; the pinned-runner recipe is `engine_goldens`'.
    #[test]
    #[ignore = "requires a GPU; run with `cargo test -p frust-testing --lib -- --ignored`"]
    fn the_image_sampler_cases_hold_cpu_oracle_parity() {
        use crate::corpus::straighten_alpha;
        use crate::diff::diff_images;
        use crate::oracle_cpu::CpuOracle;
        use crate::oracle_engine::{EngineOracle, EngineOracleOptions};
        use crate::render::{RenderedImage, SceneRenderer};
        use image::RgbaImage;

        fn rgba(image: &RenderedImage) -> RgbaImage {
            RgbaImage::from_raw(image.width, image.height, image.rgba8.clone())
                .expect("a rendered frame is width*height*4 bytes")
        }

        let mut engine =
            EngineOracle::new(&EngineOracleOptions::default()).expect("an engine oracle");
        let mut cpu = CpuOracle::new();
        let mut failures = Vec::new();
        for case in image_sampler_cases() {
            let name = case.spec.name;
            let scene = case.scene();
            let spec = case.render_spec();
            let gpu = engine
                .render(&scene, &spec)
                .expect("the engine renders the case");
            let reference = cpu
                .render(&scene, &spec)
                .expect("the CPU oracle renders the case");
            let outcome = diff_images(
                &rgba(&reference),
                &rgba(&gpu),
                case.spec.tolerance,
                case.eroded_interior,
            );
            println!(
                "image-sampler parity (FRUST_ENGINE_IMAGE_SAMPLER={}): [{name}] {} px differ, max \
                 |delta| {:?}",
                std::env::var("FRUST_ENGINE_IMAGE_SAMPLER").unwrap_or_default(),
                outcome.report.pixel_count,
                outcome.report.max_difference
            );
            if !outcome.passed {
                failures.push(format!(
                    "[{name}] engine vs cpu beyond {:?}: {} px, max {:?}",
                    case.spec.tolerance, outcome.report.pixel_count, outcome.report.max_difference
                ));
            }
            for (arm, image) in [("engine", &gpu), ("cpu", &reference)] {
                for message in case.failed_probes(&straighten_alpha(image)) {
                    failures.push(format!("[{name}] {arm} probe: {message}"));
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn the_image_sampler_cases_are_unit_shaped_and_magnify_four_times() {
        let baselined: BTreeSet<&str> = unit_cases().iter().map(|case| case.spec.name).collect();
        for case in image_sampler_cases() {
            assert!(case.spec.name.starts_with("unit-image-"));
            assert!(
                !baselined.contains(case.spec.name),
                "a measurement case carries no reviewed baseline"
            );
            assert_eq!((case.spec.width, case.spec.height), (SIZE, SIZE));
            assert!(case.spec.skip.is_empty(), "both oracles draw images");
            let images = case
                .scene()
                .commands()
                .iter()
                .filter_map(|command| match command {
                    Command::Image { data, dest, .. } => Some(dest.width() / f64::from(data.width)),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert!(!images.is_empty());
            assert!(
                images
                    .iter()
                    .all(|scale| (*scale - MAGNIFICATION).abs() < 1e-9)
            );
        }
    }

    /// The glyph-quad case mixes the three routes it exists to mix, and is a
    /// measurement case rather than a baselined one.
    #[test]
    fn the_glyph_quad_cases_mix_cached_outline_and_colour_runs() {
        use crate::fonts::test_fonts;

        let baselined: BTreeSet<&str> = unit_cases().iter().map(|case| case.spec.name).collect();
        let emoji = test_fonts()
            .into_iter()
            .find(|(label, _)| label.contains("COLR"))
            .map(|(_, bytes)| bytes)
            .expect("the bundled emoji face");
        for case in glyph_quad_cases() {
            assert!(case.spec.name.starts_with("unit-glyph-"));
            assert!(
                !baselined.contains(case.spec.name),
                "a measurement case carries no reviewed baseline"
            );
            assert_eq!((case.spec.width, case.spec.height), (SIZE, SIZE));
            assert!(case.spec.skip.is_empty(), "both oracles draw text");
            let scene = case.scene();
            let runs: Vec<&frust_scene::GlyphRun> = scene
                .commands()
                .iter()
                .filter_map(|command| match command {
                    Command::GlyphRun(run) => Some(run),
                    _ => None,
                })
                .collect();
            let colour = |run: &&frust_scene::GlyphRun| run.font.font().data.data() == emoji;
            let rotated = |run: &&frust_scene::GlyphRun| {
                let [a, b, c, d, _, _] = run.transform.as_coeffs();
                b != 0.0 || c != 0.0 || a != d
            };
            assert!(runs.iter().any(colour), "a colour run");
            assert!(
                runs.iter().any(|run| !colour(run) && rotated(run)),
                "an outline (rotated) run"
            );
            assert!(
                runs.iter().any(|run| !colour(run) && !rotated(run)),
                "a cached (translated) run"
            );

            // Through the engine's own compiler: the first frame rasterizes
            // the cached run (drawn the general way), the second draws it as
            // glyph quads, and every run draws on both.
            let (width, height) = (case.spec.width as u16, case.spec.height as u16);
            let mut compiler = frust_engine::SceneCompiler::new(width, height);
            let mut frame = || {
                let frame = compiler
                    .compile(&scene, kurbo::Affine::IDENTITY, (width, height))
                    .expect("the case compiles");
                compiler.acknowledge_glyph_clears();
                compiler.acknowledge_glyph_replay();
                frame
            };
            let first = frame();
            let second = frame();
            assert!(first.atlas_glyph_draws > 0, "the Latin run is cached");
            assert!(
                first.glyph_quads.is_empty(),
                "rasterized on the first frame"
            );
            assert_eq!(
                second.glyph_quads.len(),
                second.atlas_glyph_draws as usize,
                "the cached run is all quads once resident"
            );
            assert!(!second.glyph_quads.is_empty());
            assert!(
                second.glyph_draws > second.atlas_glyph_draws,
                "the rotated and colour runs draw too, off the atlas"
            );
            assert_eq!(first.glyph_draws, second.glyph_draws);
        }
    }

    /// The glyph-quad cases rendered three times through one engine: the
    /// frames after the first draw the cached run as glyph quads and must read
    /// back byte-identical to the first, which drew it the general way while
    /// rasterizing it — and each frame holds CPU-oracle parity at the case's
    /// tolerance plus its probes. GPU-gated; run it plain, under
    /// `FRUST_ENGINE_IMAGE_SAMPLER=0` and under `FRUST_ENGINE_UBER_PROGRAM=1`
    /// to hold both program families and the Uber program. The
    /// pinned-runner recipe is `engine_goldens`'.
    #[test]
    #[ignore = "requires a GPU; run with `cargo test -p frust-testing --lib -- --ignored`"]
    fn the_glyph_quad_cases_hold_their_first_frame_and_cpu_oracle_parity() {
        use crate::corpus::straighten_alpha;
        use crate::diff::diff_images;
        use crate::oracle_cpu::CpuOracle;
        use crate::oracle_engine::{EngineOracle, EngineOracleOptions};
        use crate::render::{RenderedImage, SceneRenderer};
        use image::RgbaImage;

        fn rgba(image: &RenderedImage) -> RgbaImage {
            RgbaImage::from_raw(image.width, image.height, image.rgba8.clone())
                .expect("a rendered frame is width*height*4 bytes")
        }

        let mut cpu = CpuOracle::new();
        let mut failures = Vec::new();
        for case in glyph_quad_cases() {
            let name = case.spec.name;
            let scene = case.scene();
            let spec = case.render_spec();
            // A fresh engine per case, so its first frame really is the one
            // that rasterizes the case's glyphs.
            let mut engine =
                EngineOracle::new(&EngineOracleOptions::default()).expect("an engine oracle");
            let frames: Vec<RenderedImage> = (0..3)
                .map(|_| {
                    engine
                        .render(&scene, &spec)
                        .expect("the engine renders the case")
                })
                .collect();
            let reference = cpu
                .render(&scene, &spec)
                .expect("the CPU oracle renders the case");
            for (index, frame) in frames.iter().enumerate() {
                let differing = frame
                    .rgba8
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .zip(frames[0].rgba8.as_chunks::<4>().0)
                    .filter(|(a, b)| a != b)
                    .count();
                let outcome = diff_images(
                    &rgba(&reference),
                    &rgba(frame),
                    case.spec.tolerance,
                    case.eroded_interior,
                );
                println!(
                    "glyph-quad parity (sampler={}, uber={}): [{name}] frame {index}: {differing} \
                     px differ from frame 0; vs cpu {} px differ, max |delta| {:?}",
                    std::env::var("FRUST_ENGINE_IMAGE_SAMPLER").unwrap_or_default(),
                    std::env::var("FRUST_ENGINE_UBER_PROGRAM").unwrap_or_default(),
                    outcome.report.pixel_count,
                    outcome.report.max_difference
                );
                if differing != 0 {
                    failures.push(format!(
                        "[{name}] frame {index}: {differing} px differ from the first frame"
                    ));
                }
                if !outcome.passed {
                    failures.push(format!(
                        "[{name}] frame {index}: engine vs cpu beyond {:?}: {} px, max {:?}",
                        case.spec.tolerance,
                        outcome.report.pixel_count,
                        outcome.report.max_difference
                    ));
                }
                for message in case.failed_probes(&straighten_alpha(frame)) {
                    failures.push(format!("[{name}] frame {index} probe: {message}"));
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn the_checkerboard_is_a_well_formed_rgba8_image() {
        let data = checkerboard();
        assert_eq!(data.width, CELL * CELLS);
        assert_eq!(data.height, CELL * CELLS);
        assert_eq!(data.format, ImageFormat::Rgba8);
        assert_eq!(
            data.data.len(),
            (data.width * data.height * 4) as usize,
            "an Rgba8 blob is exactly four bytes per pixel"
        );
    }
}
