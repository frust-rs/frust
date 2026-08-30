//! Colour (COLR) glyphs: what a colour glyph costs a frame, and what it
//! refuses.
//!
//! Sibling of `text_basic.rs` and driven the same way — a `frust_scene::Scene`
//! recorded through `SceneBuilder`, compiled by `SceneCompiler`, read back
//! through the frame's draws, its encoded-paint table and its own glyph
//! counters — so every assertion pins a lowering decision a GPU pass would act
//! on rather than the backend's internals. No GPU, device or surface is
//! involved, and no shaping engine either: glyph ids are named directly
//! against the bundled faces in `testing/fonts/`.
//!
//! The pixel comparison against the CPU reference renderer (which shares
//! `glifo`, so a colour glyph should land near-exactly) is a GPU-bound concern
//! and lives with the phase's golden corpus, not here. What is host-testable
//! about a colour glyph — that its layers paint their *own* palette colours
//! and gradients rather than the run's brush, that they cost the compiler no
//! clip, that they take dense painter-order depths, that they cull and clip
//! like any other draw, and that a warm cache re-compiles them byte-identically
//! — is pinned below.

use std::collections::BTreeSet;
use std::sync::Arc;

use frust_engine::SceneCompiler;
use frust_engine::compile::CompiledFrame;
use frust_scene::{FontHandle, Glyph, GlyphRun, Scene, SceneBuilder};
use kurbo::{Affine, Rect};
use peniko::color::palette::css::RED;
use peniko::{Blob, Brush, FontData};
use vello_common::paint::{Paint, PremulColor};

/// Viewport every case compiles against, matching `text_basic.rs`:
/// deliberately not square, so an axis swapped somewhere in the lowering
/// cannot pass by symmetry.
const VIEWPORT: (u16, u16) = (128, 64);

/// Noto Emoji, subsetted to one COLRv1 colour glyph plus a handful of plain
/// outlines.
const EMOJI_FONT: &[u8] = include_bytes!("../../../testing/fonts/NotoEmoji-COLRv1-Subset.ttf");

/// Noto Sans Arabic, subsetted to the joining forms of `مرحبا`.
const ARABIC_FONT: &[u8] = include_bytes!("../../../testing/fonts/NotoSansArabic-Subset.ttf");

/// Noto Sans JP, subsetted to `日本語`.
const CJK_FONT: &[u8] = include_bytes!("../../../testing/fonts/NotoSansJP-Subset.otf");

/// [`EMOJI_FONT`]'s U+1F600, a COLRv1 base glyph with no `glyf` outline of its
/// own. Its paint graph is a gradient layer followed by nine solid ones.
const EMOJI: u32 = 4;

/// A plain outline glyph of [`EMOJI_FONT`] — the control for "a colour glyph
/// does not change what the rest of its run paints with".
const EMOJI_OUTLINE: u32 = 5;

/// Inked glyphs of [`ARABIC_FONT`], read off the subset's own glyph order.
const ARABIC_GLYPHS: [u32; 5] = [4, 5, 6, 7, 8];

/// Inked glyphs of [`CJK_FONT`], read off the subset's own glyph order.
const CJK_GLYPHS: [u32; 5] = [4, 5, 6, 7, 8];

/// Font size the colour cases draw at: large enough that every layer of the
/// emoji covers real pixels, small enough to sit inside [`VIEWPORT`].
const EMOJI_SIZE: f32 = 32.0;

fn font(bytes: &'static [u8]) -> FontHandle {
    FontHandle::new(FontData::new(Blob::new(Arc::new(bytes)), 0))
}

