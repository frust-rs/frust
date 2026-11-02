//! A glyph run carrying `.notdef` (glyph 0) is drawn like any other run: once
//! its first frame has rasterized it into the atlas, every later frame draws it
//! out of the resident-run cache without handing it to `glifo` — no glyph is
//! probed, no outline fetched, nothing rasterized again.
//!
//! These cases were written to answer one device measurement: on an iPhone 13
//! mini (iOS 27.2), whose Han text shapes to the system face's `.notdef`, the
//! S6 multi-script paragraph spends ~4.6 ms of a ~4.9 ms encode window inside
//! glyph runs, with `atlas_glyphs=0`, ~7,460 strips and ~571 glyph misses and
//! ~571 evictions per 60-frame `frust-perf enc` window. The first case shows
//! the `.notdef` glyph is not the cause: a run carrying it becomes resident
//! from its second frame exactly as a run of real glyphs does.
//!
//! The second case pins the cure for the cause the device numbers *do* match.
//! The glyph-atlas policy rations glyph residency to half of the mobile atlas —
//! `1024 x 1024 x 4` texels, so a 2,097,152-texel budget — and prices each
//! entry at its device size's em square: `18 * 18 * (size / 16)^2`, or about
//! 3,690 texels at the 54 px an 18 px run is drawn at under a 3x root. That is
//! room for ~568 entries. A paragraph whose glyphs need more than that is
//! admitted whole on its first frame (each run is charged its distinct glyph
//! ids, not ids times the subpixel buckets they land in), and from then on the
//! resident population alone is over the budget. The policy used to route every
//! run to outlines from there — including runs whose every glyph was already
//! resident — so nothing probed the unused entries, the LRU reaped them, and
//! the next frame admitted and rasterized the whole paragraph again: a thrash
//! cycle, `glyph_misses` and `glyph_evicted` matching each other window after
//! window. Admission is now hysteretic: the budget refuses only runs that would
//! *insert*, so the over-budget paragraph stays resident across eviction passes,
//! while a new run arriving on top of it is refused until the paragraph leaves
//! and the LRU gives its space back. The same scene at 2x (the iPhone SE) fits
//! outright and stays resident, as it always did.

#![cfg(not(target_arch = "wasm32"))]

use std::sync::{Arc, OnceLock};

use frust_engine::compile::CompiledFrame;
use frust_engine::{AtlasBudget, SceneCompiler};
use frust_scene::{FontHandle, Glyph, GlyphRun, Scene, SceneBuilder};
use kurbo::{Affine, Rect};
use peniko::{Blob, Brush, Color, FontData};

/// Noto Sans, subsetted to Latin plus combining marks. Its glyph 0 is the usual
/// boxed `.notdef` — two contours, real ink — which is the shape of glyph the
/// device's Han text fell back to.
const LATIN_FONT: &[u8] = include_bytes!("../../../testing/fonts/NotoSans-Subset.ttf");

/// How many glyph ids [`LATIN_FONT`] carries (`maxp`), `.notdef` included.
const GLYPHS_IN_FONT: u32 = 22;

/// `.notdef`.
const NOTDEF: u32 = 0;

/// Glyph ids of `Hello` in [`LATIN_FONT`] — the control run.
const HELLO: [u32; 5] = [5, 6, 7, 7, 8];

/// Real glyphs with `.notdef` between them, as a Latin word next to an
/// unsupported ideograph shapes in one face.
const MIXED: [u32; 5] = [5, NOTDEF, NOTDEF, 6, NOTDEF];

/// Nothing but `.notdef`, as a line of unsupported Han does.
const ONLY_NOTDEF: [u32; 4] = [NOTDEF; 4];

/// Frames each steady-state case is compiled for.
const FRAMES: usize = 8;

/// Host extent of the small cases.
const SIZE: u16 = 256;

/// Device extent of the paragraph cases.
const PHONE: u16 = 1024;

