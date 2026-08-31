//! Glyph-run lowering: what a run of text costs a frame, and what it refuses.
//!
//! Every case drives the public seam — a `frust_scene::Scene` recorded through
//! `SceneBuilder`, compiled by `SceneCompiler` — and reads the result back
//! through the frame's draws, its encoded-paint table and its own glyph
//! counters, so the assertions pin the lowering decisions a GPU pass would act
//! on rather than the backend's internals.
//!
//! No GPU, device or surface is involved, and no shaping engine either: the
//! glyph ids below are named directly against the bundled test faces in
//! `testing/fonts/`, which is what keeps these cases independent of parley and
//! of whatever fonts the host has installed. The *pixel* comparison against
//! the CPU reference renderer is a GPU-bound concern and lives with the
//! phase's golden corpus, not here — including the `unit-glyph-run` and
//! `adv-10k-glyphs` cases, both of which need the `frust-testing` corpus this
//! crate does not depend on. What is host-testable about text — one draw per
//! inked glyph, one encoded paint per run, dense painter-order depths, and a
//! refused frame for numbers that would not converge — is pinned below.
//! Colour glyphs have a file of their own, `text_color_hint.rs`; what is
//! pinned here is only that one does not disturb the run around it.

use std::sync::Arc;
use std::time::{Duration, Instant};

use frust_engine::cache::images::ImageResidency;
use frust_engine::compile::CompiledFrame;
use frust_engine::{AtlasBudget, EngineError, SceneCompiler};
use frust_scene::{FontHandle, Glyph, GlyphRun, Scene, SceneBuilder};
use kurbo::{Affine, Rect};
use peniko::color::palette::css::{BLUE, RED};
use peniko::color::{ColorSpaceTag, DynamicColor, HueDirection};
use peniko::{
    Blob, Brush, Color, ColorStop, ColorStops, FontData, Gradient, GradientKind,
    LinearGradientPosition,
};
use vello_common::paint::Paint;

/// Viewport every case compiles against. Deliberately not square, so an axis
/// swapped somewhere in the lowering cannot pass by symmetry.
const VIEWPORT: (u16, u16) = (128, 64);

/// Noto Sans, subsetted to Latin plus combining marks — the same bundled face
/// `frust-testing`'s deterministic text goldens shape against.
const LATIN_FONT: &[u8] = include_bytes!("../../../testing/fonts/NotoSans-Subset.ttf");

/// Noto Emoji, subsetted to one COLRv1 colour glyph.
const EMOJI_FONT: &[u8] = include_bytes!("../../../testing/fonts/NotoEmoji-COLRv1-Subset.ttf");

/// The glyph ids `HELLO_GLYPHS` is spelled from, read off [`LATIN_FONT`]'s own
/// character map: `H`, `e`, `l`, `o`. Every one of them carries an outline.
const H: u32 = 5;
const E: u32 = 6;
const L: u32 = 7;
const O: u32 = 8;

/// `Hello` — five inked glyphs, two of them the same.
const HELLO_GLYPHS: [u32; 5] = [H, E, L, L, O];

/// [`LATIN_FONT`]'s space (U+0020), the one mapped glyph in that face with no
/// outline at all. The negative control for "one draw per *inked* glyph".
const SPACE: u32 = 3;

/// [`EMOJI_FONT`]'s U+1F600, a COLRv1 base glyph with no `glyf` outline of its
/// own.
const EMOJI: u32 = 4;

/// Font size every case draws at unless it is varying it.
const FONT_SIZE: f32 = 24.0;

fn font(bytes: &'static [u8]) -> FontHandle {
    FontHandle::new(FontData::new(Blob::new(Arc::new(bytes)), 0))
}

/// A run of `ids` laid out on one baseline at `advance` pixels apart,
/// positioned by `transform`.
fn run(handle: FontHandle, ids: &[u32], advance: f32, brush: Brush, transform: Affine) -> GlyphRun {
    GlyphRun {
        font: handle,
        font_size: FONT_SIZE,
        brush,
        transform,
        glyphs: ids
            .iter()
            .enumerate()
            .map(|(index, id)| Glyph {
                id: *id,
                x: index as f32 * advance,
                y: 0.0,
            })
            .collect(),
    }
}

/// `Hello` in `brush`, sitting inside the viewport on its baseline.
fn hello(brush: Brush) -> GlyphRun {
    run(
        font(LATIN_FONT),
        &HELLO_GLYPHS,
        18.0,
        brush,
        Affine::translate((8.0, 44.0)),
    )
}

