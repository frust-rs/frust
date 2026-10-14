//! Glyph quads: a glyph run resolved entirely out of the glyph atlas draws each
//! glyph as ONE rect instance over its slot, painted by one record the run
//! shares per atlas layer — and covers, samples and orders exactly what the
//! general atlas draw (one image record and one generated rectangle per glyph)
//! did.
//!
//! The host half proves the premise first — every cached outline glyph lands
//! on whole device pixels in every horizontal subpixel bin `glifo` quantizes
//! into, so a quad needs no fractional edge coverage at all — then holds the
//! compiled frames of the two lowerings to each other draw for draw: the same
//! depths, the same coverage, the same atlas texel under every pixel. The GPU
//! half (`#[ignore]`d, pinned runner as every engine device suite) renders the
//! same scenes through both and requires the read-backs byte-identical, in
//! both strip program families and under the Uber program.
//!
//! ```text
//! WGPU_BACKEND=vulkan WGPU_ADAPTER_NAME=T400 \
//!   cargo test -p frust-engine --test glyph_quads -- --ignored --test-threads=1
//! ```

#![cfg(not(target_arch = "wasm32"))]

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use frust_engine::compile::{CompiledFrame, GlyphQuad};
use frust_engine::{EngineRenderer, EngineTarget, OutputAlpha, SceneCompiler};
use frust_gpu::{HeadlessTarget, TierCaps};
use frust_scene::{FontHandle, Glyph, GlyphRun, Scene, SceneBuilder};
use kurbo::{Affine, Rect};
use peniko::color::DynamicColor;
use peniko::{Blob, Brush, Color, ColorStop, FontData, Gradient, ImageQuality};
use vello_common::encode::EncodedPaint;
use vello_common::paint::{ImageId, ImageSource, Paint};
use vello_common::strip::Strip;

/// Noto Sans, subsetted to Latin plus combining marks — the bundled face the
/// crate's text cases name glyph ids against.
const LATIN_FONT: &[u8] = include_bytes!("../../../testing/fonts/NotoSans-Subset.ttf");

/// Glyph ids of `Hello` in [`LATIN_FONT`].
const HELLO: [u32; 5] = [5, 6, 7, 7, 8];

/// Every glyph id the subset carries (`maxp` counts 22), `.notdef` included.
const EVERY_GLYPH: std::ops::Range<u32> = 0..22;

/// Host and device extent.
const SIZE: u16 = 256;

/// The read-back format: R, G, B, A in byte order, no swizzle.
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// The Latin face, one process-wide handle — a `peniko::Blob`'s id is what the
/// atlas keys a font on, so two handles would be two fonts.
fn latin_font() -> FontHandle {
    static FONT: std::sync::OnceLock<FontHandle> = std::sync::OnceLock::new();
    FONT.get_or_init(|| FontHandle::new(FontData::new(Blob::new(Arc::new(LATIN_FONT)), 0)))
        .clone()
}

/// A run of `ids` at `size`, its glyphs `advance` apart starting at `x0`.
fn run(ids: &[u32], size: f32, x0: f32, advance: f32, brush: Brush, transform: Affine) -> GlyphRun {
    GlyphRun {
        font: latin_font(),
        font_size: size,
        brush,
        transform,
        glyphs: ids
            .iter()
            .enumerate()
            .map(|(index, id)| Glyph {
                id: *id,
                x: x0 + index as f32 * advance,
                y: 0.0,
            })
            .collect(),
    }
}

fn solid(color: Color) -> Brush {
    Brush::Solid(color)
}

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