/// The paragraph's font size before the root scale.
const PARAGRAPH_SIZE: f32 = 18.0;

/// Separate faces the over-budget paragraph is drawn in — each its own blob, so
/// its own font identity in the atlas — and the most that still fit at 3x.
const OVER_BUDGET_FACES: usize = 8;
const UNDER_BUDGET_FACES: usize = 7;

/// The face the new run on top of the over-budget paragraph is drawn in: one
/// past the paragraph's, so none of its glyphs is resident when it arrives.
const NEW_RUN_FACE: usize = OVER_BUDGET_FACES;

/// Glyphs in the new run drawn on top of the over-budget paragraph.
const NEW_RUN_GLYPHS: u32 = 100;

/// The mobile glyph budget: half of the `1024 x 1024 x 4` atlas array's texels
/// (the policy's `glyph_texel_budget` over `AtlasBudget::MOBILE`).
const MOBILE_GLYPH_BUDGET: u64 = 1024 * 1024 * 4 / 2;

/// What the policy charges one entry at the 54 px device size the paragraph is
/// keyed at under a 3x root: `18 * 18 * (54 / 16)^2`, truncated (the policy's
/// `glyph_texels_at(54.0)`).
const TEXELS_AT_54_PX: u64 = 3_690;

/// Frames past two of `glifo`'s eviction passes (one every 64 frames), so a
/// reap and the re-admission after it both fall inside the window.
const PAST_TWO_EVICTIONS: usize = 140;

/// The Latin face, one process-wide handle — a `peniko::Blob`'s id is what the
/// atlas keys a font on, so two handles would be two fonts.
fn latin_font() -> FontHandle {
    static FONT: OnceLock<FontHandle> = OnceLock::new();
    FONT.get_or_init(|| FontHandle::new(FontData::new(Blob::new(Arc::new(LATIN_FONT)), 0)))
        .clone()
}

/// The `index`th of several copies of the Latin face, each a blob of its own and
/// therefore a font of its own to the atlas.
fn face(index: usize) -> FontHandle {
    static FACES: OnceLock<Vec<FontHandle>> = OnceLock::new();
    FACES.get_or_init(|| {
        (0..=NEW_RUN_FACE)
            .map(|_| FontHandle::new(FontData::new(Blob::new(Arc::new(LATIN_FONT.to_vec())), 0)))
            .collect()
    })[index]
        .clone()
}

