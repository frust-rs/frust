//! The ADVERSARIAL corpus: cases chosen because they break renderers, not
//! because they exercise a `Command` variant for the first time (that is
//! [`super::unit`]'s job).
//!
//! # What this corpus is for
//!
//! Every case here names the failure mode it guards against in its own
//! `about` line and doc comment — degenerate geometry, non-finite transform
//! components, extreme scale/depth/count parameters, and the two documented
//! "ignored" policies ([`frust_scene::Command::PopClip`]/
//! [`frust_scene::Command::PopLayer`]/[`frust_scene::Command::PopSnapshot`]'s
//! unbalanced-pop contract). A case whose only reasonable assertion is "did
//! not panic" is marked [`crate::case::CaseSpec::no_ref`] (see each case's own
//! doc comment for why), the same policy [`super::unit`] uses for a case with
//! no reviewed reference yet.
//!
//! # Recording rule
//!
//! Every case records through [`SceneBuilder`] and nothing else — the same
//! rule [`super::unit`] documents, so a case can never encode a display list
//! the widget layer could not have produced (an unbalanced pop is reachable
//! from ordinary widget code; a hand-built [`frust_scene::Command`] is not).
//!
//! # Why most cases carry few or no probes
//!
//! [`crate::CpuOracle`] is bit-for-bit deterministic on a given target (see
//! `oracle_cpu`'s Determinism section), so the promoted `cpu/` baseline
//! itself is the regression gate for a case's exact pixels — a probe is only
//! needed for an assertion that must hold independently of any one
//! rasterizer's antialiasing choices (an exact zero where nothing was
//! painted, a composite arithmetic result, "did not panic"). [`super::unit`]'s
//! `unit-blur-rrect` already sets this precedent: a filtered-edge case is
//! promoted with no probes at all, because "the edge ramp is exactly where
//! two rasterizers legitimately differ".
use frust_scene::{Glyph, GlyphRun, Scene, SceneBuilder};
use frust_text::{FontFamily, TextContext, TextStyle};
use kurbo::{Affine, BezPath, Point, Rect};
use peniko::color::palette::css::{BLACK, BLUE, GREEN, RED, WHITE};
use peniko::{Blob, Brush, Color, ImageAlphaType, ImageData, ImageFormat};

use super::{CorpusCase, Expect, Probe};
use crate::case::CaseSpec;
use crate::fonts::register_test_fonts;

/// Every case in this corpus renders into a `SIZE`x`SIZE` frame unless it has
/// its own reason to differ (the 1-px-divider family fixes its own smaller
/// canvas so the same device-pixel indices are comparable across scales).
const SIZE: u32 = 64;

/// [`SIZE`] as the float the geometry below is written in.
const EDGE: f64 = SIZE as f64;

/// The default case shape: a [`SIZE`]x[`SIZE`] transparent-based frame at the
/// crate's tight default tolerance — mirrors [`super::unit`]'s own `case`
/// helper.
fn case(name: &'static str) -> CaseSpec {
    CaseSpec::new(name).with_size(SIZE, SIZE)
}

/// An opaque black backdrop filling the frame, matching [`super::unit`]'s own
/// helper of the same name — what makes a composite arithmetic expectation
/// exact rather than an observation.
fn black_backdrop(builder: &mut SceneBuilder<'_>) {
    builder.fill_rect(Rect::new(0.0, 0.0, EDGE, EDGE), Brush::Solid(BLACK));
}

/// Every case in the adversarial corpus.
#[must_use]
pub fn adversarial_cases() -> Vec<CorpusCase> {
    let mut cases = vec![
        degenerate_path(),
        nan_transform(),
        subpixel_rrect(),
        five_thousand_layers(),
        ten_thousand_glyphs(),
        huge_image(),
        clip_nest_eight(),
        destout_in_layer(),
        snapshot_scale_alpha(),
        empty_scene(),
        unbalanced_pops(),
        unbalanced_pop_in_snapshot(),
    ];
    cases.extend(one_px_divider_family());
    cases
}