fn scene_of(record: impl FnOnce(&mut SceneBuilder<'_>)) -> Scene {
    let mut scene = Scene::new();
    {
        let mut builder = SceneBuilder::new(&mut scene);
        record(&mut builder);
    }
    scene
}

/// Runs at every sixteenth of a pixel of horizontal offset — every subpixel
/// bin `glifo` quantizes a glyph's position into, four times over — at three
/// sizes, over a white backdrop.
fn subpixel_scene() -> Scene {
    scene_of(|builder| {
        builder.fill_rect(
            Rect::new(0.0, 0.0, f64::from(SIZE), f64::from(SIZE)),
            solid(Color::WHITE),
        );
        for step in 0..16_u32 {
            let fraction = step as f32 / 16.0;
            let size = [14.0, 18.5, 23.0][step as usize % 3];
            let baseline = 20.0 + f64::from(step) * 14.5;
            builder.draw_glyph_run(run(
                &HELLO,
                size,
                3.0 + fraction,
                11.0 + fraction,
                solid(Color::BLACK),
                Affine::translate((f64::from(step % 4) * 0.5, baseline)),
            ));
        }
    })
}

/// Text in every placement a quad has to be trimmed or carried through: a
/// pixel-aligned clip cutting through glyphs, two nested clips, glyphs off
/// the left and bottom edges, a translucent isolated layer, a uniformly
/// scaled run, and runs interleaved with gradient fills.
fn placement_scene() -> Scene {
    scene_of(|builder| {
        builder.fill_rect(
            Rect::new(0.0, 0.0, f64::from(SIZE), f64::from(SIZE)),
            two_stops(
                Gradient::new_linear((0.0, 0.0), (256.0, 256.0)),
                Color::from_rgb8(240, 240, 220),
                Color::from_rgb8(180, 200, 240),
            ),
        );
        let ink = Color::from_rgb8(20, 30, 90);
        // A clip whose edges cross glyphs on every side.
        builder.push_clip(Rect::new(13.0, 9.0, 61.0, 27.0));
        builder.draw_glyph_run(run(
            &HELLO,
            20.0,
            0.25,
            12.5,
            solid(ink),
            Affine::translate((6.0, 26.0)),
        ));
        builder.pop_clip();
        // Two nested clips, the inner narrower on the right.
        builder.push_clip(Rect::new(70.0, 4.0, 200.0, 60.0));
        builder.push_clip(Rect::new(60.0, 30.0, 131.0, 70.0));
        builder.draw_glyph_run(run(
            &[5, 6, 7, 8, 5, 6, 7, 8],
            22.0,
            0.6,
            13.0,
            solid(Color::from_rgba8(200, 0, 40, 200)),
            Affine::translate((66.0, 52.0)),
        ));
        builder.pop_clip();
        builder.pop_clip();
        // Off the left and the bottom edges.
        builder.draw_glyph_run(run(
            &HELLO,
            24.0,
            -9.4,
            13.0,
            solid(ink),
            Affine::translate((0.0, 100.0)),
        ));
        builder.draw_glyph_run(run(
            &HELLO,
            24.0,
            0.3,
            13.0,
            solid(ink),
            Affine::translate((150.0, 262.0)),
        ));
        // A translucent layer, drawn into a page and composited.
        builder.push_layer(Rect::new(8.0, 112.0, 128.0, 168.0), 0.5);
        builder.fill_rect(
            Rect::new(8.0, 112.0, 128.0, 168.0),
            solid(Color::from_rgb8(250, 250, 250)),
        );
        builder.draw_glyph_run(run(
            &[5, 6, 7, 7, 8, 5, 6],
            18.0,
            0.1,
            11.0,
            solid(Color::BLACK),
            Affine::translate((12.0, 150.0)),
        ));
        builder.pop_layer();
        // A uniform scale `glifo` absorbs into the size: still whole pixels.
        builder.push_transform(Affine::scale(1.5));
        builder.draw_glyph_run(run(
            &HELLO,
            12.0,
            0.7,
            9.0,
            solid(ink),
            Affine::translate((92.0, 82.0)),
        ));
        builder.pop_transform();
        // Short runs interleaved with gradient marks.
        for index in 0..6_u32 {
            let x = 140.0 + f64::from(index % 3) * 36.0;
            let y = 120.0 + f64::from(index / 3) * 40.0;
            builder.fill_rect(
                Rect::new(x, y - 16.0, x + 30.0, y + 4.0),
                two_stops(
                    Gradient::new_linear((x, y), (x + 30.0, y)),
                    Color::from_rgb8(255, 220, 120),
                    Color::from_rgb8(120, 220, 255),
                ),
            );
            builder.draw_glyph_run(run(
                &[5, 8],
                15.0,
                index as f32 * 0.3,
                9.0,
                solid(Color::from_rgb8(10, 10, 10)),
                Affine::translate((x + 2.0, y)),
            ));
        }
    })
}

/// A run under a rounded clip — a coverage mask, which a quad cannot carry.
fn masked_scene() -> Scene {
    scene_of(|builder| {
        builder.push_clip_rounded(Rect::new(4.0, 4.0, 90.0, 40.0), 9.0);
        builder.draw_glyph_run(run(
            &HELLO,
            24.0,
            0.0,
            14.0,
            solid(Color::BLACK),
            Affine::translate((6.0, 30.0)),
        ));
        builder.pop_clip();
    })
}

/// A rotated run and a skewed one: no whole-pixel placement exists.
fn transformed_scene() -> Scene {
    scene_of(|builder| {
        builder.draw_glyph_run(run(
            &HELLO,
            20.0,
            0.0,
            12.0,
            solid(Color::BLACK),
            Affine::translate((40.0, 60.0)) * Affine::rotate(0.3),
        ));
        builder.draw_glyph_run(run(
            &HELLO,
            20.0,
            0.0,
            12.0,
            solid(Color::BLACK),
            Affine::translate((40.0, 140.0)) * Affine::skew(0.25, 0.0),
        ));
    })
}

fn compiler(quads: bool) -> SceneCompiler {
    let mut compiler = SceneCompiler::new(SIZE, SIZE);
    compiler.set_hint_text(false);
    compiler.set_glyph_quads(quads);
    compiler
}

/// Compile `scene` and acknowledge what a serviced frame acknowledges.
fn compile(compiler: &mut SceneCompiler, scene: &Scene) -> CompiledFrame {
    let frame = compiler
        .compile(scene, Affine::IDENTITY, (SIZE, SIZE))
        .expect("an in-range scene compiles");
    compiler.acknowledge_glyph_clears();
    compiler.acknowledge_glyph_replay();
    compiler.acknowledge_image_plan();
    frame
}

/// Frames compiled before [`settled`]'s: past the atlas policy's size-change
/// guard (a font first drawn at several sizes reads as a size change and is
/// drawn as outlines for eight frames), then one more to rasterize every
/// glyph into the atlas.
const SETTLING_FRAMES: usize = 12;

/// A frame of `scene` past [`SETTLING_FRAMES`] — every glyph is resident.
fn settled(quads: bool, scene: &Scene) -> CompiledFrame {
    let mut compiler = compiler(quads);
    for _ in 0..SETTLING_FRAMES {
        compile(&mut compiler, scene);
    }
    compile(&mut compiler, scene)
}

/// One draw's coverage, decoded from its strips the way the renderer's
/// instances read them: per strip pair, the alpha-sampled span (one column of
/// four coverage bytes per pixel) and the solid gap fill after it. Only
/// non-zero pixels are kept.
fn coverage(
    frame: &CompiledFrame,
    strip_range: std::ops::Range<usize>,
) -> BTreeMap<(u32, u32), u8> {
    let strips: &[Strip] = &frame.strip_buf()[strip_range];
    let alphas = frame.alphas();
    let mut out = BTreeMap::new();
    for pair in strips.windows(2) {
        let (strip, next) = (&pair[0], &pair[1]);
        let width = u32::from(strip.width_to(next));
        let base = strip.alpha_idx() as usize;
        for column in 0..width {
            for row in 0..4_u32 {
                let alpha = alphas[base + (column * 4 + row) as usize];
                if alpha != 0 {
                    out.insert(
                        (u32::from(strip.x) + column, u32::from(strip.y) + row),
                        alpha,
                    );
                }
            }
        }
        if next.fill_gap() && next.y == strip.y {
            let start = u32::from(strip.x) + width;
            for x in start..u32::from(next.x) {
                for row in 0..4_u32 {
                    out.insert((x, u32::from(strip.y) + row), 255);
                }
            }
        }
    }
    out
}

/// The whole-pixel rectangle `[x0, x1) x [y0, y1)` exactly `cover` fills at
/// full coverage, or `None` when it is anything else — a partial pixel, a
/// hole, or a non-rectangular footprint.
fn full_rect(cover: &BTreeMap<(u32, u32), u8>) -> Option<[u32; 4]> {
    if cover.is_empty() || cover.values().any(|alpha| *alpha != 255) {
        return None;
    }
    let x0 = cover.keys().map(|(x, _)| *x).min()?;
    let x1 = cover.keys().map(|(x, _)| *x).max()? + 1;
    let y0 = cover.keys().map(|(_, y)| *y).min()?;
    let y1 = cover.keys().map(|(_, y)| *y).max()? + 1;
    ((x1 - x0) as usize * (y1 - y0) as usize == cover.len()).then_some([x0, y0, x1, y1])
}

/// The image record a draw paints with, if it paints with one.
fn image_record<'a>(
    frame: &'a CompiledFrame,
    paint: &Paint,
) -> Option<&'a vello_common::encode::EncodedImage> {
    match paint {
        Paint::Indexed(indexed) => match frame.encoded_paints.get(indexed.index())? {
            EncodedPaint::Image(image) => Some(image),
            _ => None,
        },
        Paint::Solid(_) => None,
    }
}

