//! Glyphs of an `hvgl`-only face (Apple's PingFang) through the glyph atlas.
//!
//! `hvgl_provider.rs` pins the outline route (CoreText's own outlines, filled
//! every frame). This file pins the route the engine takes by default: the
//! frust-side insert that rasterizes the same outlines once into atlas
//! entries of their own (`text/hvgl_atlas.rs`), after which they are served
//! like any `glifo` entry: resident runs, glyph quads, eviction, clears and
//! the page replay.
//!
//! What is pinned, each against the compiler's public counters (the policy's
//! `AtlasPass::no_room` and `unrasterizable` are crate-private, so a refused
//! insert and an inkless glyph are told apart by what they do to
//! `hvgl_outline_fallbacks`, `glyph_draws` and the entry map):
//!
//! - **The route.** A PingFang run is drawn from the atlas on the second frame
//!   with no outline fallback, and is a resident run on the third.
//! - **Eviction and clears.** hvgl entries age out like any other, their
//!   rectangles come back as clears exactly once, and the cycle repeats.
//! - **The budget.** hvgl entries are charged to the glyph texel budget: an
//!   over-budget Han paragraph keeps being served while a run that would
//!   insert is refused to outlines.
//! - **A full atlas.** A glyph the packer has no room for is drawn as an
//!   outline and counted as a fallback, never skipped.
//! - **Inkless glyphs.** A space takes no slot and is not a fallback.
//! - **Brushes.** A gradient cannot tint a coverage mask: outlines.
//! - **Keys.** Unhinted whatever `hint_text` says, carrying the run's
//!   variation coordinates (so weights 300 and 600 are different entries).
//! - **The switch.** `set_hvgl_atlas(false)` restores outline-every-frame.
//!
//! Every case needs macOS's PingFang and CoreText. Elsewhere (and on a Mac
//! where `TextContext` does not resolve PingFang SC to an identified `hvgl`
//! face) it prints one `SKIPPED (n): <precondition>` line and passes, so a
//! Linux run compiles the whole file and reports what it did not cover.
//! The Metal cases are `#[ignore]`d; run them with
//! `cargo test -p frust-engine --test hvgl_atlas -- --ignored --test-threads=1`.

use std::sync::atomic::{AtomicUsize, Ordering};

use frust_engine::compile::CompiledFrame;
use frust_engine::{AtlasBudget, SceneCompiler};
use frust_scene::{Glyph, GlyphRun, Scene, SceneBuilder};
use frust_text::{FontFamily, FontWeight, TextContext, TextStyle};
use kurbo::{Affine, Point};
use peniko::color::palette::css::{BLACK, BLUE, RED};
use peniko::{Brush, Color, Gradient};

/// Viewport the cases compile against: wide enough that a 150-glyph paragraph
/// at [`FONT_SIZE`] keeps every glyph's draw on the target.
const VIEWPORT: (u16, u16) = (4096, 64);

/// Font size every case draws at: a whole number, so quantization is the
/// identity.
const FONT_SIZE: f32 = 24.0;

/// Frames a case idles through to be sure `glifo`'s eviction has run: past the
/// entry-age window (64) plus a full eviction period (64).
const IDLE_FRAMES: usize = 64 + 64 + 2;

/// How many cases printed a skip in this process.
static SKIPS: AtomicUsize = AtomicUsize::new(0);

/// Prints the one counted skip line and passes.
fn skip(missing: &str) {
    let n = SKIPS.fetch_add(1, Ordering::Relaxed) + 1;
    println!("SKIPPED ({n}): {missing}");
}

/// `count` distinct Han characters from the CJK Unified Ideographs block's
/// common range, starting `from` characters in.
fn han(from: u32, count: u32) -> String {
    (0x4E00 + from..0x4E00 + from + count)
        .filter_map(char::from_u32)
        .collect()
}

/// `text` laid out in PingFang SC at `weight`, as scene runs on one baseline.
fn laid_out(ctx: &mut TextContext, weight: FontWeight, text: &str) -> Vec<GlyphRun> {
    let mut style = TextStyle::new(FONT_SIZE, Color::BLACK);
    style.family = FontFamily::named("PingFang SC");
    style.weight = weight;
    ctx.layout(text, &style, None)
        .to_scene_runs(Point::new(8.0, 8.0))
}