/// A run of `ids` on one baseline at `advance` pixels apart, positioned by
/// `transform`.
fn run(
    bytes: &'static [u8],
    ids: &[u32],
    font_size: f32,
    advance: f32,
    transform: Affine,
) -> GlyphRun {
    GlyphRun {
        font: font(bytes),
        font_size,
        brush: Brush::Solid(RED),
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

/// The one colour glyph, sitting inside the viewport on its baseline.
fn emoji() -> GlyphRun {
    run(
        EMOJI_FONT,
        &[EMOJI],
        EMOJI_SIZE,
        0.0,
        Affine::translate((8.0, 44.0)),
    )
}

fn scene_of(record: impl FnOnce(&mut SceneBuilder<'_>)) -> Scene {
    let mut scene = Scene::new();
    let mut builder = SceneBuilder::new(&mut scene);
    record(&mut builder);
    scene
}

fn compiler() -> SceneCompiler {
    SceneCompiler::new(VIEWPORT.0, VIEWPORT.1)
}

fn compile(scene: &Scene) -> CompiledFrame {
    compiler()
        .compile(scene, Affine::IDENTITY, VIEWPORT)
        .expect("an in-range scene compiles")
}

fn compile_run(glyph_run: GlyphRun) -> CompiledFrame {
    compile(&scene_of(|builder| builder.draw_glyph_run(glyph_run)))
}

// ---------------------------------------------------------------------
// A colour glyph paints, and paints its own colours
// ---------------------------------------------------------------------

#[test]
fn a_colour_glyph_paints_one_draw_per_layer_and_counts_as_one_glyph() {
    let frame = compile_run(emoji());

    assert_eq!(
        frame.skipped_glyphs, 0,
        "a COLRv1 glyph is drawn, not refused"
    );
    assert_eq!(
        frame.glyph_draws, 1,
        "and is counted once, however many layers it took"
    );
    assert!(
        frame.draws().len() > 1,
        "a colour glyph costs one draw per layer ({} recorded)",
        frame.draws().len()
    );
    assert!(
        !frame.strip_buf().is_empty(),
        "and every layer carries real coverage"
    );
}

#[test]
fn a_colour_glyphs_layers_paint_their_own_colours_not_the_runs_brush() {
    let frame = compile_run(emoji());

    let solids: Vec<u32> = frame
        .draws()
        .iter()
        .filter_map(|draw| match &draw.paint {
            Paint::Solid(color) => Some(color.as_premul_rgba8().to_u32()),
            Paint::Indexed(_) => None,
        })
        .collect();
    let run_brush = PremulColor::from_alpha_color(RED)
        .as_premul_rgba8()
        .to_u32();

    assert!(
        !solids.is_empty(),
        "the glyph's palette layers paint solid colours"
    );
    assert!(
        solids.iter().all(|color| *color != run_brush),
        "and none of them is the run's own brush: {solids:x?}"
    );
    assert!(
        solids.iter().collect::<BTreeSet<_>>().len() > 1,
        "the palette is read per layer, not once for the glyph: {solids:x?}"
    );
}

#[test]
fn a_colour_glyphs_gradient_layer_encodes_one_entry_and_asks_for_one_ramp() {
    let frame = compile_run(emoji());

    assert_eq!(
        frame.encoded_paints.len(),
        1,
        "this glyph's one gradient layer is the only side-table entry it needs"
    );
    assert_eq!(
        frame.lut_requests.len(),
        1,
        "and it asks for exactly one colour ramp"
    );
    assert!(
        frame
            .draws()
            .iter()
            .any(|draw| matches!(draw.paint, Paint::Indexed(_))),
        "a gradient layer paints from the side table"
    );
}

#[test]
fn a_colour_glyph_leaves_the_rest_of_its_run_painting_the_runs_brush() {
    let frame = compile_run(run(
        EMOJI_FONT,
        &[EMOJI, EMOJI_OUTLINE],
        EMOJI_SIZE,
        40.0,
        Affine::translate((8.0, 44.0)),
    ));

    assert_eq!(frame.glyph_draws, 2, "both glyphs paint");
    assert_eq!(frame.skipped_glyphs, 0);

    let last = frame.draws().last().expect("the run recorded draws");
    assert_eq!(
        last.paint,
        Paint::Solid(PremulColor::from_alpha_color(RED)),
        "the outline glyph after the colour one still carries the run's brush"
    );
}

// ---------------------------------------------------------------------
// What a colour glyph costs the compiler
// ---------------------------------------------------------------------

#[test]
fn a_colour_glyph_costs_the_compiler_no_clip_at_all() {
    // Its layers are drawn as their own outlines rather than as the glyph's
    // bounding rectangle behind a clip, so nothing reaches the clip stack —
    // and an outline glyph recorded after one is not drawn under a clip
    // nothing closed.
    let frame = compile(&scene_of(|builder| {
        builder.draw_glyph_run(emoji());
        builder.draw_glyph_run(run(
            EMOJI_FONT,
            &[EMOJI_OUTLINE],
            EMOJI_SIZE,
            0.0,
            Affine::translate((80.0, 44.0)),
        ));
    }));

    assert_eq!(frame.scissor_clips, 0);
    assert_eq!(frame.mask_clips, 0);
    assert_eq!(frame.clip_mask_strips, 0);
    assert_eq!(frame.glyph_draws, 2, "and both glyphs still paint");
}

#[test]
fn colour_layers_take_dense_painter_order_depths() {
    let frame = compile(&scene_of(|builder| {
        builder.fill_rect(Rect::new(0.0, 0.0, 128.0, 64.0), Brush::Solid(RED));
        builder.draw_glyph_run(emoji());
    }));

    let depths: Vec<u32> = frame.draws().iter().map(|draw| draw.depth).collect();
    let expected: Vec<u32> = (0..depths.len() as u32).collect();
    assert_eq!(
        depths, expected,
        "the background takes depth 0 and each layer the next, with no gap \
         left by a layer that drew nothing"
    );
}

// ---------------------------------------------------------------------
// Culling and clipping
// ---------------------------------------------------------------------

#[test]
fn a_colour_glyph_placed_outside_the_viewport_records_nothing() {
    let frame = compile_run(run(
        EMOJI_FONT,
        &[EMOJI],
        EMOJI_SIZE,
        0.0,
        Affine::translate((4000.0, 4000.0)),
    ));

    assert!(frame.draws().is_empty(), "every layer culls away");
    assert_eq!(
        frame.glyph_draws, 0,
        "a glyph that painted nothing is not counted as drawn"
    );
    assert_eq!(
        frame.skipped_glyphs, 0,
        "nor as refused — it was drawn and found empty"
    );
}

#[test]
fn a_colour_glyph_under_a_clip_that_admits_nothing_records_nothing() {
    let frame = compile(&scene_of(|builder| {
        builder.push_clip(Rect::ZERO);
        builder.draw_glyph_run(emoji());
        builder.pop_clip();
    }));

    assert!(frame.draws().is_empty());
    assert_eq!(frame.glyph_draws, 0);
    assert!(
        frame.encoded_paints.is_empty(),
        "and no layer leaves an orphan gradient entry behind"
    );
    assert!(frame.lut_requests.is_empty());
}

#[test]
fn a_clip_trims_a_colour_glyph_to_its_own_rectangle() {
    let unclipped = compile_run(emoji());
    let clipped = compile(&scene_of(|builder| {
        // Whole pixels and axis-aligned, so the clip scissors; the trimming is
        // applied to each layer's coverage after it was generated.
        builder.push_clip(Rect::new(0.0, 0.0, 20.0, 64.0));
        builder.draw_glyph_run(emoji());
        builder.pop_clip();
    }));

    assert_eq!(clipped.scissor_clips, 1, "the clip lowered to a scissor");
    assert_eq!(clipped.mask_clips, 0);
    assert!(
        clipped.alphas().len() < unclipped.alphas().len(),
        "the clip cuts the glyph's right-hand coverage away ({} of {} bytes \
         survive)",
        clipped.alphas().len(),
        unclipped.alphas().len()
    );
    assert!(
        clipped.glyph_draws > 0,
        "and keeps the part of it the clip admits"
    );
}

// ---------------------------------------------------------------------
// The retained caches
// ---------------------------------------------------------------------

#[test]
fn recompiling_a_colour_glyph_against_a_warm_cache_is_byte_identical() {
    let scene = scene_of(|builder| builder.draw_glyph_run(emoji()));
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
        "a colour glyph rasterizes to the strips its cold fetch did — its \
         layer outlines come out of the same retained outline cache"
    );
    assert_eq!(cold.alphas(), warm.alphas());
    assert_eq!(cold.glyph_draws, warm.glyph_draws);
    assert_eq!(cold.draws().len(), warm.draws().len());
    assert_eq!(cold.lut_requests.len(), warm.lut_requests.len());
}

// ---------------------------------------------------------------------
// The other bundled scripts
// ---------------------------------------------------------------------

#[test]
fn a_run_of_arabic_glyphs_draws_one_per_glyph() {
    // The joining forms of `مرحبا`, laid out right to left. Shaping is
    // Parley's and happens above this crate; what is pinned here is that the
    // face's own outlines lower like any other.
    let frame = compile_run(run(
        ARABIC_FONT,
        &ARABIC_GLYPHS,
        24.0,
        -18.0,
        Affine::translate((110.0, 44.0)),
    ));

    assert_eq!(frame.glyph_draws, ARABIC_GLYPHS.len() as u32);
    assert_eq!(frame.skipped_glyphs, 0);
    assert!(
        frame.encoded_paints.is_empty(),
        "a solid run encodes inline"
    );
}

#[test]
fn a_run_of_cjk_glyphs_draws_one_per_glyph() {
    // `日本語` and neighbours, out of a CFF-outlined face — the one bundled
    // font whose outlines are PostScript rather than TrueType.
    let frame = compile_run(run(
        CJK_FONT,
        &CJK_GLYPHS,
        24.0,
        24.0,
        Affine::translate((4.0, 44.0)),
    ));

    assert_eq!(frame.glyph_draws, CJK_GLYPHS.len() as u32);
    assert_eq!(frame.skipped_glyphs, 0);
}