fn image_id(image: &vello_common::encode::EncodedImage) -> ImageId {
    match &image.source {
        ImageSource::OpaqueId { id, .. } => *id,
        ImageSource::Pixmap(_) => panic!("an atlas glyph names its slot"),
    }
}

fn image_records(frame: &CompiledFrame) -> usize {
    frame
        .encoded_paints
        .iter()
        .filter(|paint| matches!(paint, EncodedPaint::Image(_)))
        .count()
}

/// The quad the draw at `depth` is.
fn quad_at(frame: &CompiledFrame, depth: u32) -> Option<&GlyphQuad> {
    frame.glyph_quads.iter().find(|quad| quad.depth == depth)
}

/// A quad's rectangle as `[x0, y0, x1, y1]`.
fn quad_rect(quad: &GlyphQuad) -> [u32; 4] {
    [
        u32::from(quad.x),
        u32::from(quad.y),
        u32::from(quad.x) + u32::from(quad.width),
        u32::from(quad.y) + u32::from(quad.height),
    ]
}

// --- The premise: no fractional edge ever reaches a quad -------------------

/// The proof a glyph quad needs no fractional edge coverage: on a settled
/// frame drawn the general way, every atlas glyph — across every subpixel bin
/// — is painted nearest-sampled, placed by a whole-pixel translation, and its
/// generated coverage is exactly its slot's rectangle at full coverage. So a
/// rect instance with all four edge fractions zero reproduces the strip
/// rasterizer's coverage byte for byte, in every bin.
#[test]
fn every_cached_outline_glyph_lands_on_whole_pixels_in_every_subpixel_bin() {
    let scene = subpixel_scene();
    let frame = settled(false, &scene);

    let slots: HashMap<ImageId, [u32; 2]> = frame
        .glyph_slots
        .iter()
        .map(|slot| (slot.id, slot.region.size))
        .collect();
    let mut atlas_draws = 0_u32;
    let mut whole = 0_u32;
    let mut bins = BTreeSet::new();
    for draw in frame.draws() {
        let Some(image) = image_record(&frame, &draw.paint) else {
            continue;
        };
        atlas_draws += 1;
        assert_eq!(
            image.sampler.quality,
            ImageQuality::Low,
            "a cached outline is sampled nearest"
        );
        let [a, b, c, d, tx, ty] = image.transform.as_coeffs();
        assert_eq!(
            [a, b, c, d],
            [1.0, 0.0, 0.0, 1.0],
            "placed by a translation alone"
        );
        assert!(
            tx.fract() == 0.0 && ty.fract() == 0.0,
            "placed on whole pixels: ({tx}, {ty})"
        );
        let size = slots[&image_id(image)];
        let cover = coverage(&frame, draw.strip_range.clone());
        let rect = full_rect(&cover).expect("an atlas glyph covers whole pixels only");
        assert_eq!(
            [rect[2] - rect[0], rect[3] - rect[1]],
            size,
            "its slot's own extent"
        );
        // The device corner is where the inverse translation puts the slot's
        // origin.
        assert_eq!(
            [f64::from(rect[0]) + tx, f64::from(rect[1]) + ty],
            [0.0, 0.0]
        );
        whole += 1;
    }

    // Which bins the scene's glyph positions fall in, by `glifo`'s own rule:
    // the fractional device x, in quarters, rounded and held below four.
    for command in scene.commands() {
        let frust_scene::Command::GlyphRun(run) = command else {
            continue;
        };
        for glyph in &run.glyphs {
            let x = run.transform.translation().x + f64::from(glyph.x);
            let fraction = x - x.floor();
            bins.insert(((fraction * 4.0).round() as u32).min(3));
        }
    }

    assert!(atlas_draws >= 16 * 4, "the scene's glyphs are all cached");
    assert_eq!(whole, atlas_draws);
    assert_eq!(bins.len(), 4, "every subpixel bin is exercised: {bins:?}");
}