/// A `TextContext` that resolves PingFang SC to an identified `hvgl` face, or
/// the printed skip.
fn pingfang() -> Option<TextContext> {
    if !cfg!(target_os = "macos") {
        skip("needs macOS's PingFang and CoreText");
        return None;
    }
    let mut ctx = TextContext::new();
    let resolved = laid_out(&mut ctx, FontWeight::REGULAR, "中")
        .iter()
        .all(|run| {
            let font = run.font.font();
            run.font.postscript_name().is_some() && has_hvgl_table(font.data.data(), font.index)
        });
    if !resolved {
        skip("TextContext does not resolve PingFang SC to an identified hvgl face");
        return None;
    }
    Some(ctx)
}

/// Whether the face at `index` of the font file `data` carries an `hvgl` table.
fn has_hvgl_table(data: &[u8], index: u32) -> bool {
    let be_u32 =
        |at: usize| u32::from_be_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]);
    let directory = if data.len() > 16 && &data[0..4] == b"ttcf" {
        be_u32(12 + 4 * index as usize) as usize
    } else {
        0
    };
    let tables = usize::from(u16::from_be_bytes([
        data[directory + 4],
        data[directory + 5],
    ]));
    (0..tables).any(|table| {
        let record = directory + 12 + 16 * table;
        &data[record..record + 4] == b"hvgl"
    })
}

fn scene_of(runs: Vec<GlyphRun>) -> Scene {
    let mut scene = Scene::new();
    let mut builder = SceneBuilder::new(&mut scene);
    for run in runs {
        builder.draw_glyph_run(run);
    }
    scene
}

/// `runs` with every brush replaced by `brush`.
fn with_brush(runs: &[GlyphRun], brush: Brush) -> Vec<GlyphRun> {
    runs.iter()
        .map(|run| GlyphRun {
            brush: brush.clone(),
            ..run.clone()
        })
        .collect()
}

/// A compiler with the hvgl route at its default (on).
fn fresh() -> SceneCompiler {
    SceneCompiler::new(VIEWPORT.0, VIEWPORT.1)
}

/// A compiler over a deliberately small atlas.
fn small_compiler(atlas_size: (u32, u32), max_atlases: usize) -> SceneCompiler {
    SceneCompiler::with_atlas_budget(
        VIEWPORT.0,
        VIEWPORT.1,
        AtlasBudget {
            atlas_size,
            max_atlases,
        },
    )
}

/// Compiles `scene` and acknowledges what a serviced frame acknowledges (the
/// evicted rectangles zeroed, the recorded pages replayed), as
/// `EngineRenderer::encode` does for every frame it does not refuse.
fn serviced(compiler: &mut SceneCompiler, runs: &[GlyphRun]) -> CompiledFrame {
    let frame = compiler
        .compile(&scene_of(runs.to_vec()), Affine::IDENTITY, VIEWPORT)
        .expect("an in-range scene compiles");
    compiler.acknowledge_glyph_clears();
    compiler.acknowledge_glyph_replay();
    frame
}

/// How many atlas pages have recorded rasterization commands outstanding,
/// draining them the way the render-to-atlas pass does.
fn drain_dirty_pages(compiler: &mut SceneCompiler) -> usize {
    let mut pages = 0;
    compiler
        .glyph_atlas_mut()
        .replay_pending_atlas_commands(|_| pages += 1);
    compiler.acknowledge_glyph_replay();
    pages
}

/// Every glyph of `runs`, counted.
fn glyph_count(runs: &[GlyphRun]) -> u32 {
    runs.iter().map(|run| run.glyphs.len() as u32).sum()
}

/// A run of `ids` from `runs[0]`'s face on one baseline.
fn run_of_ids(like: &GlyphRun, ids: &[u32]) -> GlyphRun {
    GlyphRun {
        glyphs: ids
            .iter()
            .enumerate()
            .map(|(index, id)| Glyph {
                id: *id,
                x: index as f32 * FONT_SIZE,
                y: 0.0,
            })
            .collect(),
        transform: Affine::translate((8.0, 44.0)),
        ..like.clone()
    }
}

