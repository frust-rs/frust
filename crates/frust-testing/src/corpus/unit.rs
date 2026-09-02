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
//! case's recorded scene and asserts all 16 `Command` variants appear. A new
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
use kurbo::{BezPath, Point, Rect};
use peniko::color::palette::css::{BLACK, BLUE, GREEN, RED, WHITE, YELLOW};
use peniko::{Blob, Brush, Color, ImageAlphaType, ImageData, ImageFormat};

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
/// pixel-exact cases and the snapshot pair.
#[must_use]
pub fn unit_cases() -> Vec<CorpusCase> {
    vec![
        fill_rect(),
        rounded_rect(),
        stroke_line(),
        glyph_run(),
        clip_rect(),
        clip_rounded(),
        clip_balance(),
        image(),
        blur_rrect(),
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

/// `Command::ShaderQuad`, SKIPPED on the CPU oracle.
///
/// There is no shader pre-pass without a GPU, so the CPU arm can only lower
/// this to `convert.rs`'s opaque dark placeholder — a frame whose bytes are a
/// statement about the MISS path, not about the command. Comparing it against
/// a promoted baseline would be promoting a lie, so the case names
/// [`ORACLE_ID`] in its skip set and is never rendered there at all (rather
/// than rendered and ignored).
///
/// On a GPU arm the case renders through the same public `encode_scene` seam
/// an app does. This corpus registers no shader override, so what it pins is
/// the command reaching the renderer and taking its documented miss path
/// without a panic; the override-hit path needs `ShaderOverrideSpec`, which
/// belongs to a renderer-side case, not a scene-recorded one.
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
/// A `Command::PushSnapshot` bracket lowered INLINE — the only lowering this
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
        builder.push_snapshot(1, rect, ALPHA, 1.0);
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
        builder.pop_snapshot();
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
/// `Command::PushSnapshot`'s contract is that syntactic nesting is allowed
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
        builder.push_snapshot(1, rect, 0.5, 1.0);
        builder.fill_rect(Rect::new(0.0, 0.0, 20.0, 10.0), Brush::Solid(WHITE));
        // Inner bracket: its own alpha and scale must not reach the pixels.
        builder.push_snapshot(2, rect, 0.25, 2.0);
        builder.fill_rect(Rect::new(0.0, 10.0, 20.0, 20.0), Brush::Solid(GREEN));
        builder.pop_snapshot();
        builder.pop_snapshot();
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
        }
    }

    /// Every `Command` variant, as `variant_name` spells them.
    const EVERY_VARIANT: &[&str] = &[
        "FillRect",
        "RoundedRect",
        "Line",
        "GlyphRun",
        "PushClip",
        "PushClipRounded",
        "PopClip",
        "Image",
        "BlurredRoundedRect",
        "PushLayer",
        "PopLayer",
        "ClearRect",
        "Path",
        "ShaderQuad",
        "PushSnapshot",
        "PopSnapshot",
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