// --- What a cached run lowers to -----------------------------------------

#[test]
fn a_settled_run_draws_one_quad_per_inked_glyph_and_one_record_per_run() {
    let scene = scene_of(|builder| {
        builder.draw_glyph_run(run(
            &HELLO,
            24.0,
            0.25,
            15.0,
            solid(Color::BLACK),
            Affine::translate((8.0, 40.0)),
        ));
    });
    let frame = settled(true, &scene);

    assert_eq!(frame.glyph_draws, 5);
    assert_eq!(frame.atlas_glyph_draws, 5);
    assert_eq!(frame.glyph_quads.len(), 5, "one quad per inked glyph");
    assert_eq!(
        frame.glyph_slots.len(),
        5,
        "every quad still reports its slot"
    );
    assert_eq!(image_records(&frame), 1, "one record for the whole run");
    assert_eq!(frame.glyph_run_paints, vec![0]);
    assert_eq!(frame.draws().len(), 5);
    for draw in frame.draws() {
        assert!(matches!(draw.paint, Paint::Indexed(ref indexed) if indexed.index() == 0));
        assert!(
            quad_at(&frame, draw.depth).is_some(),
            "every draw of the run is a quad"
        );
        assert!(
            draw.strip_range.is_empty(),
            "outside a layer a quad generates no coverage"
        );
    }
    let depths: Vec<u32> = frame.glyph_quads.iter().map(|quad| quad.depth).collect();
    assert!(
        depths.windows(2).all(|pair| pair[0] < pair[1]),
        "in depth order"
    );
    assert_eq!(frame.fast_rect_draws, 0);
    assert!(
        frame.strip_buf().is_empty(),
        "nothing in the frame generated strips"
    );
}