/// `adv-degenerate-path`: zero-length, moveto-only, and no-segment-closed
/// subpaths, filled and stroked, alongside one ordinary subpath.
///
/// # Guards against
///
/// A flattener that dereferences a subpath's first *segment* instead of
/// checking for one first, or that divides by a zero-length tangent to
/// compute a join/cap, panics or NaNs out on exactly this input — reachable
/// any time a widget records an empty/degenerate path (an animated path
/// morph mid-collapse, a procedurally generated outline with a coincident
/// pair of points). The real subpath alongside them is the control: a
/// regression that makes the WHOLE path vanish (rather than just its
/// degenerate parts) must still fail this case.
fn degenerate_path() -> CorpusCase {
    fn record(scene: &mut Scene) {
        let mut path = BezPath::new();
        // An ordinary quadrilateral: the control that must keep filling.
        path.move_to((20.0, 40.0));
        path.line_to((44.0, 40.0));
        path.line_to((44.0, 60.0));
        path.line_to((20.0, 60.0));
        path.close_path();
        // Moveto-only: no segment at all in this subpath.
        path.move_to((50.0, 10.0));
        // Closed with nothing recorded between the move and the close.
        path.move_to((10.0, 10.0));
        path.close_path();
        // A zero-length "segment" (line back to its own start), then closed.
        path.move_to((30.0, 8.0));
        path.line_to((30.0, 8.0));
        path.close_path();

        let mut builder = SceneBuilder::new(scene);
        builder.fill_path(path, Brush::Solid(RED));

        // The same degenerate shapes again, STROKED: a stroker walks
        // segments (and computes joins/caps) differently from a filler, so
        // both need their own no-panic coverage.
        let mut stroked = BezPath::new();
        stroked.move_to((50.0, 50.0));
        stroked.move_to((15.0, 50.0));
        stroked.close_path();
        builder.stroke_path(stroked, 2.0, Brush::Solid(BLUE));
    }
    CorpusCase {
        spec: case("adv-degenerate-path"),
        record,
        probes: &[
            Probe {
                x: 32,
                y: 50,
                expect: Expect::Exact([255, 0, 0, 255]),
                why: "an ordinary subpath in the same path must still fill, alongside the \
                      degenerate ones",
            },
            Probe {
                x: 50,
                y: 10,
                expect: Expect::Exact([0, 0, 0, 0]),
                why: "a moveto-only subpath has no segments and must paint nothing",
            },
            Probe {
                x: 10,
                y: 10,
                expect: Expect::Exact([0, 0, 0, 0]),
                why: "a subpath closed with no segments between move and close must paint \
                      nothing",
            },
            Probe {
                x: 30,
                y: 8,
                expect: Expect::Exact([0, 0, 0, 0]),
                why: "a zero-length closed segment must paint nothing",
            },
        ],
        eroded_interior: false,
        about: "zero-length, moveto-only, and no-segment-closed subpaths, filled and stroked, \
                next to an ordinary subpath that must keep filling",
    }
}

/// `adv-nan-transform`: an `Affine` carrying a `NaN` component on a
/// `Command::FillRect`, and one carrying `+inf` on a `Command::GlyphRun`.
///
/// `no_ref`: the malformed content's own pixels are not a stable contract on
/// any backend — the only portable assertion is that rendering it does not
/// panic, and that ordinary content recorded immediately before and after it
/// is unaffected.
///
/// # Guards against
///
/// A rasterizer path that assumes a finite bounding box (to size a tile
/// buffer, to compute a stroke join) and panics or hangs on `NaN`/`inf`
/// instead of producing SOME output (garbage is acceptable; a crash is not).
/// `frust-render`'s own `compositor.rs`/`snapshot.rs` already guard several
/// `is_finite` boundaries for exactly this reason; this case is the one that
/// would catch a spot they missed.
fn nan_transform() -> CorpusCase {
    fn record(scene: &mut Scene) {
        let mut cx = TextContext::new();
        let registered = register_test_fonts(&mut cx);
        let style = TextStyle {
            family: FontFamily::stack(registered.iter().map(|f| f.name.clone())),
            ..TextStyle::new(16.0, BLACK)
        };
        // "H" rather than an arbitrary letter: it is in the bundled Latin
        // face's subset (`testing/fonts/LICENSES.md`), so this shapes against
        // the registered test font instead of falling back to a HOST font —
        // see `crate::frame::foreign_font_runs`, the byte-identity gate that
        // now runs over this corpus and would otherwise flag a runner-local
        // frame.
        let layout = cx.layout("H", &style, None);

        let mut builder = SceneBuilder::new(scene);
        // Control, recorded BEFORE any malformed transform: must be
        // unaffected by whatever happens next.
        builder.fill_rect(Rect::new(4.0, 4.0, 20.0, 20.0), Brush::Solid(GREEN));

        // A NaN component on a FillRect's transform.
        builder.push_transform(Affine::new([f64::NAN, 0.0, 0.0, 1.0, 0.0, 0.0]));
        builder.fill_rect(Rect::new(0.0, 0.0, EDGE, EDGE), Brush::Solid(RED));
        builder.pop_transform();

        // An infinite translation on a GlyphRun's transform.
        builder.push_transform(Affine::translate((f64::INFINITY, 0.0)));
        for run in layout.to_scene_runs(Point::new(30.0, 40.0)) {
            builder.draw_glyph_run(run);
        }
        builder.pop_transform();

        // Control, recorded AFTER: recording/rendering must resume normally.
        builder.fill_rect(Rect::new(44.0, 44.0, 60.0, 60.0), Brush::Solid(BLUE));
    }
    CorpusCase {
        spec: case("adv-nan-transform").with_no_ref(true),
        record,
        probes: &[
            Probe {
                x: 10,
                y: 10,
                expect: Expect::Exact([0, 128, 0, 255]),
                why: "content recorded before the NaN/inf transforms must be unaffected",
            },
            Probe {
                x: 52,
                y: 52,
                expect: Expect::Exact([0, 0, 255, 255]),
                why: "recording and rendering must resume normally after the malformed \
                      transforms are popped",
            },
        ],
        eroded_interior: false,
        about: "a NaN component on a FillRect transform and an infinite translation on a \
                GlyphRun transform must not panic; only the malformed content's own pixels are \
                unspecified",
    }
}