/// A scene built by `record`, ready to compile.
fn scene_of(record: impl FnOnce(&mut SceneBuilder<'_>)) -> Scene {
    let mut scene = Scene::new();
    let mut builder = SceneBuilder::new(&mut scene);
    record(&mut builder);
    scene
}

/// A compiler drawing every glyph as outline strips.
///
/// The glyph atlas is off on purpose: what this file pins is the *outline*
/// lowering — one draw per inked glyph, the run's brush inline, dense
/// painter-order depths — which is a live path in its own right (an animating
/// size, a size past the cache ceiling, a transform `glifo` will not cache, a
/// full atlas, and `FRUST_ENGINE_NO_ATLAS` all take it) and the fallback every
/// atlas refusal lands on. Turning the atlas on would make every one of these
/// runs an image draw sampling a slot, which is a different contract and is
/// pinned as one in `atlas_churn.rs`.
fn compiler() -> SceneCompiler {
    let mut compiler = SceneCompiler::new(VIEWPORT.0, VIEWPORT.1);
    compiler.set_image_residency(ImageResidency::disabled(AtlasBudget::MOBILE));
    compiler
}

fn compile(scene: &Scene) -> CompiledFrame {
    compiler()
        .compile(scene, Affine::IDENTITY, VIEWPORT)
        .expect("an in-range scene compiles")
}

/// The frame a single glyph run produces, with nothing else recorded.
fn compile_run(glyph_run: GlyphRun) -> CompiledFrame {
    compile(&scene_of(|builder| builder.draw_glyph_run(glyph_run)))
}

/// A three-stop linear gradient spanning the run's own width.
fn gradient_brush() -> Brush {
    Brush::Gradient(Gradient {
        kind: GradientKind::Linear(LinearGradientPosition {
            start: kurbo::Point::new(0.0, 0.0),
            end: kurbo::Point::new(96.0, 0.0),
        }),
        stops: ColorStops(
            vec![
                ColorStop {
                    offset: 0.0,
                    color: DynamicColor::from_alpha_color(RED),
                },
                ColorStop {
                    offset: 1.0,
                    color: DynamicColor::from_alpha_color(BLUE),
                },
            ]
            .into(),
        ),
        interpolation_cs: ColorSpaceTag::Srgb,
        hue_direction: HueDirection::Shorter,
        ..Default::default()
    })
}

// ---------------------------------------------------------------------
// One draw per inked glyph
// ---------------------------------------------------------------------

#[test]
fn every_inked_glyph_of_a_run_becomes_one_draw() {
    let frame = compile_run(hello(Brush::Solid(RED)));

    assert_eq!(
        frame.glyph_draws,
        HELLO_GLYPHS.len() as u32,
        "each of the five outlines in `Hello` paints once"
    );
    assert_eq!(frame.draws().len(), HELLO_GLYPHS.len());
    assert_eq!(frame.skipped_glyphs, 0, "an outline face refuses nothing");
    assert!(
        !frame.strip_buf().is_empty(),
        "a drawn glyph carries real coverage"
    );
}

#[test]
fn a_glyph_with_no_outline_records_no_draw() {
    let frame = compile_run(run(
        font(LATIN_FONT),
        &[SPACE, SPACE],
        18.0,
        Brush::Solid(RED),
        Affine::translate((8.0, 44.0)),
    ));

    assert_eq!(frame.glyph_draws, 0, "a space has no ink to paint");
    assert!(frame.draws().is_empty());
    assert_eq!(
        frame.skipped_glyphs, 0,
        "an empty outline is drawn and found empty, not refused"
    );
    assert!(frame.strip_buf().is_empty());
}

#[test]
fn an_empty_run_records_nothing() {
    let frame = compile_run(run(
        font(LATIN_FONT),
        &[],
        0.0,
        Brush::Solid(RED),
        Affine::IDENTITY,
    ));

    assert!(frame.draws().is_empty());
    assert_eq!(frame.glyph_draws, 0);
    assert!(frame.encoded_paints.is_empty(), "and encodes no paint");
}

#[test]
fn a_run_placed_outside_the_viewport_records_nothing() {
    let frame = compile_run(run(
        font(LATIN_FONT),
        &HELLO_GLYPHS,
        18.0,
        Brush::Solid(RED),
        Affine::translate((4000.0, 4000.0)),
    ));

    assert!(frame.draws().is_empty(), "every glyph culls away");
    assert_eq!(frame.glyph_draws, 0);
}

#[test]
fn a_run_under_a_clip_that_admits_nothing_records_nothing() {
    let frame = compile(&scene_of(|builder| {
        builder.push_clip(Rect::ZERO);
        builder.draw_glyph_run(hello(Brush::Solid(RED)));
        builder.pop_clip();
    }));

    assert!(frame.draws().is_empty());
    assert_eq!(frame.glyph_draws, 0);
    assert!(
        frame.encoded_paints.is_empty(),
        "a run that cannot draw leaves no orphan paint entry behind"
    );
}

#[test]
fn a_clip_trims_a_run_to_its_own_rectangle() {
    let unclipped = compile_run(hello(Brush::Solid(RED)));
    let clipped = compile(&scene_of(|builder| {
        // Whole pixels and axis-aligned, so the clip scissors rather than
        // masking: the trimming below is the scissor's, applied to the glyph
        // coverage after it was generated.
        builder.push_clip(Rect::new(0.0, 0.0, 40.0, 64.0));
        builder.draw_glyph_run(hello(Brush::Solid(RED)));
        builder.pop_clip();
    }));

    assert_eq!(clipped.scissor_clips, 1, "the clip lowered to a scissor");
    assert_eq!(clipped.mask_clips, 0);
    assert!(
        clipped.glyph_draws < unclipped.glyph_draws,
        "the clip cuts the run's trailing glyphs away entirely \
         ({} of {} survive)",
        clipped.glyph_draws,
        unclipped.glyph_draws
    );
    assert!(clipped.glyph_draws > 0, "and keeps its leading ones");
}

// ---------------------------------------------------------------------
// One paint per run
// ---------------------------------------------------------------------

#[test]
fn a_solid_run_paints_every_glyph_with_one_inline_colour() {
    let frame = compile_run(hello(Brush::Solid(RED)));

    assert!(
        frame.encoded_paints.is_empty(),
        "a solid colour travels inside the paint and costs no side-table entry"
    );
    assert!(frame.lut_requests.is_empty());
    for draw in frame.draws() {
        assert!(
            matches!(draw.paint, Paint::Solid(_)),
            "every glyph of the run carries the run's own colour"
        );
    }
}

#[test]
fn a_gradient_brushed_run_encodes_one_entry_and_one_ramp_for_the_whole_run() {
    let frame = compile_run(hello(gradient_brush()));

    assert_eq!(
        frame.encoded_paints.len(),
        1,
        "the run's brush is encoded once, not once per glyph"
    );
    assert_eq!(
        frame.lut_requests.len(),
        1,
        "and asks for exactly one colour ramp"
    );
    assert_eq!(frame.glyph_draws, HELLO_GLYPHS.len() as u32);

    let indices: Vec<usize> = frame
        .draws()
        .iter()
        .map(|draw| match &draw.paint {
            Paint::Indexed(indexed) => indexed.index(),
            Paint::Solid(_) => panic!("a gradient-brushed glyph paints from the side table"),
        })
        .collect();
    assert!(
        indices.iter().all(|index| *index == 0),
        "every glyph references the same encoded entry: {indices:?}"
    );
}

// ---------------------------------------------------------------------
// Painter order
// ---------------------------------------------------------------------

#[test]
fn glyph_depths_are_dense_and_run_in_painter_order() {
    let frame = compile(&scene_of(|builder| {
        builder.fill_rect(Rect::new(0.0, 0.0, 128.0, 64.0), Brush::Solid(BLUE));
        builder.draw_glyph_run(hello(Brush::Solid(RED)));
    }));

    let depths: Vec<u32> = frame.draws().iter().map(|draw| draw.depth).collect();
    let expected: Vec<u32> = (0..depths.len() as u32).collect();
    assert_eq!(
        depths, expected,
        "the background takes depth 0 and each glyph the next, with no gap \
         left by a glyph that drew nothing"
    );
}

// ---------------------------------------------------------------------
// Refusals: numbers, and glyphs the engine cannot paint
// ---------------------------------------------------------------------

#[test]
fn a_non_finite_font_size_refuses_the_frame() {
    let mut glyph_run = hello(Brush::Solid(RED));
    glyph_run.font_size = f32::NAN;
    let scene = scene_of(|builder| builder.draw_glyph_run(glyph_run));

    assert!(matches!(
        compiler().compile(&scene, Affine::IDENTITY, VIEWPORT),
        Err(EngineError::InvalidGeometry)
    ));
}

#[test]
fn a_non_finite_glyph_position_refuses_the_frame() {
    let mut glyph_run = hello(Brush::Solid(RED));
    if let Some(glyph) = glyph_run.glyphs.get_mut(2) {
        glyph.x = f32::INFINITY;
    }
    let scene = scene_of(|builder| builder.draw_glyph_run(glyph_run));

    assert!(matches!(
        compiler().compile(&scene, Affine::IDENTITY, VIEWPORT),
        Err(EngineError::InvalidGeometry)
    ));
}

#[test]
fn a_non_finite_run_transform_refuses_the_frame() {
    let glyph_run = run(
        font(LATIN_FONT),
        &HELLO_GLYPHS,
        18.0,
        Brush::Solid(RED),
        Affine::scale(f64::NAN),
    );
    let scene = scene_of(|builder| builder.draw_glyph_run(glyph_run));

    assert!(matches!(
        compiler().compile(&scene, Affine::IDENTITY, VIEWPORT),
        Err(EngineError::InvalidTransform)
    ));
}

#[test]
fn a_colour_glyph_paints_its_own_layers_rather_than_the_runs_brush() {
    // The tripwire only; `text_color_hint.rs` pins what those layers are, what
    // they paint with, and what a colour glyph the engine cannot express does
    // instead.
    let frame = compile_run(run(
        font(EMOJI_FONT),
        &[EMOJI],
        32.0,
        Brush::Solid(RED),
        Affine::translate((8.0, 44.0)),
    ));

    assert_eq!(frame.glyph_draws, 1, "a COLR glyph is drawn, not refused");
    assert_eq!(frame.skipped_glyphs, 0);
    assert!(
        frame.draws().len() > 1,
        "and costs one draw per colour layer"
    );
    assert!(!frame.strip_buf().is_empty());
}

#[test]
fn a_colour_glyph_leaves_the_frames_own_clip_state_untouched() {
    // The COLR lowering brackets its layers in clip pushes and pops of its
    // own. If those reached the compiler's clip stack, the outline glyphs
    // recorded after the emoji would be drawn under a clip nothing closed.
    let outlines_only = compile_run(hello(Brush::Solid(RED)));
    let after_emoji = compile(&scene_of(|builder| {
        builder.draw_glyph_run(run(
            font(EMOJI_FONT),
            &[EMOJI],
            32.0,
            Brush::Solid(RED),
            Affine::translate((96.0, 44.0)),
        ));
        builder.draw_glyph_run(hello(Brush::Solid(RED)));
    }));

    assert_eq!(
        after_emoji.glyph_draws,
        outlines_only.glyph_draws + 1,
        "every outline glyph after a colour glyph still paints, and the \
         colour glyph itself counts once"
    );
    assert_eq!(
        after_emoji.scissor_clips, 0,
        "and the colour glyph pushed no clip of the compiler's"
    );
    assert_eq!(after_emoji.mask_clips, 0);
}

// ---------------------------------------------------------------------
// The font gate
// ---------------------------------------------------------------------

/// A run of two glyphs against `bytes` read as a font face at `index`.
fn run_against(bytes: Vec<u8>, index: u32) -> GlyphRun {
    GlyphRun {
        font: FontHandle::new(FontData::new(Blob::from(bytes), index)),
        font_size: FONT_SIZE,
        brush: Brush::Solid(RED),
        transform: Affine::translate((8.0, 44.0)),
        glyphs: vec![
            Glyph {
                id: H,
                x: 0.0,
                y: 0.0,
            },
            Glyph {
                id: E,
                x: 18.0,
                y: 0.0,
            },
        ],
    }
}

/// A blob that is longer than a table directory and carries a plausible file
/// tag, but names tables that are not there. The parser this gate stands in
/// front of accepts blobs on much weaker evidence than "is a font", so the
/// gate has to reach the `head` table to be worth anything.
fn plausible_but_empty_face() -> Vec<u8> {
    let mut bytes = vec![0x00, 0x01, 0x00, 0x00];
    // numTables = 0, then searchRange/entrySelector/rangeShift.
    bytes.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0]);
    bytes
}