#[test]
fn a_quad_inside_a_layer_carries_its_rectangle_into_the_layer_bounds() {
    let frame = settled(true, &placement_scene());
    assert_eq!(frame.recorder.layers.len(), 1);
    let bounds = frame.recorder.layers[0].bbox;
    let mut inside = 0;
    for draw in frame.draws() {
        let Some(quad) = quad_at(&frame, draw.depth) else {
            continue;
        };
        if draw.strip_range.is_empty() {
            continue;
        }
        inside += 1;
        let rect = full_rect(&coverage(&frame, draw.strip_range.clone()))
            .expect("a quad's strips are its rectangle at full coverage");
        assert_eq!(rect, quad_rect(quad));
        assert!(
            bounds.x0 <= quad.x
                && bounds.y0 <= quad.y
                && u32::from(bounds.x1) >= rect[2]
                && u32::from(bounds.y1) >= rect[3],
            "{quad:?} inside {bounds:?}"
        );
    }
    assert_eq!(
        inside, 7,
        "the layer's run is drawn as quads, each with coverage"
    );
}

#[test]
fn a_glyph_with_no_ink_draws_no_quad_and_no_record() {
    // Each glyph id alone, both ways: a glyph the general draw paints nothing
    // for — the space — gets no quad either.
    let mut inkless = Vec::new();
    for id in EVERY_GLYPH {
        let scene = scene_of(|builder| {
            builder.draw_glyph_run(run(
                &[id],
                24.0,
                0.0,
                0.0,
                solid(Color::BLACK),
                Affine::translate((30.0, 60.0)),
            ));
        });
        let general = settled(false, &scene);
        let quads = settled(true, &scene);
        assert_eq!(general.glyph_draws, quads.glyph_draws, "glyph {id}");
        assert_eq!(general.draws().len(), quads.draws().len(), "glyph {id}");
        if general.glyph_draws == 0 {
            inkless.push(id);
            assert!(quads.glyph_quads.is_empty(), "glyph {id}");
            assert!(quads.encoded_paints.is_empty(), "glyph {id}");
            assert!(quads.glyph_run_paints.is_empty(), "glyph {id}");
        } else {
            assert_eq!(quads.glyph_quads.len(), 1, "glyph {id}");
        }
    }
    assert!(!inkless.is_empty(), "the subset carries a space");
}

#[test]
fn the_frame_a_run_is_first_rasterized_on_keeps_todays_lowering() {
    // One size, so the run takes the atlas on its very first frame.
    let scene = scene_of(|builder| {
        builder.draw_glyph_run(run(
            &[5, 6, 7, 8, 5, 6, 7, 8],
            21.0,
            0.4,
            13.0,
            solid(Color::BLACK),
            Affine::translate((8.0, 40.0)),
        ));
    });
    let mut compiler = compiler(true);
    let first = compile(&mut compiler, &scene);
    assert_eq!(
        first.atlas_glyph_draws, 8,
        "the run is cached on its first frame"
    );
    assert!(
        first.glyph_quads.is_empty(),
        "the run rasterized its glyphs this frame, so it is drawn the general way"
    );
    assert_eq!(
        image_records(&first),
        8,
        "one record per glyph, as before quads"
    );

    let second = compile(&mut compiler, &scene);
    assert_eq!(second.glyph_quads.len(), 8);
    assert_eq!(image_records(&second), 1);
}

#[test]
fn a_run_that_missed_is_drawn_the_general_way_beside_runs_that_did_not() {
    // The placement scene's first frame past the size guard: a run whose
    // glyphs an earlier run of the same frame (or an earlier frame) already
    // rasterized hits throughout and draws quads, while a run that rasterized
    // anything draws every glyph the general way — so the frame mixes both.
    let scene = placement_scene();
    let mut compiler = compiler(true);
    let mixed = (0..=SETTLING_FRAMES)
        .map(|_| compile(&mut compiler, &scene))
        .find(|frame| frame.atlas_glyph_draws == frame.glyph_draws)
        .expect("every run reaches the atlas once the sizes settle");
    let quads = mixed.glyph_quads.len();
    assert!(
        quads > 0 && quads < mixed.atlas_glyph_draws as usize,
        "{quads} quads"
    );
    assert_eq!(
        image_records(&mixed),
        mixed.atlas_glyph_draws as usize - quads + mixed.glyph_run_paints.len(),
        "one record per general draw, one per quad run and layer"
    );
}

#[test]
fn the_kill_switch_restores_one_record_per_glyph() {
    let scene = placement_scene();
    let mut compiler = compiler(true);
    assert!(compiler.glyph_quads_enabled());
    compiler.set_glyph_quads(false);
    assert!(!compiler.glyph_quads_enabled());
    for _ in 0..SETTLING_FRAMES {
        compile(&mut compiler, &scene);
    }
    let frame = compile(&mut compiler, &scene);

    assert!(frame.atlas_glyph_draws > 0);
    assert!(frame.glyph_quads.is_empty());
    assert!(frame.glyph_run_paints.is_empty());
    assert_eq!(
        frame
            .draws()
            .iter()
            .filter(|draw| image_record(&frame, &draw.paint).is_some())
            .count(),
        frame.atlas_glyph_draws as usize,
        "one image record per atlas glyph draw"
    );
    assert_eq!(image_records(&frame), frame.atlas_glyph_draws as usize);
}