/// (a) A PingFang run draws from the atlas with no outline fallback — on its
/// first frame, since the insert runs in the collect walk — and is a resident
/// run from the second.
#[test]
fn a_pingfang_run_draws_from_the_atlas_then_is_a_resident_run() {
    let Some(mut ctx) = pingfang() else { return };
    let runs = laid_out(&mut ctx, FontWeight::REGULAR, "中文爱");
    let glyphs = glyph_count(&runs);
    assert_eq!(glyphs, 3);
    let mut compiler = fresh();
    assert!(compiler.hvgl_atlas_enabled(), "the route defaults to on");

    let frames: Vec<CompiledFrame> = (0..3).map(|_| serviced(&mut compiler, &runs)).collect();
    for (index, frame) in frames.iter().enumerate() {
        assert_eq!(frame.glyph_draws, glyphs, "frame {index}");
        assert_eq!(frame.skipped_glyphs, 0, "frame {index}");
        assert_eq!(frame.hvgl_atlas_draws, glyphs, "frame {index}");
        assert_eq!(frame.atlas_glyph_draws, glyphs, "frame {index}");
        assert_eq!(frame.hvgl_outline_fallbacks, 0, "frame {index}");
    }
    assert_eq!(compiler.glyph_atlas_entries(), 3);
    assert_eq!(
        compiler.hvgl_cached_outlines(),
        3,
        "each outline was fetched from CoreText once, to rasterize its entry"
    );
    assert_eq!(frames[0].resident_glyph_runs, 0, "nothing resident yet");
    for (index, frame) in frames.iter().enumerate().skip(1) {
        assert_eq!(frame.resident_glyph_runs, 1, "frame {index}");
        assert_eq!(frame.glyph_quads.len(), glyphs as usize, "frame {index}");
    }
    assert_eq!(compiler.resident_glyph_run_count(), 1);
    assert_eq!(compiler.hvgl_identity_mismatches(), 0);
    assert_eq!(compiler.hvgl_bytes_mismatches(), 0);
}

/// The same property the outline-route test pins in `hvgl_provider.rs`, on the
/// atlas route: Han glyphs are drawn, none skipped, and their rasterization
/// was recorded into a page for the replay.
#[test]
fn pingfang_han_glyphs_are_drawn_from_the_atlas_and_recorded_for_replay() {
    let Some(mut ctx) = pingfang() else { return };
    let runs = laid_out(&mut ctx, FontWeight::REGULAR, "中文爱");
    let mut compiler = fresh();
    let frame = compiler
        .compile(&scene_of(runs), Affine::IDENTITY, VIEWPORT)
        .expect("compiles");
    assert_eq!(frame.hvgl_atlas_draws, 3);
    assert_eq!(frame.skipped_glyphs, 0);
    assert!(compiler.glyph_replay_pending());
    assert_eq!(
        drain_dirty_pages(&mut compiler),
        1,
        "the three entries' fills were recorded into one page"
    );
    assert!(!compiler.glyph_replay_pending());
}

/// Weight rides in the entries' variation coordinates: 中 at 300 and at 600 are
/// different atlas entries (the coverage differs; the Metal case below reads
/// it back), each keyed under an `hvgl` face id (top bit set).
#[test]
fn pingfang_weights_are_distinct_atlas_entries_on_the_atlas_route() {
    let Some(mut ctx) = pingfang() else { return };
    let mut keys_at = |weight: FontWeight| {
        let runs = laid_out(&mut ctx, weight, "中");
        let mut compiler = fresh();
        let frame = serviced(&mut compiler, &runs);
        assert_eq!(frame.hvgl_atlas_draws, 1);
        assert_eq!(frame.hvgl_outline_fallbacks, 0);
        compiler
            .glyph_atlas_mut()
            .all_keys()
            .map(|key| (key.font_id, key.var_coords.to_vec(), key.glyph_id))
            .collect::<Vec<_>>()
    };
    let light = keys_at(FontWeight::LIGHT);
    let semibold = keys_at(FontWeight::SEMI_BOLD);
    assert_eq!(light.len(), 1);
    assert_eq!(semibold.len(), 1);
    assert!(
        light[0].0 >> 63 == 1 && semibold[0].0 >> 63 == 1,
        "an hvgl face id carries the top bit"
    );
    assert!(!light[0].1.is_empty() && !semibold[0].1.is_empty());
    assert_ne!(
        light[0].1, semibold[0].1,
        "weights are different coordinates"
    );
    assert_eq!(light[0].2, semibold[0].2, "the same glyph");
}