#[test]
fn a_font_blob_that_is_not_a_face_skips_its_run_instead_of_panicking() {
    for (label, bytes, index) in [
        ("an unloaded font resource", Vec::new(), 0),
        ("a truncated read", vec![1_u8, 2, 3, 4], 0),
        ("a face with no head table", plausible_but_empty_face(), 0),
        (
            "a collection index no collection names",
            LATIN_FONT.to_vec(),
            7,
        ),
    ] {
        let glyph_run = run_against(bytes, index);
        let glyphs = glyph_run.glyphs.len() as u32;
        let frame = compile_run(glyph_run);

        assert_eq!(frame.glyph_draws, 0, "{label} paints nothing");
        assert_eq!(
            frame.skipped_glyphs, glyphs,
            "{label} counts its whole run as skipped"
        );
        assert!(
            frame.encoded_paints.is_empty(),
            "{label} is refused before its brush is encoded"
        );
        assert!(frame.draws().is_empty(), "{label} records no draw");
    }
}

#[test]
fn the_font_gate_still_admits_the_bundled_faces() {
    // The negative controls above are only meaningful next to a positive one:
    // a gate that refused everything would pass all of them.
    assert!(compile_run(run_against(LATIN_FONT.to_vec(), 0)).glyph_draws > 0);
}