#[test]
fn a_coverage_mask_keeps_todays_lowering() {
    let frame = settled(true, &masked_scene());
    assert!(frame.atlas_glyph_draws > 0, "the run is cached");
    assert!(frame.mask_clips > 0, "the rounded clip is a mask");
    assert!(frame.glyph_quads.is_empty(), "a quad cannot carry a mask");
    assert_eq!(image_records(&frame), frame.atlas_glyph_draws as usize);
}

#[test]
fn a_transform_with_no_whole_pixel_placement_keeps_todays_lowering() {
    let quads = settled(true, &transformed_scene());
    let general = settled(false, &transformed_scene());
    assert!(quads.glyph_draws > 0, "the runs still draw");
    assert!(quads.glyph_quads.is_empty());
    assert_eq!(quads.glyph_draws, general.glyph_draws);
    assert_eq!(quads.draws().len(), general.draws().len());
}

// --- The two lowerings agree draw for draw --------------------------------

/// Settled frames of `scene` both ways cover, order and sample identically:
/// the same draws at the same depths with the same coverage, every quad
/// exactly its general draw's rectangle, and the atlas texel under every quad
/// pixel the one the general draw's record maps that pixel to.
fn assert_lowerings_agree(name: &str, scene: &Scene) -> usize {
    let general = settled(false, scene);
    let quads = settled(true, scene);

    assert_eq!(
        general.draws().len(),
        quads.draws().len(),
        "{name}: draw count"
    );
    assert_eq!(general.glyph_draws, quads.glyph_draws, "{name}");
    assert_eq!(general.atlas_glyph_draws, quads.atlas_glyph_draws, "{name}");
    assert_eq!(general.skipped_glyphs, quads.skipped_glyphs, "{name}");
    let general_slots: HashMap<ImageId, [u32; 2]> = general
        .glyph_slots
        .iter()
        .map(|slot| (slot.id, slot.region.offset))
        .collect();
    let general_layers: HashMap<ImageId, u32> = general
        .glyph_slots
        .iter()
        .map(|slot| (slot.id, slot.region.layer))
        .collect();

    for (index, (a, b)) in general.draws().iter().zip(quads.draws()).enumerate() {
        assert_eq!(a.depth, b.depth, "{name}: draw {index} depth");
        let general_cover = coverage(&general, a.strip_range.clone());
        let quad = quad_at(&quads, b.depth);
        // A quad outside a layer generates no coverage; every other draw — a
        // quad inside one included — covers exactly what the general one did.
        if quad.is_none() || !b.strip_range.is_empty() {
            let quad_cover = coverage(&quads, b.strip_range.clone());
            assert_eq!(general_cover, quad_cover, "{name}: draw {index} coverage");
        }

        let Some(quad) = quad else {
            // Not a quad: the same paint, though at a different index once an
            // earlier run's quads shared one record.
            let encoded = |frame: &CompiledFrame, paint: &Paint| match paint {
                Paint::Solid(color) => format!("{color:?}"),
                Paint::Indexed(indexed) => format!("{:?}", frame.encoded_paints[indexed.index()]),
            };
            assert_eq!(
                encoded(&general, &a.paint),
                encoded(&quads, &b.paint),
                "{name}: draw {index} paint"
            );
            continue;
        };
        let record = image_record(&general, &a.paint)
            .unwrap_or_else(|| panic!("{name}: draw {index} is an atlas glyph in both"));
        let rect = full_rect(&general_cover).expect("whole pixels");
        assert_eq!(rect, quad_rect(quad), "{name}: draw {index} rectangle");
        // Device x maps to image x by the record's inverse translation, then
        // into the atlas by the slot's offset.
        let [_, _, _, _, tx, ty] = record.transform.as_coeffs();
        let offset = general_slots[&image_id(record)];
        assert_eq!(
            [f64::from(quad.atlas_x), f64::from(quad.atlas_y)],
            [
                f64::from(offset[0]) + f64::from(quad.x) + tx,
                f64::from(offset[1]) + f64::from(quad.y) + ty
            ],
            "{name}: draw {index} atlas texel"
        );
        // The shared record resolves to the same layer and tint.
        let shared = image_record(&quads, &b.paint).expect("a quad paints with its run's record");
        assert_eq!(shared.tint, record.tint, "{name}: draw {index} tint");
        assert_eq!(
            general_layers[&image_id(record)],
            quads
                .glyph_slots
                .iter()
                .find(|slot| slot.id == image_id(shared))
                .map(|slot| slot.region.layer)
                .expect("the record names a reported slot"),
            "{name}: draw {index} layer"
        );
    }
    assert!(
        quads
            .glyph_run_paints
            .windows(2)
            .all(|pair| pair[0] < pair[1]),
        "{name}: records in ascending order"
    );
    quads.glyph_quads.len()
}