/// (b) Eviction and clear-rect cycling with hvgl entries: an idle page is
/// given back, each freed rectangle is reported as a clear exactly once, and
/// the entries come back (rasterized again) on the next draw, cycle after
/// cycle with no growth.
#[test]
fn hvgl_entries_are_evicted_cleared_and_reinserted_cycle_after_cycle() {
    let Some(mut ctx) = pingfang() else { return };
    let runs = laid_out(&mut ctx, FontWeight::REGULAR, &han(0, 40));
    let glyphs = glyph_count(&runs);
    let mut compiler = fresh();
    let empty: Vec<GlyphRun> = Vec::new();

    let mut entries_at_peak = 0;
    for cycle in 0..3 {
        let frame = serviced(&mut compiler, &runs);
        assert_eq!(frame.hvgl_atlas_draws, glyphs, "cycle {cycle}");
        assert_eq!(frame.hvgl_outline_fallbacks, 0, "cycle {cycle}");
        let entries = compiler.glyph_atlas_entries();
        assert_eq!(entries, glyphs as usize, "cycle {cycle}: one entry a glyph");
        if cycle == 0 {
            entries_at_peak = entries;
        }
        assert_eq!(entries, entries_at_peak, "cycle {cycle}: no growth");

        // Idle until the LRU gives the page back; every freed rectangle is
        // reported once, the evicted entry count in total.
        let mut clears = 0;
        let mut idle = 0;
        while compiler.glyph_atlas_entries() > 0 {
            assert!(idle < 4 * IDLE_FRAMES, "cycle {cycle}: eviction never ran");
            clears += serviced(&mut compiler, &empty).glyph_clears.len();
            idle += 1;
        }
        // The rectangles an eviction frees are carried into the *next*
        // frames' clears.
        for _ in 0..3 {
            clears += serviced(&mut compiler, &empty).glyph_clears.len();
        }
        assert_eq!(
            clears, entries,
            "cycle {cycle}: every evicted hvgl rectangle is cleared once"
        );
        assert_eq!(compiler.resident_glyph_run_count(), 0, "cycle {cycle}");
    }
}

/// A run of `ids` laid on a 24 px grid, each id placed once in every one of
/// the four subpixel buckets (a quarter pixel apart): four entries per id
/// while the run is charged one, which is how a run overshoots the budget it
/// was admitted under.
fn every_bucket(like: &GlyphRun, ids: &[u32]) -> GlyphRun {
    let glyphs = ids
        .iter()
        .enumerate()
        .flat_map(|(slot, id)| {
            (0..4).map(move |bucket| Glyph {
                id: *id,
                x: (slot * 4 + bucket) as f32 * FONT_SIZE + bucket as f32 * 0.25,
                y: 0.0,
            })
        })
        .collect();
    GlyphRun {
        glyphs,
        transform: Affine::translate((8.0, 44.0)),
        ..like.clone()
    }
}