/// `adv-subpixel-rrect`: a rounded-rect radius under one device pixel, with
/// every edge at a half-pixel offset.
///
/// # Guards against
///
/// A radius clamp that rounds a sub-pixel value up to a visible corner cut
/// (drawing a corner nobody asked for) or down to zero in a way that also
/// zeroes an otherwise-valid edge, and a rasterizer that mishandles a corner
/// arc whose radius is smaller than its own antialiasing kernel. The deep
/// interior is the control: whatever the corners do, the middle of the shape
/// must stay solidly filled.
fn subpixel_rrect() -> CorpusCase {
    fn record(scene: &mut Scene) {
        let mut builder = SceneBuilder::new(scene);
        builder.fill_rounded_rect(Rect::new(8.5, 8.5, 55.5, 55.5), 0.5, Brush::Solid(RED));
    }
    CorpusCase {
        spec: case("adv-subpixel-rrect"),
        record,
        probes: &[
            Probe {
                x: 32,
                y: 32,
                expect: Expect::Exact([255, 0, 0, 255]),
                why: "the deep interior must stay solidly filled regardless of how a sub-pixel \
                      radius is rounded at the corners",
            },
            Probe {
                x: 2,
                y: 2,
                expect: Expect::Exact([0, 0, 0, 0]),
                why: "well outside the shape stays untouched",
            },
        ],
        eroded_interior: true,
        about: "a rounded-rect radius under one device pixel, edges at half-pixel offsets — \
                must neither cut a visible corner nor vanish",
    }
}

/// `adv-5k-layers`: 5,000 nested `PushLayer`/`PopLayer` pairs around one
/// fill.
///
/// `no_ref`: this case's own pixels are an ordinary nested-alpha composite
/// (`super::unit`'s `unit-layer-alpha` already pins that arithmetic at a
/// tractable depth) — what THIS case exists to pin is that depth alone does
/// not blow up memory or a fixed-size stack, which
/// `tests/adversarial.rs`'s `five_thousand_nested_layers_stay_within_a_bounded_memory_budget`
/// asserts directly via a counting-allocator/peak-RSS bound, not via a pixel
/// comparison.
///
/// # Guards against
///
/// A layer implementation that allocates one full-frame buffer per open
/// layer with no reuse (`O(depth)` buffers alive at once is expected and
/// bounded; `O(depth^2)` or unbounded growth is not), or a fixed-capacity
/// group stack that overflows past some hard-coded depth.
fn five_thousand_layers() -> CorpusCase {
    fn record(scene: &mut Scene) {
        const DEPTH: usize = 5_000;
        let mut builder = SceneBuilder::new(scene);
        black_backdrop(&mut builder);
        let full = Rect::new(0.0, 0.0, EDGE, EDGE);
        for _ in 0..DEPTH {
            builder.push_layer(full, 0.9999);
        }
        builder.fill_rect(full, Brush::Solid(WHITE));
        for _ in 0..DEPTH {
            builder.pop_layer();
        }
    }
    CorpusCase {
        spec: case("adv-5k-layers").with_no_ref(true),
        record,
        probes: &[],
        eroded_interior: false,
        about: "5,000 nested PushLayer/PopLayer must render within a bounded memory budget, \
                asserted separately via a peak-RSS check — not just \"did not panic\"",
    }
}

/// `adv-10k-glyphs`: one `GlyphRun` carrying 10,000 densely packed, heavily
/// overlapping glyphs from a bundled test font.
///
/// # Guards against
///
/// A glyph-batch path sized for an ordinary line of text (tens of glyphs)
/// that panics, allocates unreasonably, or effectively hangs once a run's
/// glyph count reaches a scale a long unbroken token (a URL, a hash, a
/// pathological layout result) can actually produce.
fn ten_thousand_glyphs() -> CorpusCase {
    fn record(scene: &mut Scene) {
        const COUNT: usize = 10_000;
        const COLUMNS: usize = 100;
        const STEP: f32 = 0.55;

        let mut cx = TextContext::new();
        let registered = register_test_fonts(&mut cx);
        let style = TextStyle {
            family: FontFamily::stack(registered.iter().map(|f| f.name.clone())),
            ..TextStyle::new(8.0, WHITE)
        };
        // "l" rather than an arbitrary letter: it is in the bundled Latin
        // face's subset (`testing/fonts/LICENSES.md`), so this shapes against
        // the registered test font instead of falling back to a HOST font —
        // see `crate::frame::foreign_font_runs`, the byte-identity gate that
        // now runs over this corpus and would otherwise flag a runner-local
        // frame. The glyph id extracted below is repeated 10,000 times, so
        // which bundled letter it comes from does not change what this case
        // exercises.
        let layout = cx.layout("l", &style, None);
        let seed = layout
            .to_scene_runs(Point::ORIGIN)
            .into_iter()
            .next()
            .expect("a bundled test font must shape at least one glyph run for a single letter");
        let glyph_id = seed.glyphs.first().map(|g| g.id).unwrap_or(0);

        let mut glyphs = Vec::with_capacity(COUNT);
        for i in 0..COUNT {
            let column = (i % COLUMNS) as f32;
            let row = (i / COLUMNS) as f32;
            glyphs.push(Glyph {
                id: glyph_id,
                x: column * STEP,
                y: row * STEP + 6.0,
            });
        }
        let run = GlyphRun {
            font: seed.font.clone(),
            font_size: 8.0,
            brush: Brush::Solid(WHITE),
            transform: Affine::IDENTITY,
            glyphs,
        };

        let mut builder = SceneBuilder::new(scene);
        black_backdrop(&mut builder);
        builder.draw_glyph_run(run);
    }
    CorpusCase {
        spec: case("adv-10k-glyphs"),
        record,
        probes: &[Probe {
            x: 32,
            y: 32,
            expect: Expect::AtLeast { channel: 3, min: 1 },
            why: "10,000 densely packed glyphs must still paint SOME coverage near the centre \
                  of the pack, not vanish or panic before reaching it",
        }],
        eroded_interior: true,
        about: "10,000 glyphs packed into one GlyphRun, exercising the glyph-batch path at a \
                stress-test scale ordinary text never reaches",
    }
}

