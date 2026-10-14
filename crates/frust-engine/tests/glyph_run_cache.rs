//! The resident glyph-run cache: a glyph run whose every glyph is resident in
//! the atlas is remembered with the slots its glyphs resolved to, and a later
//! frame draws it from those slots without handing the run to `glifo` — and
//! draws exactly what `glifo` would have.
//!
//! Every case here runs two compilers side by side on the same frames: one with
//! the cache, one with it switched off (`set_glyph_run_cache(false)`, the
//! per-compiler counterpart of `FRUST_ENGINE_NO_GLYPH_RUN_CACHE`), which
//! resolves every glyph through `glifo` on every frame. Each frame the two
//! compile must be identical — the same draws at the same depths, the same
//! quads, records, slots and strips — and their atlases must hold the same
//! number of entries, which is what pins the cache's LRU bookkeeping to
//! `glifo`'s own: an entry the cache keeps alive that `glifo` would have reaped,
//! or one it lets age out that `glifo` would have kept, shows up as a residency
//! difference on the frame the eviction pass runs.

#![cfg(not(target_arch = "wasm32"))]

use std::sync::Arc;

use frust_engine::SceneCompiler;
use frust_engine::compile::{CompiledFrame, MAX_RESIDENT_GLYPH_RUNS};
use frust_scene::{FontHandle, Glyph, GlyphRun, Scene, SceneBuilder};
use kurbo::{Affine, Rect};
use peniko::{Blob, Brush, Color, FontData};
use vello_common::encode::EncodedPaint;

/// Noto Sans, subsetted to Latin plus combining marks — the bundled face the
/// crate's text cases name glyph ids against.
const LATIN_FONT: &[u8] = include_bytes!("../../../testing/fonts/NotoSans-Subset.ttf");

/// Glyph ids of `Hello` in [`LATIN_FONT`].
const HELLO: [u32; 5] = [5, 6, 7, 7, 8];

/// Glyph ids disjoint from [`HELLO`]'s.
const OTHERS: [u32; 4] = [9, 10, 11, 12];

/// Host extent.
const SIZE: u16 = 256;

/// Frames past the atlas policy's size-change guard (a font first drawn at
/// several sizes reads as a size change and is drawn as outlines for eight
/// frames), plus the frame that rasterizes, plus the frame the cache learns
/// the run on.
const SETTLING_FRAMES: usize = 12;

/// Frames comfortably past two of `glifo`'s eviction passes (one every 64).
const PAST_TWO_EVICTIONS: usize = 140;

/// The Latin face, one process-wide handle — a `peniko::Blob`'s id is what the
/// atlas keys a font on, so two handles would be two fonts.
fn latin_font() -> FontHandle {
    static FONT: std::sync::OnceLock<FontHandle> = std::sync::OnceLock::new();
    FONT.get_or_init(|| FontHandle::new(FontData::new(Blob::new(Arc::new(LATIN_FONT)), 0)))
        .clone()
}