// ---------------------------------------------------------------------
// The retained caches
// ---------------------------------------------------------------------

#[test]
fn recompiling_the_same_run_against_a_warm_outline_cache_is_byte_identical() {
    let scene = scene_of(|builder| builder.draw_glyph_run(hello(Brush::Solid(RED))));
    let mut compiler = compiler();

    let cold = compiler
        .compile(&scene, Affine::IDENTITY, VIEWPORT)
        .expect("the first frame compiles");
    let warm = compiler
        .compile(&scene, Affine::IDENTITY, VIEWPORT)
        .expect("the second frame compiles");

    assert_eq!(
        cold.strip_buf(),
        warm.strip_buf(),
        "a cached outline rasterizes to the strips its cold fetch did"
    );
    assert_eq!(cold.alphas(), warm.alphas());
    assert_eq!(cold.glyph_draws, warm.glyph_draws);
}

#[test]
fn a_larger_font_size_is_a_distinct_cache_entry_and_paints_more_coverage() {
    let mut compiler = compiler();
    let mut small = hello(Brush::Solid(RED));
    small.font_size = 12.0;
    let mut large = hello(Brush::Solid(RED));
    large.font_size = 36.0;

    let small_frame = compiler
        .compile(
            &scene_of(|builder| builder.draw_glyph_run(small)),
            Affine::IDENTITY,
            VIEWPORT,
        )
        .expect("the small run compiles");
    let small_alphas = small_frame.alphas().len();

    let large_frame = compiler
        .compile(
            &scene_of(|builder| builder.draw_glyph_run(large)),
            Affine::IDENTITY,
            VIEWPORT,
        )
        .expect("the large run compiles");

    assert_eq!(small_frame.glyph_draws, large_frame.glyph_draws);
    assert!(
        large_frame.alphas().len() > small_alphas,
        "the larger run covers more pixels ({} vs {}) — the size is part of \
         the outline cache key, not something a warm entry ignores",
        large_frame.alphas().len(),
        small_alphas
    );
}