/// The bytes of a `width`x`height` image, every pixel the same opaque
/// `color`, straight-alpha RGBA8.
///
/// A solid fill rather than a pattern: [`huge_image`]'s point is the source's
/// DIMENSIONS, not its content, and a uniform color downsamples to the exact
/// same color under any resampling filter, which is what makes this case's
/// probe an `Exact` rather than an `AtLeast`.
fn solid_image(width: u32, height: u32, color: [u8; 4]) -> ImageData {
    let mut data = vec![0_u8; (width as usize) * (height as usize) * 4];
    for pixel in data.chunks_exact_mut(4) {
        pixel.copy_from_slice(&color);
    }
    ImageData {
        data: Blob::from(data),
        format: ImageFormat::Rgba8,
        alpha_type: ImageAlphaType::Alpha,
        width,
        height,
    }
}

/// `adv-huge-image`: a 5000x5000 solid-color source image, drawn minified
/// into a small destination rect.
///
/// # Guards against
///
/// The common `4096` max-texture-dimension limit a GPU sampler enforces: a
/// backend that hands a source this large straight to a texture upload
/// without checking must fail or panic rather than silently truncating.
/// (The WebGL2 `2048` probe a later phase adds is a tighter version of the
/// same concern, not covered here.)
fn huge_image() -> CorpusCase {
    fn record(scene: &mut Scene) {
        const HUGE: u32 = 5_000;
        let data = solid_image(HUGE, HUGE, [255, 0, 255, 255]);
        let mut builder = SceneBuilder::new(scene);
        builder.draw_image(&data, Rect::new(8.0, 8.0, 56.0, 56.0));
    }
    CorpusCase {
        spec: case("adv-huge-image"),
        record,
        probes: &[
            Probe {
                x: 32,
                y: 32,
                expect: Expect::Exact([255, 0, 255, 255]),
                why: "a uniform-color source downsamples to the exact same color under any \
                      resampling filter, inside its dest rect",
            },
            Probe {
                x: 2,
                y: 2,
                expect: Expect::Exact([0, 0, 0, 0]),
                why: "outside the dest rect stays untouched",
            },
        ],
        eroded_interior: false,
        about: "a 5000x5000 source image (over the common 4096 max-texture-dimension limit) \
                must decode and minify into a small dest without panicking",
    }
}

/// `adv-clip-nest-8`: 8 nested clips alternating rectangular and rounded,
/// fully popped, then an unclipped root draw.
///
/// # Guards against
///
/// A clip-stack implementation with a fixed-capacity slot count (any depth
/// beyond it aliases or overflows), and — the [`super::unit`]
/// `clip_balance`/`layer_balance` concern at a depth deep enough that an
/// off-by-one in the pop-count bookkeeping only shows up past a handful of
/// levels.
fn clip_nest_eight() -> CorpusCase {
    fn record(scene: &mut Scene) {
        let mut builder = SceneBuilder::new(scene);
        for depth in 0..8_u32 {
            let inset = 3.0 * f64::from(depth);
            let rect = Rect::new(2.0 + inset, 2.0 + inset, 62.0 - inset, 62.0 - inset);
            if depth % 2 == 0 {
                builder.push_clip(rect);
            } else {
                builder.push_clip_rounded(rect, 4.0);
            }
        }
        builder.fill_rect(Rect::new(0.0, 0.0, EDGE, EDGE), Brush::Solid(RED));
        for _ in 0..8 {
            builder.pop_clip();
        }
        // Fully unwound: this draw is unclipped, at root.
        builder.fill_rect(Rect::new(0.0, 0.0, 4.0, 4.0), Brush::Solid(BLUE));
    }
    CorpusCase {
        spec: case("adv-clip-nest-8"),
        record,
        probes: &[
            Probe {
                x: 32,
                y: 32,
                expect: Expect::Exact([255, 0, 0, 255]),
                why: "the intersection of all 8 nested clips is a well-defined non-empty \
                      region at the centre, and it must be painted",
            },
            Probe {
                x: 63,
                y: 63,
                expect: Expect::Exact([0, 0, 0, 0]),
                why: "outside the OUTERMOST of the 8 clips must stay untouched",
            },
            Probe {
                x: 1,
                y: 1,
                expect: Expect::Exact([0, 0, 255, 255]),
                why: "a draw after all 8 pops must land fully opaque and unclipped at root",
            },
        ],
        eroded_interior: false,
        about: "8 clips nested alternating rect/rounded, fully popped: the deepest intersection \
                paints, outside the outermost stays untouched, and a post-unwind draw is \
                unclipped",
    }
}