/// (c) hvgl entries are charged to the glyph texel budget, as gr-01's Latin
/// case pins for `glifo`'s: an over-budget Han paragraph keeps being served
/// from the atlas (a run that inserts nothing is not what the budget guards)
/// while a run that would insert is refused to outlines.
#[test]
fn an_over_budget_han_paragraph_keeps_serving_and_refuses_only_new_runs() {
    let Some(mut ctx) = pingfang() else { return };
    // Two 256 px layers: the budget's floor (82,944 texels, 113 glyphs at
    // 24 px) is under what the packer holds, so the budget is what refuses.
    let mut compiler = small_compiler((256, 256), 2);
    let shaped = laid_out(&mut ctx, FontWeight::REGULAR, &han(0, 40));
    let ids: Vec<u32> = shaped
        .iter()
        .flat_map(|run| run.glyphs.iter().map(|glyph| glyph.id))
        .collect();
    assert_eq!(ids.len(), 40);
    // Charged 40 typical glyphs (29,160 texels: fits an empty share); inserts
    // 160 entries (116,640 texels: a third over it).
    let paragraph = vec![every_bucket(&shaped[0], &ids)];
    let paragraph_glyphs = glyph_count(&paragraph);
    assert_eq!(paragraph_glyphs, 160);
    let newcomer = laid_out(&mut ctx, FontWeight::REGULAR, &han(200, 12));
    let newcomer_glyphs = glyph_count(&newcomer);

    // Control: with the share empty the newcomer alone is admitted, so what
    // refuses it below is the spent budget and nothing about the run.
    let alone = serviced(&mut small_compiler((256, 256), 2), &newcomer);
    assert_eq!(alone.hvgl_atlas_draws, newcomer_glyphs);
    assert_eq!(alone.hvgl_outline_fallbacks, 0);

    let first = serviced(&mut compiler, &paragraph);
    assert_eq!(first.hvgl_outline_fallbacks, 0);
    assert_eq!(first.hvgl_atlas_draws, paragraph_glyphs);
    let entries = compiler.glyph_atlas_entries();
    assert_eq!(entries, paragraph_glyphs as usize);

    // Three eviction passes with a new run beside it.
    let both: Vec<GlyphRun> = paragraph.iter().chain(&newcomer).cloned().collect();
    for frame_index in 0..(3 * 64) {
        let frame = serviced(&mut compiler, &both);
        assert_eq!(
            frame.hvgl_atlas_draws, paragraph_glyphs,
            "frame {frame_index}: the resident paragraph keeps its atlas draws"
        );
        assert_eq!(
            frame.hvgl_outline_fallbacks, newcomer_glyphs,
            "frame {frame_index}: the run that would insert is refused to outlines"
        );
        assert_eq!(
            frame.glyph_draws,
            paragraph_glyphs + newcomer_glyphs,
            "frame {frame_index}: refused is drawn, not dropped"
        );
        assert_eq!(frame.skipped_glyphs, 0, "frame {frame_index}");
        assert_eq!(
            compiler.glyph_atlas_entries(),
            entries,
            "frame {frame_index}: nothing reaped and nothing inserted"
        );
    }
}

/// (d) The atlas has no room: the glyphs it cannot place are drawn as outlines
/// and counted as fallbacks. This is `AtlasPass::no_room`, not
/// `unrasterizable`: every glyph here has ink, and none is skipped or lost.
#[test]
fn a_full_atlas_falls_back_to_outlines_and_counts_them() {
    let Some(mut ctx) = pingfang() else { return };
    // One 64 px layer holds a handful of 24 px entries.
    let mut compiler = small_compiler((64, 64), 1);
    let runs = laid_out(&mut ctx, FontWeight::REGULAR, &han(0, 12));
    let glyphs = glyph_count(&runs);
    for frame_index in 0..4 {
        let frame = serviced(&mut compiler, &runs);
        println!(
            "frame {frame_index}: draws {} atlas {} fallbacks {} skipped {} entries {}",
            frame.glyph_draws,
            frame.hvgl_atlas_draws,
            frame.hvgl_outline_fallbacks,
            frame.skipped_glyphs,
            compiler.glyph_atlas_entries()
        );
        assert!(frame.hvgl_atlas_draws > 0, "frame {frame_index}: some fit");
        assert!(
            frame.hvgl_outline_fallbacks > 0,
            "frame {frame_index}: the rest are refused"
        );
        assert_eq!(
            frame.hvgl_atlas_draws + frame.hvgl_outline_fallbacks,
            glyphs,
            "frame {frame_index}: every glyph is drawn one way or the other"
        );
        assert_eq!(frame.glyph_draws, glyphs, "frame {frame_index}");
        assert_eq!(frame.skipped_glyphs, 0, "frame {frame_index}");
        assert!(!frame.strip_buf().is_empty(), "frame {frame_index}");
        assert_eq!(
            compiler.glyph_atlas_entries(),
            frame.hvgl_atlas_draws as usize,
            "frame {frame_index}: one entry per atlas draw"
        );
    }
}