// ---------------------------------------------------------------------
// The adversarial run
// ---------------------------------------------------------------------

/// Densely packed glyphs, at the scale a long unbroken token reaches.
const MANY: usize = 10_000;

/// A ceiling on the whole compile, not a performance figure.
///
/// The point of the case is that a run's work stays bounded by its glyph count
/// — no unbounded flattening, no per-glyph font-table re-parse, no cache that
/// grows without ageing — so the bound sits far above any plausible healthy
/// time (this compile measures around 0.12 s in the same unoptimized test
/// profile) and is a tripwire for work that is not bounded at all. A real
/// timing lives in the crate's benches, under a release profile this test
/// target does not have.
const MANY_BUDGET: Duration = Duration::from_secs(10);

#[test]
fn ten_thousand_glyphs_compile_without_panicking_and_within_a_bound() {
    let glyphs: Vec<Glyph> = (0..MANY)
        .map(|index| Glyph {
            id: O,
            x: (index % 120) as f32,
            y: (index / 120 % 60) as f32,
        })
        .collect();
    let glyph_run = GlyphRun {
        font: font(LATIN_FONT),
        font_size: 14.0,
        brush: Brush::Solid(Color::from_rgb8(0x20, 0x20, 0x20)),
        transform: Affine::IDENTITY,
        glyphs,
    };

    let scene = scene_of(|builder| builder.draw_glyph_run(glyph_run));
    let started = Instant::now();
    let frame = compile(&scene);
    let elapsed = started.elapsed();

    assert_eq!(
        frame.glyph_draws, MANY as u32,
        "every glyph of the run paints"
    );
    assert_eq!(frame.skipped_glyphs, 0);
    assert_eq!(
        frame.encoded_paints.len(),
        0,
        "and all ten thousand share the run's one inline colour"
    );
    assert!(
        elapsed < MANY_BUDGET,
        "compiling {MANY} glyphs took {elapsed:?}, past the {MANY_BUDGET:?} bound"
    );
}