/// `adv-destout-in-layer`: `Command::ClearRect` recorded inside a
/// TRANSLUCENT (alpha 0.5) `PushLayer` group, the [`super::unit`]
/// `unit-clear-rect` case's own promoted group used ALPHA 1.0.
///
/// # Guards against
///
/// A hoist-to-root implementation that scales the erase by the enclosing
/// group's own opacity instead of applying it fully before that opacity is
/// composited — leaving a 50%-erased pixel instead of a fully transparent
/// one, invisible at alpha 1.0 (`unit-clear-rect`'s own case) but immediate
/// at any alpha below it.
fn destout_in_layer() -> CorpusCase {
    fn record(scene: &mut Scene) {
        let mut builder = SceneBuilder::new(scene);
        // Opaque backdrop, OUTSIDE the group — what the punch must reach
        // through.
        builder.fill_rect(Rect::new(0.0, 0.0, EDGE, EDGE), Brush::Solid(RED));
        builder.push_layer(Rect::new(0.0, 0.0, EDGE, EDGE), 0.5);
        builder.clear_rect(Rect::new(0.0, 0.0, 32.0, EDGE));
        builder.pop_layer();
    }
    CorpusCase {
        spec: case("adv-destout-in-layer").with_tolerance(crate::case::Tolerance::exact()),
        record,
        probes: &[
            Probe {
                x: 8,
                y: 32,
                expect: Expect::Exact([0, 0, 0, 0]),
                why: "the erase must reach full transparency even though its enclosing layer \
                      is only 50% opaque",
            },
            Probe {
                x: 48,
                y: 32,
                expect: Expect::Exact([255, 0, 0, 255]),
                why: "backdrop stays untouched where the translucent group painted nothing, \
                      regardless of the group's own alpha",
            },
        ],
        eroded_interior: false,
        about: "Command::ClearRect inside a translucent (alpha 0.5) PushLayer group must still \
                erase fully to (0, 0, 0, 0), hoisted to root and not scaled by the group's own \
                opacity",
    }
}

/// `adv-snapshot-scale-alpha`: an outer `PushSnapshot` bracket combining
/// `alpha < 1.0` with a non-1.0 `scale`, nested one level deeper by an inner
/// bracket whose own (different) alpha/scale must be ignored.
///
/// The outer bracket's body fills its ENTIRE local rect before the scale
/// correction is applied — scaling a full-coverage shape up around its own
/// centre only ever grows its footprint, so the probes below hold regardless
/// of the exact scale/rect geometry (the same "assert the arithmetic, not
/// the edges" approach `super::unit`'s `unit-snapshot-bracket`/
/// `unit-snapshot-balance` already take).
///
/// # Guards against
///
/// [`super::unit`]'s `unit-snapshot-balance` already pins that an inner
/// bracket's own alpha/scale is ignored when the OUTER bracket is untouched
/// (scale 1.0). This case is the one that would catch a scale-correction
/// implementation that only special-cases the trivial (unscaled) outer
/// bracket and mishandles the general (scaled) one — e.g. composing the
/// inner correction on top instead of discarding it once a non-identity
/// outer correction is already active.
fn snapshot_scale_alpha() -> CorpusCase {
    fn record(scene: &mut Scene) {
        let overlay = Color::from_rgba8(255, 255, 0, 255);
        let bracket = Affine::translate((8.0, 8.0)) * Affine::scale(2.0);
        let rect = Rect::new(0.0, 0.0, 20.0, 20.0);

        let mut builder = SceneBuilder::new(scene);
        black_backdrop(&mut builder);
        builder.fill_rect(Rect::new(52.0, 52.0, 60.0, 60.0), Brush::Solid(BLUE));
        builder.push_transform(bracket);
        // Outer: alpha < 1.0 AND scale != 1.0.
        builder.push_snapshot(1, rect, 0.6, 1.3);
        builder.fill_rect(rect, Brush::Solid(RED));
        // Inner: its own alpha/scale must be ignored entirely.
        builder.push_snapshot(2, rect, 0.2, 5.0);
        builder.fill_rect(Rect::new(5.0, 5.0, 15.0, 15.0), Brush::Solid(GREEN));
        builder.pop_snapshot();
        builder.pop_snapshot();
        builder.pop_transform();
        // Recorded after the bracket: must land on top, unaffected by it.
        builder.fill_rect(Rect::new(20.0, 20.0, 30.0, 30.0), Brush::Solid(overlay));
    }
    CorpusCase {
        spec: case("adv-snapshot-scale-alpha")
            .with_base_color(BLACK)
            .with_tolerance(crate::case::Tolerance::new().with_channel(4).with_alpha(4)),
        record,
        probes: &[
            Probe {
                x: 10,
                y: 10,
                expect: Expect::Channel {
                    channel: 0,
                    value: 153,
                    tolerance: 4,
                },
                why: "the outer bracket's own alpha (0.6) applies over black to content that, \
                      once its own scale grows outward from the bracket's centre, still fully \
                      covers this point",
            },
            Probe {
                x: 35,
                y: 35,
                expect: Expect::Channel {
                    channel: 1,
                    value: 77,
                    tolerance: 4,
                },
                why: "the inner bracket's own alpha/scale must be ignored even though the \
                      OUTER bracket itself carries a non-1.0 scale — 128 * 0.6, not \
                      128 * 0.6 * 0.2",
            },
            Probe {
                x: 24,
                y: 24,
                expect: Expect::Exact([255, 255, 0, 255]),
                why: "content recorded after the bracket lands on top, unaffected by the scale \
                      correction",
            },
            Probe {
                x: 56,
                y: 56,
                expect: Expect::AtLeast {
                    channel: 2,
                    min: 201,
                },
                why: "content recorded before the bracket survives",
            },
        ],
        eroded_interior: false,
        about: "a snapshot bracket combining alpha < 1.0 with a non-1.0 scale at the OUTER \
                level, nested — only the outer bracket's parameters apply",
    }
}