#[test]
fn quads_cover_order_and_sample_exactly_what_todays_lowering_does() {
    let subpixel = assert_lowerings_agree("subpixel", &subpixel_scene());
    let placement = assert_lowerings_agree("placement", &placement_scene());
    assert_lowerings_agree("masked", &masked_scene());
    assert_lowerings_agree("transformed", &transformed_scene());
    assert!(subpixel >= 64, "the subpixel scene is drawn as quads");
    assert!(placement > 0, "the placement scene is drawn as quads");
}

/// The settled frames the equivalence cases above compare are drawn out of
/// the resident-run cache — every run remembered from an earlier frame and
/// drawn without reaching `glifo` — so the agreement they assert is the
/// cache's as much as the quads' own; and with the cache switched off the same
/// frames come back identical.
#[test]
fn the_settled_frames_compared_here_are_drawn_from_the_resident_run_cache() {
    let subpixel = settled(true, &subpixel_scene());
    assert_eq!(
        subpixel.resident_glyph_runs, 16,
        "every run of the subpixel scene"
    );
    assert!(settled(true, &placement_scene()).resident_glyph_runs > 0);
    assert_eq!(settled(false, &subpixel_scene()).resident_glyph_runs, 0);

    for scene in [subpixel_scene(), placement_scene()] {
        let mut probing = compiler(true);
        probing.set_glyph_run_cache(false);
        for _ in 0..SETTLING_FRAMES {
            compile(&mut probing, &scene);
        }
        let probed = compile(&mut probing, &scene);
        let mut cached = settled(true, &scene);
        assert_eq!(probed.resident_glyph_runs, 0);
        assert!(cached.resident_glyph_runs > 0);
        cached.resident_glyph_runs = 0;
        assert_eq!(format!("{cached:?}"), format!("{probed:?}"));
    }
}

#[test]
fn a_quad_is_trimmed_to_the_clip_with_its_atlas_texel_moved_by_the_same_amount() {
    let frame = settled(true, &placement_scene());
    let clip = [13_u16, 9, 61, 27];
    let trimmed: Vec<&GlyphQuad> = frame
        .glyph_quads
        .iter()
        .filter(|quad| quad.y < 30)
        .filter(|quad| quad.x < 64)
        .collect();
    assert!(!trimmed.is_empty());
    for quad in trimmed {
        assert!(quad.x >= clip[0] && quad.y >= clip[1], "{quad:?}");
        assert!(
            quad.x + quad.width <= clip[2] && quad.y + quad.height <= clip[3],
            "{quad:?}"
        );
    }
    // Nothing reaches past the device edges.
    for quad in &frame.glyph_quads {
        assert!(u32::from(quad.x) + u32::from(quad.width) <= u32::from(SIZE));
        assert!(u32::from(quad.y) + u32::from(quad.height) <= u32::from(SIZE));
    }
}

// --- GPU: byte-identical read-backs --------------------------------------

/// Serializes every device-creating test in this binary (see
/// `encode_contract.rs`'s identical guard for why the NVIDIA driver needs it).
static RENDER_LOCK: Mutex<()> = Mutex::new(());

fn render_lock() -> MutexGuard<'static, ()> {
    RENDER_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    use std::task::{Context, Poll, Waker};

    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}

fn drain_error_scope(device: &wgpu::Device, scope: wgpu::ErrorScopeGuard) -> Option<wgpu::Error> {
    use std::task::{Context, Poll, Waker};

    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);
    let mut future = std::pin::pin!(scope.pop());
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(error) => return error,
            Poll::Pending => {
                let _ = device.poll(wgpu::PollType::wait_indefinitely());
            }
        }
    }
}

fn gpu() -> (wgpu::Device, wgpu::Queue, TierCaps) {
    block_on(async {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = wgpu::util::initialize_adapter_from_env_or_default(&instance, None)
            .await
            .expect("no compatible GPU adapter");
        println!("frust-engine glyph-quad adapter: {:?}", adapter.get_info());
        let caps = TierCaps::probe(&adapter);
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("frust-engine glyph-quad device"),
                required_features: wgpu::Features::empty(),
                required_limits: frust_gpu::test_device_limits(&adapter, &caps),
                ..Default::default()
            })
            .await
            .expect("failed to create the device");
        (device, queue, caps)
    })
}