/// (e) A glyph with no ink (a space) takes no slot and is not counted as a
/// refused insert: no entry, no fallback, no draw, on every frame.
#[test]
fn an_inkless_pingfang_glyph_takes_no_slot_and_is_not_a_fallback() {
    let Some(mut ctx) = pingfang() else { return };
    let runs = laid_out(&mut ctx, FontWeight::REGULAR, "中 文");
    assert_eq!(glyph_count(&runs), 3, "the space is a glyph of the run");
    let mut compiler = fresh();
    for frame_index in 0..4 {
        let frame = serviced(&mut compiler, &runs);
        assert_eq!(frame.glyph_draws, 2, "frame {frame_index}");
        assert_eq!(frame.hvgl_atlas_draws, 2, "frame {frame_index}");
        assert_eq!(frame.hvgl_outline_fallbacks, 0, "frame {frame_index}");
        assert_eq!(frame.skipped_glyphs, 0, "frame {frame_index}");
        assert_eq!(
            compiler.glyph_atlas_entries(),
            2,
            "frame {frame_index}: no slot for the space"
        );
    }
    // Only spaces: nothing drawn, nothing resident, nothing refused.
    let spaces = laid_out(&mut ctx, FontWeight::REGULAR, "   ");
    let mut compiler = fresh();
    for frame_index in 0..3 {
        let frame = serviced(&mut compiler, &spaces);
        assert_eq!(frame.glyph_draws, 0, "frame {frame_index}");
        assert_eq!(frame.hvgl_outline_fallbacks, 0, "frame {frame_index}");
        assert_eq!(frame.skipped_glyphs, 0, "frame {frame_index}");
        assert_eq!(compiler.glyph_atlas_entries(), 0, "frame {frame_index}");
    }
}

/// (f) An atlas glyph is a coverage mask tinted with one colour, so a run
/// painted with a gradient is filled as outlines instead.
#[test]
fn a_gradient_brush_falls_back_to_outlines() {
    let Some(mut ctx) = pingfang() else { return };
    let runs = laid_out(&mut ctx, FontWeight::REGULAR, "中文爱");
    let glyphs = glyph_count(&runs);
    let gradient =
        Brush::Gradient(Gradient::new_linear((0.0, 0.0), (100.0, 0.0)).with_stops([RED, BLUE]));
    let mut compiler = fresh();
    for frame_index in 0..3 {
        let frame = serviced(&mut compiler, &with_brush(&runs, gradient.clone()));
        assert_eq!(frame.glyph_draws, glyphs, "frame {frame_index}");
        assert_eq!(frame.hvgl_outline_fallbacks, glyphs, "frame {frame_index}");
        assert_eq!(frame.hvgl_atlas_draws, 0, "frame {frame_index}");
        assert_eq!(frame.atlas_glyph_draws, 0, "frame {frame_index}");
        assert!(!frame.strip_buf().is_empty(), "frame {frame_index}");
        assert_eq!(compiler.glyph_atlas_entries(), 0, "frame {frame_index}");
    }
    // The same compiler takes the atlas again for a solid brush: the fallback
    // is the run's, not the face's.
    let frame = serviced(&mut compiler, &with_brush(&runs, Brush::Solid(BLACK)));
    assert_eq!(frame.hvgl_atlas_draws, glyphs);
    assert_eq!(frame.hvgl_outline_fallbacks, 0);
}

/// (g) The keys are unhinted whatever `hint_text` says: the same run lands in
/// the same entries, drawn the same, hinting on or off.
#[test]
fn hvgl_keys_stay_unhinted_under_hint_text() {
    let Some(mut ctx) = pingfang() else { return };
    let runs = laid_out(&mut ctx, FontWeight::REGULAR, "中文爱");
    let glyphs = glyph_count(&runs);
    let observe = |hint: bool| {
        let mut compiler = fresh();
        compiler.set_hint_text(hint);
        let frame = serviced(&mut compiler, &runs);
        assert_eq!(frame.hvgl_atlas_draws, glyphs, "hint_text {hint}");
        assert_eq!(frame.hvgl_outline_fallbacks, 0, "hint_text {hint}");
        let mut keys: Vec<(u64, u32, bool, u32)> = compiler
            .glyph_atlas_mut()
            .all_keys()
            .map(|key| (key.font_id, key.glyph_id, key.hinted, key.size_bits))
            .collect();
        keys.sort_unstable();
        let slots: Vec<_> = frame
            .glyph_slots
            .iter()
            .map(|slot| slot.region.size)
            .collect();
        (keys, slots)
    };
    let (unhinted_keys, unhinted_slots) = observe(false);
    let (hinted_keys, hinted_slots) = observe(true);
    assert_eq!(unhinted_keys.len(), 3);
    assert!(
        hinted_keys.iter().all(|key| !key.2),
        "no hvgl key is hinted: {hinted_keys:?}"
    );
    assert_eq!(hinted_keys, unhinted_keys, "the same keys either way");
    assert_eq!(hinted_slots, unhinted_slots, "the same entry extents");
}