/// `adv-empty-scene`: zero commands.
///
/// # Guards against
///
/// A renderer that assumes at least one draw call before it may present a
/// frame (a walk that indexes command `0` unconditionally, an accumulator
/// that never initializes its output buffer because nothing ever wrote to
/// it) — reachable any time a widget subtree paints nothing this frame (a
/// fully clipped-away region, a `Visibility::Hidden` root).
fn empty_scene() -> CorpusCase {
    fn record(_scene: &mut Scene) {
        // Deliberately empty — no `SceneBuilder` calls at all.
    }
    let base = Color::from_rgba8(20, 30, 40, 255);
    CorpusCase {
        spec: case("adv-empty-scene").with_base_color(base),
        record,
        probes: &[
            Probe {
                x: 0,
                y: 0,
                expect: Expect::Exact([20, 30, 40, 255]),
                why: "a scene with zero commands must still clear to its base colour",
            },
            Probe {
                x: SIZE - 1,
                y: SIZE - 1,
                expect: Expect::Exact([20, 30, 40, 255]),
                why: "...including the opposite corner",
            },
        ],
        eroded_interior: false,
        about: "a scene with zero commands must render as a plain base-colour clear, not panic \
                or produce garbage",
    }
}

/// `adv-unbalanced-pops`: one extra `PopClip`, `PopLayer`, and `PopSnapshot`
/// beyond a balanced nest of all three, each individually ignored per
/// `frust_scene::scene`'s documented policy (`PopClip`: scene.rs:210-211;
/// `PopLayer`: scene.rs:256-257; the unbalanced-`PopSnapshot` policy:
/// scene.rs:326-327).
///
/// # Guards against
///
/// [`super::unit`]'s `clip_balance`/`layer_balance` cases each cover one
/// stack in isolation; this case is the one that would catch a group-stack
/// implementation that only tracks ONE shared depth counter across
/// clip/layer/snapshot instead of the three independent ones the spec
/// requires — a bug that would only surface once more than one kind of
/// group is open and popped past balance at once.
fn unbalanced_pops() -> CorpusCase {
    fn record(scene: &mut Scene) {
        let mut builder = SceneBuilder::new(scene);
        black_backdrop(&mut builder);
        let rect = Rect::new(8.0, 8.0, 40.0, 40.0);
        builder.push_clip(rect);
        builder.push_layer(rect, 0.5);
        builder.push_snapshot(1, rect, 0.5, 2.0);
        builder.fill_rect(Rect::new(0.0, 0.0, EDGE, EDGE), Brush::Solid(GREEN));
        builder.pop_snapshot();
        builder.pop_layer();
        builder.pop_clip();
        // One extra, unbalanced pop of each kind: every one must be ignored.
        builder.pop_snapshot();
        builder.pop_layer();
        builder.pop_clip();
        builder.fill_rect(Rect::new(44.0, 44.0, 60.0, 60.0), Brush::Solid(BLUE));
    }
    CorpusCase {
        spec: case("adv-unbalanced-pops").with_base_color(BLACK),
        record,
        probes: &[Probe {
            x: 52,
            y: 52,
            expect: Expect::Exact([0, 0, 255, 255]),
            why: "a draw after one extra PopClip/PopLayer/PopSnapshot each must still land \
                  fully opaque, unclipped and unscaled at root",
        }],
        eroded_interior: false,
        about: "one extra PopClip, PopLayer, and PopSnapshot beyond a balanced nest of all \
                three at once, each individually documented as ignored (scene.rs:210-211, \
                :256-257, :326-327)",
    }
}