/// A run of `ids` at `size`, its glyphs `advance` apart starting at `x0`.
fn run(ids: &[u32], size: f32, x0: f32, advance: f32, color: Color, transform: Affine) -> GlyphRun {
    GlyphRun {
        font: latin_font(),
        font_size: size,
        brush: Brush::Solid(color),
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

fn scene_of(record: impl FnOnce(&mut SceneBuilder<'_>)) -> Scene {
    let mut scene = Scene::new();
    {
        let mut builder = SceneBuilder::new(&mut scene);
        builder.fill_rect(
            Rect::new(0.0, 0.0, f64::from(SIZE), f64::from(SIZE)),
            Brush::Solid(Color::WHITE),
        );
        record(&mut builder);
    }
    scene
}

fn compiler(cache: bool, hint: bool) -> SceneCompiler {
    let mut compiler = SceneCompiler::new(SIZE, SIZE);
    compiler.set_hint_text(hint);
    compiler.set_glyph_quads(true);
    compiler.set_glyph_run_cache(cache);
    compiler
}

/// Compile `scene` and acknowledge what a serviced frame acknowledges,
/// answering whether the frame left atlas fills to replay — i.e. whether it
/// rasterized anything.
fn compile(compiler: &mut SceneCompiler, scene: &Scene) -> (CompiledFrame, bool) {
    let frame = compiler
        .compile(scene, Affine::IDENTITY, (SIZE, SIZE))
        .expect("an in-range scene compiles");
    let rasterized = compiler.glyph_replay_pending();
    compiler.acknowledge_glyph_clears();
    compiler.acknowledge_glyph_replay();
    compiler.acknowledge_image_plan();
    (frame, rasterized)
}

/// The two arms: one compiler with the cache, one resolving every glyph
/// through `glifo`.
struct Arms {
    cached: SceneCompiler,
    probing: SceneCompiler,
}

impl Arms {
    fn new(hint: bool) -> Self {
        Self {
            cached: compiler(true, hint),
            probing: compiler(false, hint),
        }
    }

    /// Compile `scene` on both arms and require identical frames and identical
    /// atlas residency; answers the cached arm's frame.
    fn frame(&mut self, label: &str, scene: &Scene) -> CompiledFrame {
        let (mut cached, _) = compile(&mut self.cached, scene);
        let (probing, _) = compile(&mut self.probing, scene);
        assert_eq!(
            probing.resident_glyph_runs, 0,
            "{label}: the probing arm never reads the cache"
        );
        let resident = cached.resident_glyph_runs;
        cached.resident_glyph_runs = 0;
        assert_eq!(
            format!("{cached:?}"),
            format!("{probing:?}"),
            "{label}: the two arms compiled different frames"
        );
        assert_eq!(
            self.cached.glyph_atlas_entries(),
            self.probing.glyph_atlas_entries(),
            "{label}: the two arms hold different atlas residency"
        );
        cached.resident_glyph_runs = resident;
        cached
    }
}

/// Runs at every sixteenth of a pixel of horizontal offset — every subpixel
/// bin `glifo` quantizes into, four times over — at three sizes, each moved by
/// `shift` and scaled by `scale`.
fn runs_scene(shift: (f64, f64), scale: f64, color: Color) -> Scene {
    scene_of(|builder| {
        for step in 0..16_u32 {
            let fraction = step as f32 / 16.0;
            let size = [14.0, 18.5, 23.0][step as usize % 3];
            let baseline = 14.0 + f64::from(step) * 14.5;
            builder.draw_glyph_run(run(
                &HELLO,
                size,
                3.0 + fraction,
                11.0 + fraction,
                color,
                Affine::translate((f64::from(step % 4) * 0.5 + shift.0, baseline + shift.1))
                    * Affine::scale(scale),
            ));
        }
    })
}

/// A run of `ids` at 18 px with its pen at `(x, 30)`.
fn single(ids: &[u32], x: f64, size: f32, color: Color) -> Scene {
    scene_of(|builder| {
        builder.draw_glyph_run(run(
            ids,
            size,
            0.25,
            12.5,
            color,
            Affine::translate((x, 30.0)),
        ));
    })
}

/// The tint of the first record a frame's glyph quads share.
fn record_tint(frame: &CompiledFrame) -> Option<Color> {
    let index = *frame.glyph_run_paints.first()?;
    match &frame.encoded_paints[index] {
        EncodedPaint::Image(image) => image.tint.map(|tint| tint.color),
        _ => None,
    }
}

#[test]
fn a_repeated_run_is_drawn_from_the_cache_exactly_as_glifo_draws_it() {
    for hint in [false, true] {
        let mut arms = Arms::new(hint);
        let scene = runs_scene((0.0, 0.0), 1.0, Color::BLACK);
        let mut last = 0;
        for index in 0..SETTLING_FRAMES + 4 {
            last = arms
                .frame(&format!("hint {hint}, frame {index}"), &scene)
                .resident_glyph_runs;
        }
        assert_eq!(
            last, 16,
            "hint {hint}: every run of a settled frame is resident"
        );
    }
}

#[test]
fn a_moving_run_is_placed_and_bucketed_where_glifo_places_it() {
    for hint in [false, true] {
        for scale in [1.0, 1.5, 2.25] {
            let mut arms = Arms::new(hint);
            let mut resident = 0;
            for index in 0..SETTLING_FRAMES + 48 {
                // Irrational-ish steps, so the glyphs walk through every bucket
                // and every whole-pixel boundary in both directions.
                let step = index as f64;
                let shift = ((step * 0.3713).sin() * 6.0, (step * 0.2171).cos() * 3.0);
                let frame = arms.frame(
                    &format!("hint {hint}, scale {scale}, frame {index}"),
                    &runs_scene(shift, scale, Color::BLACK),
                );
                resident += frame.resident_glyph_runs;
            }
            assert!(
                resident > 0,
                "hint {hint}, scale {scale}: a moving run is still served once its buckets are known"
            );
        }
    }
}

#[test]
fn a_whole_pixel_move_hits_and_moves_every_quad_by_the_same_amount() {
    let mut arms = Arms::new(false);
    let still = single(&HELLO, 20.0, 18.0, Color::BLACK);
    for index in 0..SETTLING_FRAMES {
        arms.frame(&format!("frame {index}"), &still);
    }
    let before = arms.frame("before", &still);
    let after = arms.frame("after", &single(&HELLO, 27.0, 18.0, Color::BLACK));
    assert_eq!(before.resident_glyph_runs, 1);
    assert_eq!(
        after.resident_glyph_runs, 1,
        "a translation is not a new run"
    );
    assert_eq!(before.glyph_quads.len(), after.glyph_quads.len());
    for (a, b) in before.glyph_quads.iter().zip(&after.glyph_quads) {
        assert_eq!(b.x, a.x + 7);
        assert_eq!((b.y, b.width, b.height), (a.y, a.width, a.height));
        assert_eq!((b.atlas_x, b.atlas_y), (a.atlas_x, a.atlas_y));
    }
}

#[test]
fn a_changed_size_or_scale_misses_and_a_changed_colour_hits() {
    let mut arms = Arms::new(false);
    let black = single(&HELLO, 20.0, 18.0, Color::BLACK);
    for index in 0..SETTLING_FRAMES {
        arms.frame(&format!("settle {index}"), &black);
    }
    assert_eq!(arms.frame("settled", &black).resident_glyph_runs, 1);

    // The colour is the tint of the run's shared record, never part of a key:
    // the run is the same run, drawn in the new colour.
    let red = Color::from_rgb8(200, 20, 20);
    let recoloured = arms.frame("recoloured", &single(&HELLO, 20.0, 18.0, red));
    assert_eq!(recoloured.resident_glyph_runs, 1, "a colour change hits");
    assert_eq!(record_tint(&recoloured), Some(red));

    // A new size is a new set of keys, so nothing remembered can serve it.
    let larger = arms.frame("larger", &single(&HELLO, 20.0, 20.0, Color::BLACK));
    assert_eq!(larger.resident_glyph_runs, 0, "a size change misses");

    // So is a scale, which `glifo` absorbs into the size.
    let mut arms = Arms::new(false);
    for index in 0..SETTLING_FRAMES {
        arms.frame(&format!("settle {index}"), &black);
    }
    let scaled = scene_of(|builder| {
        builder.draw_glyph_run(run(
            &HELLO,
            18.0,
            0.25,
            12.5,
            Color::BLACK,
            Affine::translate((20.0, 30.0)) * Affine::scale(1.25),
        ));
    });
    assert_eq!(
        arms.frame("scaled", &scaled).resident_glyph_runs,
        0,
        "a scale change misses"
    );
    for index in 0..SETTLING_FRAMES {
        arms.frame(&format!("scaled {index}"), &scaled);
    }
    assert_eq!(
        arms.frame("scaled, settled", &scaled).resident_glyph_runs,
        1,
        "and is remembered once it settles"
    );
}

#[test]
fn an_eviction_invalidates_the_slots_it_reaped() {
    let mut arms = Arms::new(false);
    let hello = single(&HELLO, 20.0, 18.0, Color::BLACK);
    let others = single(&OTHERS, 20.0, 18.0, Color::BLACK);
    for index in 0..SETTLING_FRAMES {
        arms.frame(&format!("hello {index}"), &hello);
    }
    assert_eq!(arms.frame("hello, settled", &hello).resident_glyph_runs, 1);
    let with_hello = arms.cached.glyph_atlas_entries();

    for index in 0..PAST_TWO_EVICTIONS {
        arms.frame(&format!("others {index}"), &others);
    }
    assert!(
        arms.cached.glyph_atlas_entries() < with_hello + OTHERS.len(),
        "the eviction passes reaped the run that left the screen"
    );

    // Its slots went back to the allocator, so the remembered ones are void:
    // the run misses, and is rasterized again exactly as `glifo` would.
    let (back, rasterized) = compile(&mut arms.cached, &hello);
    let (probing, probing_rasterized) = compile(&mut arms.probing, &hello);
    assert_eq!(
        back.resident_glyph_runs, 0,
        "a reaped run is not drawn from the cache"
    );
    assert!(
        rasterized && probing_rasterized,
        "both arms rasterize it again"
    );
    let mut back = back;
    back.resident_glyph_runs = 0;
    assert_eq!(format!("{back:?}"), format!("{probing:?}"));

    assert_eq!(
        arms.frame("hello, again", &hello).resident_glyph_runs,
        1,
        "and is remembered again once resident"
    );
}

#[test]
fn the_atlas_lru_still_sees_a_resident_run() {
    // `HELLO` is drawn every frame, out of the cache once it settles; `OTHERS`
    // only while settling. Across several eviction passes the two arms must
    // reap exactly the same entries frame for frame — the resident run is
    // never evicted ahead of the stale one.
    let mut arms = Arms::new(false);
    let both = scene_of(|builder| {
        builder.draw_glyph_run(run(
            &HELLO,
            18.0,
            0.25,
            12.5,
            Color::BLACK,
            Affine::translate((20.0, 30.0)),
        ));
        builder.draw_glyph_run(run(
            &OTHERS,
            18.0,
            0.25,
            12.5,
            Color::BLACK,
            Affine::translate((20.0, 80.0)),
        ));
    });
    let hello = single(&HELLO, 20.0, 18.0, Color::BLACK);
    for index in 0..SETTLING_FRAMES {
        arms.frame(&format!("both {index}"), &both);
    }
    let mut resident = 0;
    for index in 0..3 * PAST_TWO_EVICTIONS {
        resident += arms
            .frame(&format!("hello {index}"), &hello)
            .resident_glyph_runs;
    }
    assert_eq!(
        resident,
        3 * PAST_TWO_EVICTIONS as u32,
        "the resident run was served from the cache on every frame"
    );

    // The resident run's glyphs are all still there; the stale run's are gone.
    let (frame, rasterized) = compile(&mut arms.cached, &hello);
    assert_eq!(frame.resident_glyph_runs, 1);
    assert!(!rasterized, "nothing of the resident run was reaped");
    let (_, rasterized) = compile(&mut arms.cached, &single(&OTHERS, 20.0, 18.0, Color::BLACK));
    assert!(rasterized, "the stale run was reaped");
}

#[test]
fn the_bound_forgets_the_least_recently_drawn_runs() {
    let mut compiler = compiler(true, false);
    let mut serial = 0_u32;
    let mut latest = Vec::new();
    for frame in 0..30 {
        let scene = scene_of(|builder| {
            for slot in 0..100_u32 {
                // A sequence of four ids no other run in the test repeats.
                let ids: Vec<u32> = (0..4)
                    .map(|digit| 1 + serial / 21_u32.pow(digit) % 21)
                    .collect();
                serial += 1;
                latest = ids.clone();
                builder.draw_glyph_run(run(
                    &ids,
                    14.0,
                    0.0,
                    9.0,
                    Color::BLACK,
                    Affine::translate((
                        f64::from(slot % 5) * 50.0,
                        12.0 + f64::from(slot / 5) * 12.0,
                    )),
                ));
            }
        });
        compile(&mut compiler, &scene);
        assert!(
            compiler.resident_glyph_run_count() <= MAX_RESIDENT_GLYPH_RUNS,
            "frame {frame}: {} runs remembered",
            compiler.resident_glyph_run_count()
        );
    }
    assert!(
        compiler.resident_glyph_run_count() >= 100,
        "the runs of the frame just drawn are kept"
    );

    // The most recently drawn run survived the flood and is served from the
    // cache on the next frame.
    let last = scene_of(|builder| {
        builder.draw_glyph_run(run(
            &latest,
            14.0,
            0.0,
            9.0,
            Color::BLACK,
            Affine::translate((200.0, 240.0)),
        ));
    });
    let (frame, _) = compile(&mut compiler, &last);
    assert_eq!(frame.resident_glyph_runs, 1);
}

#[test]
fn the_kill_switch_restores_per_glyph_probing() {
    let mut arms = Arms::new(false);
    let scene = runs_scene((0.0, 0.0), 1.0, Color::BLACK);
    for index in 0..SETTLING_FRAMES {
        arms.frame(&format!("frame {index}"), &scene);
    }
    assert!(arms.cached.glyph_run_cache_enabled());
    assert!(arms.frame("cached", &scene).resident_glyph_runs > 0);

    arms.cached.set_glyph_run_cache(false);
    assert!(!arms.cached.glyph_run_cache_enabled());
    for index in 0..4 {
        assert_eq!(
            arms.frame(&format!("switched off {index}"), &scene)
                .resident_glyph_runs,
            0,
            "every run goes back through `glifo`"
        );
    }

    // The cache serves quads only: with quads off it is never read either.
    arms.cached.set_glyph_run_cache(true);
    arms.cached.set_glyph_quads(false);
    arms.probing.set_glyph_quads(false);
    for index in 0..4 {
        assert_eq!(
            arms.frame(&format!("quads off {index}"), &scene)
                .resident_glyph_runs,
            0
        );
    }
}
