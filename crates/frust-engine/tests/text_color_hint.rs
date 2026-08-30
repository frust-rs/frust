//! Colour (COLR) glyphs and the hinting policy: what a colour glyph costs a
//! frame and what it refuses, and which half of "is this run hinted" belongs
//! to `SceneCompiler` rather than to `glifo`.
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
//!
//! The hinting cases below pin the other half of `p5-03`'s undeliverable
//! scope (see `frust_engine::text`'s module doc): `SceneCompiler::for_caps`
//! turns hinting on for a desktop-class `TierCaps` and off for a mobile one,
//! `SceneCompiler::set_hint_text` answers the same question directly for a
//! case that wants either answer without building one, and the retained
//! `GlyphPrepCache` a compiler carries across frames must not leak a hinted
//! outline into an unhinted frame or back — `glifo`'s own outline cache keys
//! on its hint flag for exactly that reason. What is *not* pinned here is the
//! transform-shape half of the policy (a rotated or skewed run is unhinted
//! regardless of `hint_text`): that predicate lives inside `glifo` and is
//! `glifo`'s own to test.

use std::collections::BTreeSet;
use std::sync::Arc;

use frust_engine::SceneCompiler;
use frust_engine::compile::CompiledFrame;
use frust_gpu::{DownlevelProfile, TierCaps};
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

/// Noto Sans, subsetted to Latin plus combining marks — the same bundled face
/// `text_basic.rs` hints its own cases against.
const LATIN_FONT: &[u8] = include_bytes!("../../../testing/fonts/NotoSans-Subset.ttf");

/// `LATIN_FONT`'s `H`, `e`, `l`, `o`, read off its own character map — the
/// same ids `text_basic.rs` uses.
const HELLO_GLYPHS: [u32; 5] = [5, 6, 7, 7, 8];

/// A font size small enough that vertical hinting's pixel-grid snapping has
/// something to move: at a large size a hinted and an unhinted stem already
/// sit close enough to the same pixel that the two outlines can coincide by
/// chance, which is exactly the opposite of what these cases need to pin.
const SMALL_LATIN_SIZE: f32 = 9.0;

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

/// A compiler with `hint_text` set directly, bypassing `TierCaps`.
fn compiler_hinted(hint_text: bool) -> SceneCompiler {
    let mut compiler = compiler();
    compiler.set_hint_text(hint_text);
    compiler
}

/// `HELLO_GLYPHS` at [`SMALL_LATIN_SIZE`], translate-only (so the transform is
/// the positive-uniform-scale-without-skew shape `glifo` hints under) — the
/// small Latin run every hinting case below draws.
fn small_latin_run() -> GlyphRun {
    GlyphRun {
        font: font(LATIN_FONT),
        font_size: SMALL_LATIN_SIZE,
        brush: Brush::Solid(RED),
        transform: Affine::translate((4.0, 32.0)),
        glyphs: HELLO_GLYPHS
            .iter()
            .enumerate()
            .map(|(index, id)| Glyph {
                id: *id,
                x: index as f32 * (SMALL_LATIN_SIZE * 0.6),
                y: 0.0,
            })
            .collect(),
    }
}