/// `adv-unbalanced-pop-in-snapshot`: an unbalanced `PopClip` and an unbalanced
/// `PopLayer`, each inside its own `alpha < 1.0` `PushSnapshot` bracket, plus
/// the original review counterexample (a `PopClip` immediately followed by
/// the bracket's own `PopSnapshot`).
///
/// `no_ref`: the malformed content's own pixels are not a stable contract —
/// see `adv-nan-transform`'s identical rationale — the only portable
/// assertion is that rendering does not panic and that ordinary content
/// recorded after the malformed brackets is unaffected.
///
/// # Guards against
///
/// [`crate::oracle_cpu::CpuOracle`]'s inline `PushSnapshot`/`PopSnapshot`
/// emulation tracks its own alpha layer via a `snapshot_layer_pushed` flag
/// separate from the shared clip/opacity `groups` stack `PopClip`/`PopLayer`
/// pop from. Before the fix this guards against, an unbalanced `PopClip` or
/// `PopLayer` recorded inside the bracket (both individually documented as
/// ignored-when-unmatched, `scene.rs`'s policy `adv-unbalanced-pops` already
/// covers standalone) consumed the snapshot's own `groups` entry AND popped
/// the one backend layer the bracket had pushed, while `snapshot_layer_pushed`
/// stayed `true` — so the bracket's own `PopSnapshot` popped a SECOND,
/// already-empty backend layer, underflowing `vello_cpu`'s layer stack and
/// panicking. The GPU (`vello`) arm survives the identical scene because
/// vello's resolver tolerates the imbalance the CPU rasterizer does not — an
/// oracle-only crash on a scene a `SceneBuilder` can actually produce.
fn unbalanced_pop_in_snapshot() -> CorpusCase {
    fn record(scene: &mut Scene) {
        let mut builder = SceneBuilder::new(scene);
        black_backdrop(&mut builder);
        let rect = Rect::new(8.0, 8.0, 40.0, 40.0);

        // The original review counterexample: an unbalanced PopClip
        // immediately consumes the bracket's own emulated alpha layer.
        builder.push_snapshot(1, rect, 0.5, 1.0);
        builder.pop_clip();
        builder.pop_snapshot();

        // The PopLayer-shaped variant of the same desync.
        builder.push_snapshot(2, rect, 0.5, 1.0);
        builder.pop_layer();
        builder.pop_snapshot();

        // Control, recorded after both malformed brackets: recording and
        // rendering must resume normally.
        builder.fill_rect(Rect::new(44.0, 44.0, 60.0, 60.0), Brush::Solid(BLUE));
    }
    CorpusCase {
        spec: case("adv-unbalanced-pop-in-snapshot").with_no_ref(true),
        record,
        probes: &[Probe {
            x: 52,
            y: 52,
            expect: Expect::Exact([0, 0, 255, 255]),
            why: "content recorded after an unbalanced PopClip and an unbalanced PopLayer, each \
                  inside its own alpha < 1.0 snapshot bracket, must still land fully opaque and \
                  unclipped at root",
        }],
        eroded_interior: false,
        about: "an unbalanced PopClip and an unbalanced PopLayer, each inside its own alpha < \
                1.0 PushSnapshot bracket — guards against the emulated snapshot layer's \
                bookkeeping desyncing from the backend layer stack and panicking on the \
                bracket's own PopSnapshot",
    }
}

/// The device-pixel canvas every [`one_px_divider_family`] case renders into
/// — fixed across the family so the same probe coordinates are comparable at
/// every scale (`RenderSpec::scale` multiplies recorded geometry, never the
/// output canvas's own device-pixel dimensions).
const DIVIDER_CANVAS: u32 = 32;

/// Records two vertical 1-device-px dividers under `scale`: one centred on
/// an INTEGER device x (straddling two pixel columns 50/50, per
/// `super::unit`'s `unit-fill-rect`/`unit-stroke-line` convention), one
/// centred on a HALF-pixel device x (aligning exactly to one pixel column's
/// edges, crisp).
///
/// Both are authored in LOCAL units — `1.0 / scale` wide, positioned at
/// `device_x / scale` — so that after `RenderSpec::scale` multiplies them
/// back up, they land at the intended device-pixel positions regardless of
/// which scale this is called under.
fn draw_one_px_dividers(builder: &mut SceneBuilder<'_>, scale: f64) {
    let width_local = 1.0 / scale;
    let canvas_local = f64::from(DIVIDER_CANVAS) / scale;
    let integer_x_local = 12.0 / scale;
    let half_x_local = 22.5 / scale;
    builder.stroke_line(
        Point::new(integer_x_local, 0.0),
        Point::new(integer_x_local, canvas_local),
        width_local,
        Brush::Solid(RED),
    );
    builder.stroke_line(
        Point::new(half_x_local, 0.0),
        Point::new(half_x_local, canvas_local),
        width_local,
        Brush::Solid(BLUE),
    );
}

/// `record` for the 1.0x member of [`one_px_divider_family`].
///
/// A named top-level `fn` rather than a closure: [`CorpusCase::record`] is a
/// plain `fn(&mut Scene)` pointer with no room to capture a per-case scale,
/// so each family member gets its own zero-argument wrapper around
/// [`draw_one_px_dividers`].
fn record_divider_1x(scene: &mut Scene) {
    let mut builder = SceneBuilder::new(scene);
    draw_one_px_dividers(&mut builder, 1.0);
}

/// `record` for the 2.0x member — see [`record_divider_1x`].
fn record_divider_2x(scene: &mut Scene) {
    let mut builder = SceneBuilder::new(scene);
    draw_one_px_dividers(&mut builder, 2.0);
}

/// `record` for the 2.75x member (the Pixel 5a's own device density) — see
/// [`record_divider_1x`].
fn record_divider_2_75x(scene: &mut Scene) {
    let mut builder = SceneBuilder::new(scene);
    draw_one_px_dividers(&mut builder, 2.75);
}