/// A white backdrop with whatever `record` draws over it.
fn scene_of(extent: u16, record: impl FnOnce(&mut SceneBuilder<'_>)) -> Scene {
    let mut scene = Scene::new();
    {
        let mut builder = SceneBuilder::new(&mut scene);
        let extent = f64::from(extent);
        builder.fill_rect(
            Rect::new(0.0, 0.0, extent, extent),
            Brush::Solid(Color::WHITE),
        );
        record(&mut builder);
    }
    scene
}

/// One 18 px black run of `ids`, its pen off the pixel grid.
fn line(ids: &[u32]) -> Scene {
    scene_of(SIZE, |builder| {
        builder.draw_glyph_run(GlyphRun {
            font: latin_font(),
            font_size: 18.0,
            brush: Brush::Solid(Color::BLACK),
            transform: Affine::translate((3.25, 40.0)),
            glyphs: ids
                .iter()
                .enumerate()
                .map(|(index, id)| Glyph {
                    id: *id,
                    x: index as f32 * 11.3,
                    y: 0.0,
                })
                .collect(),
        });
    })
}

/// A paragraph of `faces` runs at [`PARAGRAPH_SIZE`] under a `root_scale` root,
/// each run carrying every glyph of its face — `.notdef` included — at all four
/// of `glifo`'s subpixel buckets, so each face asks for up to
/// `4 * GLYPHS_IN_FONT` atlas entries.
fn paragraph(faces: usize, root_scale: f64) -> Scene {
    scene_of(extent_at(root_scale), |builder| {
        draw_paragraph(builder, faces, root_scale);
    })
}

/// The host extent a [`PHONE`]-wide device covers under a `root_scale` root.
fn extent_at(root_scale: f64) -> u16 {
    (f64::from(PHONE) / root_scale) as u16
}

/// One run of `glyphs` glyphs of [`face`] `index` at [`PARAGRAPH_SIZE`] on line
/// `line`: every glyph id of the face in turn, each at all four subpixel
/// buckets before the next, ten device pixels apart and offset by the bucket's
/// quarter — wrapping back to the first id past `4 * GLYPHS_IN_FONT`.
fn face_run(index: usize, line: usize, glyphs: u32, root_scale: f64) -> GlyphRun {
    GlyphRun {
        font: face(index),
        font_size: PARAGRAPH_SIZE,
        brush: Brush::Solid(Color::BLACK),
        transform: Affine::translate((2.0, 20.0 + 18.0 * line as f64)),
        glyphs: (0..glyphs)
            .map(|slot| {
                let id = (slot / 4) % GLYPHS_IN_FONT;
                let bucket = slot % 4;
                let device_x = 10.0 * f64::from(slot) + f64::from(bucket) / 4.0;
                Glyph {
                    id,
                    x: (device_x / root_scale) as f32,
                    y: 0.0,
                }
            })
            .collect(),
    }
}

/// [`paragraph`]'s runs, recorded into `builder`.
fn draw_paragraph(builder: &mut SceneBuilder<'_>, faces: usize, root_scale: f64) {
    for index in 0..faces {
        builder.draw_glyph_run(face_run(index, index, 4 * GLYPHS_IN_FONT, root_scale));
    }
}

/// The [`NEW_RUN_GLYPHS`]-glyph run of [`NEW_RUN_FACE`], on the line below the
/// over-budget paragraph.
fn draw_new_run(builder: &mut SceneBuilder<'_>, root_scale: f64) {
    builder.draw_glyph_run(face_run(
        NEW_RUN_FACE,
        OVER_BUDGET_FACES,
        NEW_RUN_GLYPHS,
        root_scale,
    ));
}

/// A compiler with glyph quads and the resident-run cache on, as a phone
/// renderer runs them.
fn compiler(extent: u16, hint: bool) -> SceneCompiler {
    let mut compiler = SceneCompiler::with_atlas_budget(extent, extent, AtlasBudget::MOBILE);
    compiler.set_hint_text(hint);
    compiler.set_glyph_quads(true);
    compiler.set_glyph_run_cache(true);
    compiler
}

/// What one compiled frame cost the text path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TextCost {
    /// Runs drawn out of the resident-run cache, without `glifo`.
    resident_runs: u32,
    /// Glyphs that produced a draw.
    glyph_draws: u32,
    /// Of those, the ones sampled from the atlas.
    atlas_draws: u32,
    /// Atlas lookups `glifo` (or the resident cache's probe) made that hit.
    atlas_hits: u64,
    /// Lookups that missed — every outline fetch or rasterization starts with
    /// one.
    atlas_misses: u64,
    /// Entries resident after the frame.
    entries: usize,
    /// Whether the frame recorded fills that rasterize new glyphs.
    rasterized: bool,
    /// Strips generated — the per-frame cost of glyphs drawn as outlines.
    strips: usize,
}