/// (h) `set_hvgl_atlas(false)` restores outline-every-frame and its counters;
/// switching back resumes the atlas route.
#[test]
fn switching_the_hvgl_atlas_off_restores_outline_every_frame() {
    let Some(mut ctx) = pingfang() else { return };
    let runs = laid_out(&mut ctx, FontWeight::REGULAR, "中文爱");
    let glyphs = glyph_count(&runs);
    let mut compiler = fresh();
    compiler.set_hvgl_atlas(false);
    assert!(!compiler.hvgl_atlas_enabled());
    for frame_index in 0..3 {
        let frame = serviced(&mut compiler, &runs);
        assert_eq!(frame.glyph_draws, glyphs, "frame {frame_index}");
        assert_eq!(frame.hvgl_outline_fallbacks, glyphs, "frame {frame_index}");
        assert_eq!(frame.hvgl_atlas_draws, 0, "frame {frame_index}");
        assert_eq!(frame.atlas_glyph_draws, 0, "frame {frame_index}");
        assert_eq!(frame.resident_glyph_runs, 0, "frame {frame_index}");
        assert!(!frame.strip_buf().is_empty(), "frame {frame_index}");
        assert_eq!(compiler.glyph_atlas_entries(), 0, "frame {frame_index}");
    }
    assert_eq!(compiler.hvgl_cached_outlines(), glyphs as usize);

    compiler.set_hvgl_atlas(true);
    assert!(compiler.hvgl_atlas_enabled());
    let frame = serviced(&mut compiler, &runs);
    assert_eq!(frame.hvgl_atlas_draws, glyphs);
    assert_eq!(frame.hvgl_outline_fallbacks, 0);
    assert_eq!(compiler.glyph_atlas_entries(), glyphs as usize);
}

/// The outline route draws the same glyphs the atlas route does: the same
/// draw count and no skips, so the switch is a cost choice, not a content one.
#[test]
fn the_two_routes_draw_the_same_glyphs() {
    let Some(mut ctx) = pingfang() else { return };
    let runs = laid_out(&mut ctx, FontWeight::SEMI_BOLD, "中文爱 Ab1");
    let mut atlas = fresh();
    let mut outlines = fresh();
    outlines.set_hvgl_atlas(false);
    let on = serviced(&mut atlas, &runs);
    let off = serviced(&mut outlines, &runs);
    assert_eq!(on.glyph_draws, off.glyph_draws);
    assert_eq!(on.skipped_glyphs, off.skipped_glyphs);
    assert_eq!(on.hvgl_atlas_draws, off.hvgl_outline_fallbacks);
}

/// Raw glyph ids of the face, not shaped text: a run built by hand from the
/// face's handle takes the same route.
#[test]
fn a_hand_built_run_of_glyph_ids_takes_the_atlas_route() {
    let Some(mut ctx) = pingfang() else { return };
    let shaped = laid_out(&mut ctx, FontWeight::REGULAR, "中");
    let ids: Vec<u32> = (100..140).collect();
    let runs = vec![run_of_ids(&shaped[0], &ids)];
    let mut compiler = fresh();
    let frame = serviced(&mut compiler, &runs);
    assert_eq!(frame.hvgl_outline_fallbacks, 0);
    assert_eq!(frame.skipped_glyphs, 0);
    assert_eq!(frame.hvgl_atlas_draws, frame.glyph_draws);
    assert!(frame.hvgl_atlas_draws > 0);
}

#[cfg(target_os = "macos")]
mod metal {
    use std::sync::{Mutex, MutexGuard, PoisonError};

    use frust_engine::{EngineRenderer, EngineTarget, OutputAlpha};
    use frust_gpu::{HeadlessTarget, TierCaps};
    use frust_text::FontWeight;
    use kurbo::Affine;
    use peniko::Color;

    use super::{GlyphRun, VIEWPORT, laid_out, pingfang, scene_of};

    static RENDER_LOCK: Mutex<()> = Mutex::new(());

    fn render_lock() -> MutexGuard<'static, ()> {
        RENDER_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        use std::task::{Context, Poll, Waker};