/// `adv-1px-divider-{1x,2x,2-75x}`: axis-aligned 1-device-px dividers at an
/// integer and a half-pixel device offset, at 1.0x, 2.0x, and 2.75x (the
/// Pixel 5a's density) device scale.
///
/// One `CaseSpec` cannot vary `scale` internally — it is a single field of
/// the deterministic-input contract every command in a case renders under —
/// so this is 3 named cases sharing one geometry helper rather than 1,
/// deliberately: exercising `RenderSpec::scale` itself (a fractional,
/// non-power-of-two factor is where rounding a hairline's local width back
/// up to a device pixel actually goes wrong) needs 3 distinct render passes,
/// not 3 regions of one frame.
///
/// # Guards against
///
/// A hairline width or position that rounds to ZERO device pixels once
/// divided by a fractional scale and re-multiplied (the divider vanishes
/// instead of degrading to partial coverage), and — at the half-pixel
/// offset specifically — a renderer that always centres a "1 unit" stroke on
/// its nominal coordinate regardless of the requested crisp alignment.
fn one_px_divider_family() -> Vec<CorpusCase> {
    /// A family member's golden name paired with its zero-argument `record`
    /// wrapper (see [`record_divider_1x`] for why one is needed per scale).
    type NamedRecord = (&'static str, fn(&mut Scene));

    let variants: [NamedRecord; 3] = [
        ("adv-1px-divider-1x", record_divider_1x as fn(&mut Scene)),
        ("adv-1px-divider-2x", record_divider_2x as fn(&mut Scene)),
        (
            "adv-1px-divider-2-75x",
            record_divider_2_75x as fn(&mut Scene),
        ),
    ];
    let scales = [1.0_f64, 2.0, 2.75];

    variants
        .into_iter()
        .zip(scales)
        .map(|((name, record), scale)| CorpusCase {
            spec: CaseSpec::new(name)
                .with_size(DIVIDER_CANVAS, DIVIDER_CANVAS)
                .with_scale(scale),
            record,
            probes: &[
                Probe {
                    x: 12,
                    y: 16,
                    expect: Expect::AtLeast { channel: 3, min: 1 },
                    why: "an integer-centred 1-device-px divider must paint SOME coverage at \
                          its column, not round away to nothing",
                },
                Probe {
                    x: 22,
                    y: 16,
                    expect: Expect::AtLeast {
                        channel: 3,
                        min: 250,
                    },
                    why: "a half-pixel-centred 1-device-px divider aligns exactly to one pixel \
                          column and must render at (near) full coverage, not antialiased away",
                },
                Probe {
                    x: 2,
                    y: 16,
                    expect: Expect::Exact([0, 0, 0, 0]),
                    why: "far from both dividers stays untouched",
                },
            ],
            eroded_interior: true,
            about: "a 1-device-px divider at an integer and a half-pixel device offset, at \
                    device scale — the hairline-vanishes-at-a-fractional-scale failure mode",
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn case_names_are_unique_and_adv_prefixed() {
        let cases = adversarial_cases();
        let names: BTreeSet<&str> = cases.iter().map(|case| case.spec.name).collect();
        assert_eq!(
            names.len(),
            cases.len(),
            "two adversarial cases share a name — the name is the golden artifact's file stem"
        );
        for case in &cases {
            assert!(
                case.spec.name.starts_with("adv-"),
                "`{}` is in the adversarial corpus but is not `adv-`-prefixed",
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
    fn exactly_the_thirteen_documented_families_are_present() {
        // 12 families were documented pre-p1-r1-02; the review-fix task added
        // `adv-unbalanced-pop-in-snapshot` as a 13th. The 1-px-divider family
        // is 3 CaseSpecs sharing one geometry helper (see
        // `one_px_divider_family`'s docs for why), so the corpus totals 15
        // CaseSpecs across 13 families.
        let cases = adversarial_cases();
        let mut families: BTreeSet<&str> = BTreeSet::new();
        for case in &cases {
            let family = case
                .spec
                .name
                .strip_prefix("adv-1px-divider")
                .map(|_| "adv-1px-divider")
                .unwrap_or(case.spec.name);
            families.insert(family);
        }
        assert_eq!(families.len(), 13, "{families:?}");
    }

    #[test]
    fn only_the_documented_cases_are_no_ref() {
        let no_ref: BTreeSet<&str> = adversarial_cases()
            .iter()
            .filter(|case| case.spec.no_ref)
            .map(|case| case.spec.name)
            .collect();
        assert_eq!(
            no_ref,
            BTreeSet::from([
                "adv-nan-transform",
                "adv-5k-layers",
                "adv-unbalanced-pop-in-snapshot"
            ]),
            "only the malformed-transform, the 5,000-layer depth, and the unbalanced-pop-in- \
             snapshot cases have no stable baseline to compare against"
        );
    }

    #[test]
    fn every_case_but_the_empty_scene_records_at_least_one_command() {
        for case in adversarial_cases() {
            if case.spec.name == "adv-empty-scene" {
                assert!(
                    case.scene().commands().is_empty(),
                    "`adv-empty-scene` must record zero commands"
                );
                continue;
            }
            assert!(
                !case.scene().commands().is_empty(),
                "`{}` records an empty scene",
                case.spec.name
            );
        }
    }

    #[test]
    fn the_divider_family_shares_geometry_across_all_three_scales() {
        let cases = adversarial_cases();
        let scales: Vec<f64> = cases
            .iter()
            .filter(|case| case.spec.name.starts_with("adv-1px-divider"))
            .map(|case| case.spec.scale)
            .collect();
        assert_eq!(scales, vec![1.0, 2.0, 2.75]);
    }
}