/// Renders `scene` for `frames` frames through a fresh renderer of the given
/// program family, configured by `configure`, returning every read-back.
fn render(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    caps: &TierCaps,
    scene: &Scene,
    image_sampler: bool,
    configure: impl FnOnce(&mut EngineRenderer),
) -> Vec<Vec<u8>> {
    let mut renderer =
        EngineRenderer::with_image_sampler(device, caps, FORMAT, None, image_sampler)
            .expect("the engine builds on this device");
    configure(&mut renderer);
    renderer.finish_warm_up(device);
    let size = u32::from(SIZE);
    let target = HeadlessTarget::new(device, size, size, FORMAT);
    let mut frames = Vec::new();
    for _ in 0..=SETTLING_FRAMES + 1 {
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("frust-engine glyph-quad frame"),
        });
        renderer
            .encode(
                device,
                queue,
                &mut encoder,
                scene,
                EngineTarget {
                    view: target.view(),
                    format: FORMAT,
                    width: size,
                    height: size,
                    depth: None,
                    output: OutputAlpha::Premultiplied,
                },
                Color::TRANSPARENT,
                Affine::IDENTITY,
            )
            .expect("a well-formed frame encodes");
        queue.submit([encoder.finish()]);
        renderer.end_frame(queue);
        let error = drain_error_scope(device, scope);
        assert!(error.is_none(), "the frame raised {error:?}");
        frames.push(target.read_back(device, queue));
    }
    frames
}

fn differing(a: &[u8], b: &[u8]) -> usize {
    a.as_chunks::<4>()
        .0
        .iter()
        .zip(b.as_chunks::<4>().0)
        .filter(|(x, y)| x != y)
        .count()
}

/// The first frame of `scene` on which every glyph draw samples the atlas —
/// the frame a run first rasterizes on draws the general way, so from here on
/// every frame must read back the same.
fn first_atlas_frame(scene: &Scene) -> usize {
    let mut compiler = compiler(true);
    (0..=SETTLING_FRAMES)
        .find(|_| {
            let frame = compile(&mut compiler, scene);
            frame.glyph_draws > 0 && frame.atlas_glyph_draws == frame.glyph_draws
        })
        .unwrap_or(0)
}

/// The acceptance bar on a device: every frame of a scene drawn with glyph
/// quads reads back byte-identical to the same frame drawn the general way,
/// and every frame from the first one on which the atlas serves the whole
/// scene — the frame that rasterized its glyphs, drawn the general way — reads
/// back identical to that frame, in both strip program families, through
/// class runs and through the Uber program alone.
#[test]
#[ignore = "requires a GPU (Vulkan/Metal); run with `cargo test -p frust-engine --test glyph_quads -- --ignored`"]
fn glyph_quads_render_byte_identically_to_todays_lowering() {
    let _guard = render_lock();
    let (device, queue, caps) = gpu();

    let cases = [
        ("subpixel", subpixel_scene()),
        ("placement", placement_scene()),
        ("masked", masked_scene()),
        ("transformed", transformed_scene()),
    ];
    for (name, scene) in &cases {
        // Quads really are drawn in the settled frames of the scenes built for
        // them (the renderer's own compiler runs the same policy).
        if matches!(*name, "subpixel" | "placement") {
            assert!(!settled(true, scene).glyph_quads.is_empty(), "{name}");
        }
        let first_atlas = first_atlas_frame(scene);
        for image_sampler in [true, false] {
            for uber in [false, true] {
                let general = render(&device, &queue, &caps, scene, image_sampler, |r| {
                    r.set_glyph_quads(false);
                    r.set_uber_program(uber);
                });
                let quads = render(&device, &queue, &caps, scene, image_sampler, |r| {
                    r.set_glyph_quads(true);
                    r.set_uber_program(uber);
                });
                assert!(
                    general[0].as_chunks::<4>().0.iter().any(|px| px[3] != 0),
                    "{name}: the frame drew nothing"
                );
                let mut worst = (0, 0);
                for frame in 0..general.len() {
                    let against_general = differing(&general[frame], &quads[frame]);
                    let against_first = if frame >= first_atlas {
                        differing(&general[first_atlas], &quads[frame])
                    } else {
                        0
                    };
                    worst = (worst.0.max(against_general), worst.1.max(against_first));
                    assert_eq!(
                        against_general, 0,
                        "{name} (sampler {image_sampler}, uber {uber}, frame {frame}): \
                         {against_general} pixels differ from the general lowering"
                    );
                    assert_eq!(
                        against_first, 0,
                        "{name} (sampler {image_sampler}, uber {uber}, frame {frame}): \
                         {against_first} pixels differ from frame {first_atlas}, the first the \
                         atlas served whole"
                    );
                }
                println!(
                    "glyph quads: [{name}] sampler={image_sampler} uber={uber}: {} frames, atlas \
                     whole from frame {first_atlas}; worst {} px against the general lowering, \
                     {} px against frame {first_atlas}",
                    general.len(),
                    worst.0,
                    worst.1
                );
            }
        }
    }
}