/// Compile `glyph_run` once against `compiler`.
fn compile_with(mut compiler: SceneCompiler, glyph_run: GlyphRun) -> CompiledFrame {
    compiler
        .compile(
            &scene_of(|builder| builder.draw_glyph_run(glyph_run)),
            Affine::IDENTITY,
            VIEWPORT,
        )
        .expect("an in-range scene compiles")
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

// ---------------------------------------------------------------------
// The hinting policy's device-class half
// ---------------------------------------------------------------------

#[test]
fn hinting_changes_a_small_latin_runs_output_on_a_desktop_class_compiler() {
    let unhinted = compile_with(compiler_hinted(false), small_latin_run());
    let hinted = compile_with(compiler_hinted(true), small_latin_run());

    assert_eq!(
        unhinted.glyph_draws, hinted.glyph_draws,
        "the same glyphs paint either way"
    );
    assert!(
        unhinted.strip_buf() != hinted.strip_buf() || unhinted.alphas() != hinted.alphas(),
        "vertical hinting snaps at least one stem in this small run to a \
         different pixel than the unhinted outline lands on"
    );
}

#[test]
fn a_hinted_runs_output_is_byte_stable_across_two_frames() {
    let mut compiler = compiler_hinted(true);
    let scene = scene_of(|builder| builder.draw_glyph_run(small_latin_run()));

    let cold = compiler
        .compile(&scene, Affine::IDENTITY, VIEWPORT)
        .expect("the first frame compiles");
    let warm = compiler
        .compile(&scene, Affine::IDENTITY, VIEWPORT)
        .expect("the second frame compiles");

    assert_eq!(
        cold.strip_buf(),
        warm.strip_buf(),
        "a hinted outline rasterizes identically on a warm cache, the same \
         guarantee the colour-glyph cache case above pins for an unhinted one"
    );
    assert_eq!(cold.alphas(), warm.alphas());
    assert_eq!(cold.glyph_draws, warm.glyph_draws);
}

#[test]
fn a_mobile_tier_compiler_draws_unhinted_output_byte_identical_to_hint_text_false() {
    let mobile = compile_with(
        SceneCompiler::for_caps(
            VIEWPORT.0,
            VIEWPORT.1,
            &TierCaps::fake(DownlevelProfile::WebGl2),
        ),
        small_latin_run(),
    );
    let explicit_unhinted = compile_with(compiler_hinted(false), small_latin_run());

    assert_eq!(
        mobile.strip_buf(),
        explicit_unhinted.strip_buf(),
        "a mobile-class TierCaps draws the same unhinted output the run \
         always drew before this policy could turn hinting on"
    );
    assert_eq!(mobile.alphas(), explicit_unhinted.alphas());
    assert_eq!(mobile.glyph_draws, explicit_unhinted.glyph_draws);
}

#[test]
fn a_desktop_tier_compiler_draws_hinted_output_byte_identical_to_hint_text_true() {
    let desktop = compile_with(
        SceneCompiler::for_caps(
            VIEWPORT.0,
            VIEWPORT.1,
            &TierCaps::fake(DownlevelProfile::Full),
        ),
        small_latin_run(),
    );
    let explicit_hinted = compile_with(compiler_hinted(true), small_latin_run());

    assert_eq!(desktop.strip_buf(), explicit_hinted.strip_buf());
    assert_eq!(desktop.alphas(), explicit_hinted.alphas());
    assert_eq!(desktop.glyph_draws, explicit_hinted.glyph_draws);
}

#[test]
fn flipping_hint_text_on_one_compiler_never_serves_a_stale_cache_entry() {
    // One compiler, one retained `GlyphPrepCache`, three frames that toggle
    // `hint_text` between them — pinning that `glifo`'s outline cache (keyed
    // on its own `hint` flag) never hands a frame the other frame's outline
    // rather than fetching its own.
    let mut compiler = compiler();
    let scene = scene_of(|builder| builder.draw_glyph_run(small_latin_run()));

    compiler.set_hint_text(true);
    let hinted_first = compiler
        .compile(&scene, Affine::IDENTITY, VIEWPORT)
        .expect("a hinted frame compiles");

    compiler.set_hint_text(false);
    let unhinted = compiler
        .compile(&scene, Affine::IDENTITY, VIEWPORT)
        .expect("an unhinted frame compiles");

    compiler.set_hint_text(true);
    let hinted_second = compiler
        .compile(&scene, Affine::IDENTITY, VIEWPORT)
        .expect("hinting turned back on compiles");

    let baseline_hinted = compile_with(compiler_hinted(true), small_latin_run());
    let baseline_unhinted = compile_with(compiler_hinted(false), small_latin_run());

    assert_eq!(
        hinted_first.strip_buf(),
        baseline_hinted.strip_buf(),
        "the first hinted frame matches a compiler that was hinted from the start"
    );
    assert_eq!(
        unhinted.strip_buf(),
        baseline_unhinted.strip_buf(),
        "turning hinting off does not keep serving the hinted frame's cached \
         outline"
    );
    assert_eq!(
        hinted_second.strip_buf(),
        baseline_hinted.strip_buf(),
        "and turning it back on again does not keep serving the unhinted \
         frame's cached outline either — both directions round-trip cleanly \
         on the one retained cache"
    );
}