        let mut cx = Context::from_waker(Waker::noop());
        let mut future = std::pin::pin!(future);
        loop {
            match future.as_mut().poll(&mut cx) {
                Poll::Ready(value) => return value,
                Poll::Pending => std::thread::yield_now(),
            }
        }
    }

    fn gpu() -> (wgpu::Device, wgpu::Queue, TierCaps) {
        block_on(async {
            let instance = wgpu::Instance::new(
                wgpu::InstanceDescriptor::new_without_display_handle_from_env(),
            );
            let adapter = wgpu::util::initialize_adapter_from_env_or_default(&instance, None)
                .await
                .expect("no compatible GPU adapter");
            let caps = TierCaps::probe(&adapter);
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    label: Some("frust-engine hvgl atlas device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: frust_gpu::test_device_limits(&adapter, &caps),
                    ..Default::default()
                })
                .await
                .expect("failed to create the device");
            (device, queue, caps)
        })
    }

    /// How many pixels of `runs`, rendered headless through Metal with the
    /// hvgl atlas route `atlas`, carry ink, and how many frames it rendered.
    pub(super) fn inked_pixels(runs: &[GlyphRun], atlas: bool, frames: usize) -> usize {
        let _guard = render_lock();
        let (device, queue, caps) = gpu();
        const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
        let scene = scene_of(runs.to_vec());
        let mut renderer = EngineRenderer::with_image_sampler(&device, &caps, FORMAT, None, true)
            .expect("the engine builds on this device");
        renderer.set_hvgl_atlas(atlas);
        assert_eq!(renderer.hvgl_atlas(), atlas);
        renderer.finish_warm_up(&device);
        let (width, height) = (u32::from(VIEWPORT.0), u32::from(VIEWPORT.1));
        let target = HeadlessTarget::new(&device, width, height, FORMAT);
        for _ in 0..frames {
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frust-engine hvgl atlas frame"),
            });
            renderer
                .encode(
                    &device,
                    &queue,
                    &mut encoder,
                    &scene,
                    EngineTarget {
                        view: target.view(),
                        format: FORMAT,
                        width,
                        height,
                        depth: None,
                        output: OutputAlpha::Premultiplied,
                    },
                    Color::TRANSPARENT,
                    Affine::IDENTITY,
                )
                .expect("a well-formed frame encodes");
            queue.submit([encoder.finish()]);
            renderer.end_frame(&queue);
        }
        let pixels = target.read_back(&device, &queue);
        pixels
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|px| px[3] != 0)
            .count()
    }

    #[test]
    #[ignore = "requires a GPU (Metal); run with `cargo test -p frust-engine --test hvgl_atlas -- --ignored --test-threads=1`"]
    fn the_atlas_route_renders_ink_close_to_the_outline_route() {
        let Some(mut ctx) = pingfang() else { return };
        let runs = laid_out(&mut ctx, FontWeight::REGULAR, "中文爱");
        let atlas = inked_pixels(&runs, true, 3);
        let outlines = inked_pixels(&runs, false, 3);
        println!("inked pixels: atlas {atlas}, outlines {outlines}");
        assert!(atlas > 100, "only {atlas} pixels carry ink");
        assert!(outlines > 100, "only {outlines} pixels carry ink");
        // The route is a cost choice: the same glyphs on near the same pixels.
        let (low, high) = (atlas.min(outlines) as f64, atlas.max(outlines) as f64);
        assert!(high / low < 1.15, "atlas {atlas} vs outlines {outlines}");
    }

    /// Strokes thicken with weight on the atlas route too: the weight is in the
    /// entries' variation coordinates.
    #[test]
    #[ignore = "requires a GPU (Metal); run with `cargo test -p frust-engine --test hvgl_atlas -- --ignored --test-threads=1`"]
    fn the_atlas_route_draws_semibold_heavier_than_light() {
        let Some(mut ctx) = pingfang() else { return };
        let light_runs = laid_out(&mut ctx, FontWeight::LIGHT, "中");
        let semibold_runs = laid_out(&mut ctx, FontWeight::SEMI_BOLD, "中");
        let light = inked_pixels(&light_runs, true, 2);
        let semibold = inked_pixels(&semibold_runs, true, 2);
        println!("inked pixels: light {light}, semibold {semibold}");
        assert!(light > 0);
        assert!(
            semibold > light,
            "weight 600 inks {semibold} pixels, weight 300 {light}"
        );
    }
}