/// Compile `scene` under `root` and acknowledge what a serviced frame
/// acknowledges, answering what the text path cost.
fn compile(compiler: &mut SceneCompiler, scene: &Scene, root: Affine, extent: u16) -> TextCost {
    let hits = compiler.glyph_atlas_mut().cache_hits();
    let misses = compiler.glyph_atlas_mut().cache_misses();
    let frame: CompiledFrame = compiler
        .compile(scene, root, (extent, extent))
        .expect("an in-range scene compiles");
    let rasterized = compiler.glyph_replay_pending();
    compiler.acknowledge_glyph_clears();
    compiler.acknowledge_glyph_replay();
    compiler.acknowledge_image_plan();
    TextCost {
        resident_runs: frame.resident_glyph_runs,
        glyph_draws: frame.glyph_draws,
        atlas_draws: frame.atlas_glyph_draws,
        atlas_hits: compiler.glyph_atlas_mut().cache_hits() - hits,
        atlas_misses: compiler.glyph_atlas_mut().cache_misses() - misses,
        entries: compiler.glyph_atlas_entries(),
        rasterized,
        strips: frame.strip_buf().len(),
    }
}

/// `FRAMES` frames of `ids` on a fresh compiler.
fn frames_of(ids: &[u32], hint: bool) -> Vec<TextCost> {
    let mut compiler = compiler(SIZE, hint);
    let scene = line(ids);
    (0..FRAMES)
        .map(|_| compile(&mut compiler, &scene, Affine::IDENTITY, SIZE))
        .collect()
}

#[test]
fn a_notdef_bearing_run_is_resident_from_its_second_frame() {
    for hint in [false, true] {
        let control = frames_of(&HELLO, hint);
        for (label, ids) in [("mixed", &MIXED[..]), ("only .notdef", &ONLY_NOTDEF[..])] {
            let costs = frames_of(ids, hint);
            let glyphs = u32::try_from(ids.len()).expect("a short run");
            let first = costs[0];

            // Frame one: `.notdef` has ink, so every glyph is a draw, and the
            // atlas served every one of them (rasterizing them on the way).
            assert_eq!(first.glyph_draws, glyphs, "{label} hint={hint}: frame 1");
            assert_eq!(first.atlas_draws, glyphs, "{label} hint={hint}: frame 1");
            assert!(first.rasterized, "{label} hint={hint}: frame 1 rasterizes");
            assert!(
                first.atlas_misses > 0,
                "{label} hint={hint}: frame 1 misses"
            );
            assert_eq!(first.resident_runs, 0, "{label} hint={hint}: frame 1");

            for (index, cost) in costs.iter().enumerate().skip(1) {
                let frame = index + 1;
                assert_eq!(
                    cost.resident_runs, 1,
                    "{label} hint={hint}: frame {frame} draws the run from the resident cache"
                );
                assert_eq!(
                    cost.glyph_draws, glyphs,
                    "{label} hint={hint}: frame {frame}"
                );
                assert_eq!(
                    cost.atlas_draws, glyphs,
                    "{label} hint={hint}: frame {frame}"
                );
                assert_eq!(
                    cost.atlas_misses, 0,
                    "{label} hint={hint}: frame {frame} fetches no outline"
                );
                assert!(
                    !cost.rasterized,
                    "{label} hint={hint}: frame {frame} rasterizes nothing"
                );
                assert_eq!(
                    cost.entries, first.entries,
                    "{label} hint={hint}: frame {frame} holds what frame 1 inserted"
                );
                // The resident cache answered alone: no lookup reached the atlas
                // after frame two's probe found the slots, so `glifo` walked
                // nothing.
                if frame > 2 {
                    assert_eq!(
                        cost.atlas_hits, 0,
                        "{label} hint={hint}: frame {frame} reaches `glifo` for no glyph"
                    );
                }
                // The control run's frames read the same, counter for counter,
                // apart from the glyph counts the two runs differ in.
                let same = control[index];
                assert_eq!(
                    (
                        cost.resident_runs,
                        cost.atlas_misses,
                        cost.rasterized,
                        cost.strips
                    ),
                    (
                        same.resident_runs,
                        same.atlas_misses,
                        same.rasterized,
                        same.strips
                    ),
                    "{label} hint={hint}: frame {frame} costs what the control run costs"
                );
            }
        }
    }
}

/// [`PAST_TWO_EVICTIONS`] frames of a `faces`-face paragraph under a
/// `root_scale` root on the mobile atlas.
fn paragraph_frames(faces: usize, root_scale: f64) -> Vec<TextCost> {
    let mut compiler = compiler(PHONE, false);
    let scene = paragraph(faces, root_scale);
    let root = Affine::scale(root_scale);
    (0..PAST_TWO_EVICTIONS)
        .map(|_| compile(&mut compiler, &scene, root, PHONE))
        .collect()
}

/// Every frame after the first draws every run out of the resident cache.
fn assert_resident_after_first(label: &str, faces: usize, costs: &[TextCost]) {
    let runs = u32::try_from(faces).expect("a handful of faces");
    for (index, cost) in costs.iter().enumerate().skip(1) {
        let frame = index + 1;
        assert_eq!(cost.resident_runs, runs, "{label}: frame {frame}");
        assert_eq!(cost.atlas_draws, cost.glyph_draws, "{label}: frame {frame}");
        assert!(
            !cost.rasterized,
            "{label}: frame {frame} rasterizes nothing"
        );
    }
}

#[test]
fn a_paragraph_over_the_mobile_glyph_budget_at_3x_stays_resident_and_refuses_only_new_glyphs() {
    // The old cliff's two sides, which fit the budget outright and were always
    // resident: seven faces at 3x, and the same paragraph at 2x with eight.
    assert_resident_after_first(
        "seven faces at 3x",
        UNDER_BUDGET_FACES,
        &paragraph_frames(UNDER_BUDGET_FACES, 3.0),
    );
    assert_resident_after_first(
        "eight faces at 2x",
        OVER_BUDGET_FACES,
        &paragraph_frames(OVER_BUDGET_FACES, 2.0),
    );

    // Eight faces at 3x: admitted and rasterized whole on frame one, which
    // leaves the resident population over the budget.
    let mut compiler = compiler(PHONE, false);
    let root = Affine::scale(3.0);
    let paragraph = paragraph(OVER_BUDGET_FACES, 3.0);
    let costs: Vec<TextCost> = (0..PAST_TWO_EVICTIONS)
        .map(|_| compile(&mut compiler, &paragraph, root, PHONE))
        .collect();
    let first = costs[0];
    assert!(first.rasterized, "frame 1 rasterizes the paragraph");
    assert_eq!(first.atlas_draws, first.glyph_draws, "frame 1 is all atlas");
    let entries = u64::try_from(first.entries).expect("a few hundred");
    assert!(
        entries * TEXELS_AT_54_PX > MOBILE_GLYPH_BUDGET,
        "fixture precondition: {entries} entries at 54 px charge {} texels, over \
         the {MOBILE_GLYPH_BUDGET}-texel mobile budget",
        entries * TEXELS_AT_54_PX
    );

    // Every later frame — across both eviction passes inside the window — is
    // served from the atlas out of the resident-run cache: nothing rasterized,
    // nothing reaped and re-inserted. The cycle the device line showed
    // (`atlas_glyphs=0`, misses matching evictions) is gone.
    for (index, cost) in costs.iter().enumerate().skip(1) {
        let frame = index + 1;
        let runs = u32::try_from(OVER_BUDGET_FACES).expect("a handful");
        assert_eq!(
            cost.resident_runs, runs,
            "frame {frame}: every run resident"
        );
        assert_eq!(
            cost.atlas_draws, cost.glyph_draws,
            "frame {frame}: every glyph sampled from the atlas"
        );
        assert!(!cost.rasterized, "frame {frame}: nothing rasterized again");
        assert_eq!(
            cost.entries, first.entries,
            "frame {frame}: the over-budget population is kept, not reaped"
        );
    }
    // And it costs, frame for frame, exactly what the same paragraph costs on
    // the desktop budget it fits outright: the same hits and misses — the only
    // misses after frame one are the eviction pass's sibling-bucket probes,
    // which look up and insert nothing, on both budgets alike — the same
    // entries and the same strips. Over the budget is no longer a different
    // regime for text that is already resident.
    let mut fits = SceneCompiler::with_atlas_budget(PHONE, PHONE, AtlasBudget::DESKTOP);
    fits.set_hint_text(false);
    fits.set_glyph_quads(true);
    fits.set_glyph_run_cache(true);
    for (index, cost) in costs.iter().enumerate() {
        let control = compile(&mut fits, &paragraph, root, PHONE);
        assert_eq!(
            *cost,
            control,
            "frame {}: the over-budget paragraph costs what it costs where it fits",
            index + 1
        );
    }
    // Frame two is where the population is first over the budget as a frame
    // opens: the residency question is answered by atlas hits, one per glyph
    // drawn, and the resident-run cache keeps the slots it found so no later
    // frame reaches `glifo` at all.
    assert!(
        costs[1].atlas_hits >= u64::from(costs[1].glyph_draws),
        "frame 2 proves the paragraph resident by hits ({} hits, {} glyphs)",
        costs[1].atlas_hits,
        costs[1].glyph_draws
    );
    assert!(
        costs[1].strips <= costs_of_resident_strips(),
        "frame 2 generates no outline strips ({} strips)",
        costs[1].strips
    );

    // A new run arriving on top would insert, so it is refused — drawn as
    // outlines — while the paragraph holds the budget, and the paragraph stays
    // served the whole time.
    let with_new_run = scene_of(extent_at(3.0), |builder| {
        draw_paragraph(builder, OVER_BUDGET_FACES, 3.0);
        draw_new_run(builder, 3.0);
    });
    for frame in 0..8 {
        let cost = compile(&mut compiler, &with_new_run, root, PHONE);
        assert_eq!(
            cost.atlas_draws, first.atlas_draws,
            "new-run frame {frame}: only the paragraph samples the atlas"
        );
        assert!(
            cost.glyph_draws > cost.atlas_draws,
            "new-run frame {frame}: the new run still draws, as outlines"
        );
        assert!(!cost.rasterized, "new-run frame {frame}: nothing inserted");
        assert_eq!(cost.entries, first.entries, "new-run frame {frame}");
    }

    // The paragraph leaves. The new run stays refused until the LRU has reaped
    // the paragraph's entries — space is freed by eviction, never by refusing a
    // run that is still on screen — and is admitted on the frame after.
    let new_run_alone = scene_of(extent_at(3.0), |builder| draw_new_run(builder, 3.0));
    let alone: Vec<TextCost> = (0..PAST_TWO_EVICTIONS)
        .map(|_| compile(&mut compiler, &new_run_alone, root, PHONE))
        .collect();
    let reap = alone
        .iter()
        .position(|cost| cost.entries == 0)
        .expect("the eviction pass reaps the departed paragraph inside two passes");
    for (frame, cost) in alone.iter().enumerate().take(reap + 1) {
        assert_eq!(
            cost.atlas_draws, 0,
            "alone frame {frame}: refused while the paragraph's entries hold the budget"
        );
    }
    let admitted = alone[reap + 1];
    assert!(
        admitted.rasterized,
        "the frame after the reap rasterizes the new run"
    );
    assert_eq!(
        admitted.atlas_draws, admitted.glyph_draws,
        "and draws it from the atlas"
    );
    let resident = alone[reap + 2];
    assert_eq!(resident.resident_runs, 1, "from then on it is resident");
    assert_eq!(resident.atlas_misses, 0);
}

/// The strips a frame of fully resident runs generates: the backdrop's, since
/// a glyph quad outside an isolated layer generates none.
fn costs_of_resident_strips() -> usize {
    let mut compiler = compiler(PHONE, false);
    let scene = paragraph(0, 3.0);
    compile(&mut compiler, &scene, Affine::scale(3.0), PHONE)
        .strips
        .max(1)
}
