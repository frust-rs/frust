//! The glyph-atlas policy under churn: what an animated size costs, what a
//! static page costs, and what the kill switch takes away.
//!
//! The policy is the engine's answer to the one thing `glifo`'s atlas does not
//! decide for itself — *which* glyphs are worth an atlas slot. Its failure mode
//! is not a wrong pixel but an unbounded one: a size that moves mints a fresh
//! key every frame, and a cache that grows once per frame forever is worse than
//! no cache at all, because it pays the rasterization *and* the eviction. So
//! the cases below are mostly bounds, measured over frame counts long enough
//! for a leak to show: ten thousand frames of a size sweeping 12 px to 48 px,
//! two hundred frames of a page holding still, and enough idle frames for
//! `glifo`'s own LRU to run a full eviction pass.
//!
//! # Why the module is included by path
//!
//! `frust-engine` declares `text` as `pub(crate)`, so the policy is not
//! nameable from an integration test through the crate's public API, and
//! `src/lib.rs` is not this task's to widen. The module is compiled into this
//! test target directly instead. That is sound precisely because the policy is
//! pure: it names no `crate::`-rooted path, only `glifo`, `peniko`,
//! `frust_scene` and `std`, so the copy compiled here is the same code the
//! crate compiles, not a stand-in for it. The tier-dependent half — which page
//! geometry an adapter gets — *is* reachable publicly, through
//! `frust_engine::AtlasBudget`, and is pinned that way below.
//!
//! No GPU, device or surface is involved anywhere here. Both structures the
//! policy owns — `glifo`'s entry map and `vello_common`'s rectangle packer —
//! are plain host-side bookkeeping, so a slot is allocated, aged and reclaimed
//! in these cases exactly as it would be behind a real texture; what a real
//! texture adds is the pixels, which the policy neither produces nor inspects.

use std::collections::HashSet;
use std::sync::Arc;

use frust_engine::AtlasBudget;
use frust_gpu::{DownlevelProfile, TierCaps};
use frust_scene::{FontHandle, Glyph, GlyphRun};
use glifo::{AtlasConfig, GlyphCacheKey, RasterMetrics};
use kurbo::Affine;
use peniko::color::palette::css::{BLACK, WHITE};
use peniko::{Blob, Brush, FontData};
use vello_common::multi_atlas::AllocationStrategy;

#[path = "../src/text/atlas_policy.rs"]
mod atlas_policy;

use atlas_policy::{
    AtlasPass, AtlasPolicy, GlyphRoute, MAX_CACHED_FONT_SIZE, OutlineReason, RunKey, RunRoute,
    SETTLE_FRAMES, SIZE_QUANTUM, glyph_cache_config, quantize_font_size,
};

/// Noto Sans, subsetted to Latin plus combining marks — the same bundled face
/// the rest of the crate's text cases name glyph ids against.
const LATIN_FONT: &[u8] = include_bytes!("../../../testing/fonts/NotoSans-Subset.ttf");

/// Glyph ids of `Hello` in [`LATIN_FONT`]: `H`, `e`, `l`, `l`, `o`.
const HELLO: [u32; 5] = [5, 6, 7, 7, 8];

/// A font id no other case uses, so one case's size history can never be read
/// as another's animation. Handed out by [`next_font`].
static NEXT_FONT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Frames every long-running case runs for.
///
/// Ten thousand is roughly three minutes of animation at 60 Hz — far past the
/// 64-frame eviction period, so a leak that only shows after several eviction
/// cycles still shows.
const CHURN_FRAMES: u64 = 10_000;

/// The most glyphs any case here may leave resident.
///
/// Sized against what the cases actually put on screen — two runs of five
/// glyphs each — with room to spare, so it fails on a leak rather than on an
/// off-by-one.
const ENTRY_BOUND: usize = 16;

/// A font id nothing else in this file uses.
fn next_font() -> u64 {
    NEXT_FONT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// A run identity at `size`, on its own private font.
fn run_key(font_id: u64, size: f32) -> RunKey {
    RunKey {
        font_id,
        font_index: 0,
        font_size: size,
        hinted: false,
        context_color: BLACK,
    }
}

/// The page geometry every case packs into unless it is deliberately
/// constraining it: one desktop-sized layer, grown on demand.
fn pages() -> AtlasConfig {
    AtlasConfig {
        initial_atlas_count: 0,
        max_atlases: 8,
        atlas_size: (2048, 2048),
        auto_grow: true,
        allocation_strategy: AllocationStrategy::FirstFit,
    }
}

/// A policy over [`pages`], enabled or killed by `disabled`.
fn policy(disabled: bool) -> AtlasPolicy {
    AtlasPolicy::new(pages(), disabled)
}

/// A plausible bitmap extent for `key`'s glyph: a little narrower than the em
/// and about as tall, which is what a Latin lowercase letter rasterizes to.
///
/// Exact numbers do not matter to any assertion here — what matters is that a
/// bigger size claims proportionally more atlas space, so the packer is put
/// under the same pressure a real rasterizer would put it under.
fn raster(key: &GlyphCacheKey) -> Option<RasterMetrics> {
    let size = f32::from_bits(key.size_bits);
    let width = (size * 0.6).ceil().clamp(1.0, f32::from(u16::MAX));
    let height = size.ceil().clamp(1.0, f32::from(u16::MAX));
    Some(RasterMetrics {
        width: width as u16,
        height: height as u16,
        bearing_x: 0,
        bearing_y: height as i16,
    })
}

/// Collect one run of [`HELLO`] at `size`, laid out on a whole-pixel baseline,
/// and answer the route the run itself took.
fn collect_hello(engine: &mut AtlasPolicy, font_id: u64, size: f32) -> RunRoute {
    let route = engine.classify_run(&run_key(font_id, size));
    if let Some(run) = route.atlas() {
        for (index, glyph) in HELLO.iter().enumerate() {
            engine.collect_glyph(run, *glyph, index as f32 * size * 0.5);
        }
    }
    route
}

/// One whole frame drawing `runs` (a font id and a size each), returning the
/// atlas pass it produced.
fn frame(engine: &mut AtlasPolicy, runs: &[(u64, f32)]) -> AtlasPass {
    engine.begin_frame();
    for (font_id, size) in runs {
        collect_hello(engine, *font_id, *size);
    }
    let pass = engine.build(raster);
    engine.end_frame();
    pass
}

/// The size the animated run is drawn at on `frame_index`.
///
/// A sawtooth sweeping 12 px to 48 px about every 27 frames — a real animation
/// pace, under half a second a cycle — with a step chosen so the sequence does
/// not close on itself: successive cycles land on different `f32`s, exactly as
/// an animation interpolated against wall-clock time does. A cycle that
/// *repeated* would be the easy case, since the second pass over it would hit
/// every key the first pass minted.
fn animated_size(frame_index: u64) -> f32 {
    // Accumulated in `f64` so the phase itself does not quantize: at `f32`
    // precision the sawtooth closes on itself after a few hundred cycles, which
    // would make the sweep kinder than a real one.
    let phase = ((frame_index as f64) * 0.037_182_8).fract();
    (12.0 + 36.0 * phase) as f32
}

#[test]
fn quantization_snaps_to_a_quarter_pixel_without_moving_a_size_more_than_an_eighth() {
    // Every whole size — which is nearly every size anything actually asks for
    // — is a fixed point, bit for bit. This is what makes turning the atlas on
    // a no-op for ordinary text rather than a re-rasterization at a nearby
    // size.
    for size in 1..=128 {
        let size = size as f32;
        assert_eq!(
            quantize_font_size(size),
            Some(size),
            "a whole size must quantize to itself"
        );
    }

    // And nothing else moves by more than half a quantum.
    let mut steps = 0;
    let mut size = 8.0_f32;
    while size <= 64.0 {
        let quantized = quantize_font_size(size).expect("a positive finite size is usable");
        assert!(
            (quantized - size).abs() <= SIZE_QUANTUM / 2.0 + f32::EPSILON,
            "quantizing {size} to {quantized} moved it more than half a quantum"
        );
        size += 0.01;
        steps += 1;
    }
    assert!(steps > 5_000, "fixture precondition: the sweep is dense");
}

#[test]
fn a_size_that_cannot_be_rasterized_is_refused_rather_than_keyed() {
    for size in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.0, -12.0] {
        assert_eq!(
            quantize_font_size(size),
            None,
            "{size} is not a size a glyph can be rasterized at"
        );
    }

    let mut engine = policy(false);
    engine.begin_frame();
    let route = engine.classify_run(&run_key(next_font(), f32::NAN));
    assert_eq!(route.outline_reason(), Some(OutlineReason::UnusableSize));
    assert_eq!(
        engine.tracked_sizes(),
        0,
        "a size that was never usable must not enter the animation tracker"
    );
}

#[test]
fn a_static_page_uploads_on_its_first_frame_and_does_nothing_on_its_second() {
    let mut engine = policy(false);
    let font = next_font();

    engine.begin_frame();
    let route = collect_hello(&mut engine, font, 16.0);
    assert!(
        route.atlas().is_some(),
        "a font drawn at a size nothing preceded is a first appearance, not a change"
    );
    let first = engine.build(raster);
    assert_eq!(
        first.uploads.len(),
        4,
        "one upload per distinct glyph; `Hello`'s two `l`s share a key"
    );
    assert!(first.clears.is_empty(), "nothing has been evicted yet");
    assert_eq!(first.refused, 0);
    engine.end_frame();

    // The whole point of the cache: the second frame of an unchanged page is
    // free.
    engine.begin_frame();
    let route = engine.classify_run(&run_key(font, 16.0));
    let run = *route.atlas().expect("a settled size stays cached");
    for (index, glyph) in HELLO.iter().enumerate() {
        assert_eq!(
            engine.collect_glyph(&run, *glyph, index as f32 * 8.0),
            GlyphRoute::Cached,
            "every glyph of an unchanged page is already resident"
        );
    }
    let second = engine.build(raster);
    assert!(
        second.is_empty(),
        "an unchanged page's atlas pass has nothing to do and can be skipped whole"
    );
    engine.end_frame();

    // And it stays free.
    for _ in 0..200 {
        let pass = frame(&mut engine, &[(font, 16.0)]);
        assert!(pass.is_empty(), "a page that never changes never uploads");
    }
    assert_eq!(engine.entry_count(), 4);
    assert_eq!(engine.page_count(), 1);
}

#[test]
fn ten_thousand_frames_of_an_animated_size_keep_the_cache_bounded() {
    let mut engine = policy(false);
    let animated = next_font();
    let static_font = next_font();

    let mut uploads = 0_usize;
    let mut animating_frames = 0_u64;
    let mut worst_entries = 0_usize;
    let mut worst_tracked = 0_usize;

    for index in 0..CHURN_FRAMES {
        engine.begin_frame();
        let route = collect_hello(&mut engine, animated, animated_size(index));
        if route.outline_reason() == Some(OutlineReason::SizeAnimating) {
            animating_frames += 1;
        }
        collect_hello(&mut engine, static_font, 16.0);
        let pass = engine.build(raster);
        uploads += pass.uploads.len();
        assert_eq!(pass.refused, 0, "a 2048-square page is not under pressure");
        engine.end_frame();

        worst_entries = worst_entries.max(engine.entry_count());
        worst_tracked = worst_tracked.max(engine.tracked_sizes());
    }

    // The bound the whole policy exists for.
    assert!(
        worst_entries <= ENTRY_BOUND,
        "a sweeping size left {worst_entries} entries resident at its peak"
    );
    // The other structure a sweep could grow without bound, and the one that
    // would leak silently since it holds no atlas space.
    assert!(
        worst_tracked <= ENTRY_BOUND,
        "the animation tracker grew to {worst_tracked} recorded sizes"
    );
    assert_eq!(
        engine.page_count(),
        1,
        "nothing here should ever need a second page"
    );

    // Eight glyph rasterizations in three minutes of animation: `Hello`'s four
    // distinct glyphs for the static run, and the same four for the animated
    // run's very first frame — which is not yet distinguishable from static
    // text, and must not be, or a page would pay eight frames of outlines
    // before any of its text cached.
    assert_eq!(
        uploads, 8,
        "the only uploads are the first frame of each run"
    );
    assert!(
        animating_frames >= CHURN_FRAMES - 2,
        "the sweep was recognised as animating on only {animating_frames} of {CHURN_FRAMES} frames"
    );

    // The negative control: what the same sweep costs a cache that keys the
    // size the way `glifo` does on its own, by exact `f32` bits and with no
    // notion of a size in motion. Counted here rather than asserted about,
    // because "the policy helps" is only a claim if the unpoliced number is
    // known.
    let mut raw_keys = HashSet::new();
    let mut quantized_keys = HashSet::new();
    for index in 0..CHURN_FRAMES {
        let size = animated_size(index);
        let quantized = quantize_font_size(size).expect("a swept size is usable");
        for glyph in HELLO {
            raw_keys.insert((size.to_bits(), glyph));
            quantized_keys.insert((quantized.to_bits(), glyph));
        }
    }
    assert!(
        raw_keys.len() > 30_000,
        "fixture precondition: the sweep really is non-repeating ({} keys)",
        raw_keys.len()
    );
    assert!(
        quantized_keys.len() < raw_keys.len() / 50,
        "quantization alone must already collapse the sweep by two orders of magnitude"
    );
    assert!(
        uploads < quantized_keys.len() / 50,
        "the animation guard must collapse what is left"
    );
}

#[test]
fn a_glyph_repeated_across_a_frame_is_rasterized_once() {
    let mut engine = policy(false);
    let font = next_font();

    engine.begin_frame();
    let route = engine.classify_run(&run_key(font, 24.0));
    let run = *route.atlas().expect("a first appearance is cached");
    // The same glyph at the same whole-pixel position, two hundred times: one
    // paragraph's worth of a common letter.
    for _ in 0..200 {
        assert_eq!(
            engine.collect_glyph(&run, HELLO[0], 0.0),
            GlyphRoute::Pending
        );
    }
    let pass = engine.build(raster);
    assert_eq!(
        pass.uploads.len(),
        1,
        "a key collected many times is allocated once"
    );
    engine.end_frame();
}

#[test]
fn two_concurrent_sizes_of_one_font_settle_instead_of_reading_as_an_animation() {
    // A heading and its body text share a face and never move. Nothing here may
    // be mistaken for an animation, or the most ordinary screen in the
    // framework would never cache a glyph.
    let mut engine = policy(false);
    let font = next_font();

    // The frame a second size first appears *is* a change, and is treated as
    // one — there is no way to tell it from the first frame of a size sweep.
    // What matters is that it settles.
    for _ in 0..(SETTLE_FRAMES + 4) {
        frame(&mut engine, &[(font, 32.0), (font, 16.0)]);
    }

    let mut uploads = 0_usize;
    for _ in 0..200 {
        engine.begin_frame();
        for size in [32.0_f32, 16.0] {
            let route = collect_hello(&mut engine, font, size);
            assert!(
                route.atlas().is_some(),
                "two sizes held still are two settled sizes, not an animation"
            );
        }
        let pass = engine.build(raster);
        uploads += pass.uploads.len();
        engine.end_frame();
    }

    assert_eq!(uploads, 0, "a settled two-size page uploads nothing");
    assert_eq!(
        engine.entry_count(),
        8,
        "four distinct glyphs at each of two sizes"
    );
}

#[test]
fn a_size_past_the_cache_ceiling_is_drawn_as_outlines() {
    let mut engine = policy(false);

    engine.begin_frame();
    // At the ceiling, not past it: the comparison must not be off by one.
    assert!(
        engine
            .classify_run(&run_key(next_font(), MAX_CACHED_FONT_SIZE))
            .atlas()
            .is_some(),
        "the ceiling itself is cacheable"
    );
    assert_eq!(
        engine
            .classify_run(&run_key(next_font(), MAX_CACHED_FONT_SIZE + SIZE_QUANTUM))
            .outline_reason(),
        Some(OutlineReason::SizeTooLarge)
    );
    assert_eq!(
        engine
            .classify_run(&run_key(next_font(), 512.0))
            .outline_reason(),
        Some(OutlineReason::SizeTooLarge)
    );
    let pass = engine.build(raster);
    assert!(
        pass.uploads.is_empty(),
        "an oversized run collects nothing to upload"
    );
    engine.end_frame();
}

#[test]
fn an_evicted_slot_is_cleared_before_its_rectangle_can_be_reused() {
    let mut engine = policy(false);
    let font = next_font();

    frame(&mut engine, &[(font, 16.0)]);
    assert_eq!(engine.entry_count(), 4);

    // The screen navigates away. `glifo` sweeps on its own schedule, so the
    // eviction frame is found rather than predicted — and until it happens
    // there is nothing to clear.
    let mut idle = 0;
    loop {
        let pass = frame(&mut engine, &[]);
        assert!(
            pass.clears.is_empty(),
            "a clear rect can only exist once something has been evicted"
        );
        idle += 1;
        assert!(idle < 500, "fixture precondition: eviction runs eventually");
        if engine.entry_count() == 0 {
            break;
        }
    }

    // The screen comes back on the very next frame, so the freed rectangles are
    // reallocated in the same pass that zeroes them — which is exactly the
    // ordering the pass exists to fix: clears first, then the uploads that may
    // land on top of them.
    let pass = frame(&mut engine, &[(font, 16.0)]);
    assert_eq!(
        pass.clears.len(),
        4,
        "every evicted slot is reported for clearing exactly once"
    );
    assert!(
        !pass.uploads.is_empty(),
        "fixture precondition: the returning page reallocates"
    );

    // A cleared region is the padded one, so the transparent border `glifo`
    // packs around a glyph is zeroed too rather than left holding the previous
    // occupant's edge.
    for rect in &pass.clears {
        assert!(rect.width > 0 && rect.height > 0);
    }

    // And a clear is never re-reported: the queue is drained, not read.
    let next = frame(&mut engine, &[(font, 16.0)]);
    assert!(next.clears.is_empty());
}

#[test]
fn a_full_atlas_falls_back_to_outlines_rather_than_dropping_a_glyph() {
    // One tiny page, no growth: the smallest atlas that can hold a glyph or two
    // and then must refuse.
    let mut engine = AtlasPolicy::new(
        AtlasConfig {
            initial_atlas_count: 0,
            max_atlases: 1,
            atlas_size: (64, 64),
            auto_grow: false,
            allocation_strategy: AllocationStrategy::FirstFit,
        },
        false,
    );
    let font = next_font();

    engine.begin_frame();
    let route = engine.classify_run(&run_key(font, 48.0));
    let run = *route.atlas().expect("a first appearance is cached");
    let mut keys = Vec::new();
    for glyph in 0..32_u32 {
        assert_eq!(
            engine.collect_glyph(&run, glyph, 0.0),
            GlyphRoute::Pending,
            "the collect phase claims a glyph before it knows whether it fits"
        );
        keys.push(run.key(glyph, 0.0));
    }
    let pass = engine.build(raster);

    assert!(
        pass.refused > 0,
        "fixture precondition: a 64-square page cannot hold 32 glyphs at 48 px"
    );
    assert_eq!(
        pass.uploads.len() + pass.refused as usize,
        keys.len(),
        "every collected glyph is either allocated or refused, never lost"
    );
    // A refused glyph resolves to no slot, which is the same answer an
    // uncollected one gives, and is what routes it back to outline strips.
    let unresolved = keys.iter().filter(|key| engine.slot(key).is_none()).count();
    assert_eq!(unresolved, pass.refused as usize);
    engine.end_frame();
}

#[test]
fn nothing_resolves_to_a_slot_before_the_frame_has_been_built() {
    let mut engine = policy(false);
    let font = next_font();

    engine.begin_frame();
    let route = engine.classify_run(&run_key(font, 16.0));
    let run = *route.atlas().expect("a first appearance is cached");
    let key = run.key(HELLO[0], 0.0);
    engine.collect_glyph(&run, HELLO[0], 0.0);
    assert!(
        engine.slot(&key).is_none(),
        "a key collected this frame has no slot until the atlas pass has run"
    );

    engine.build(raster);
    assert!(
        engine.slot(&key).is_some(),
        "and has one immediately afterwards"
    );

    engine.end_frame();
    assert!(
        engine.slot(&key).is_none(),
        "and cannot be resolved outside a frame at all"
    );
}

#[test]
fn collecting_outside_the_collect_phase_routes_to_outlines_rather_than_silently_failing() {
    let mut engine = policy(false);
    let font = next_font();

    // Before any frame is opened.
    assert_eq!(
        engine.classify_run(&run_key(font, 16.0)).outline_reason(),
        Some(OutlineReason::NotCollecting)
    );

    engine.begin_frame();
    let route = engine.classify_run(&run_key(font, 16.0));
    let run = *route.atlas().expect("a first appearance is cached");
    engine.build(raster);
    // And after the collect phase has closed.
    assert_eq!(
        engine.collect_glyph(&run, HELLO[0], 0.0),
        GlyphRoute::Outline(OutlineReason::NotCollecting)
    );
    engine.end_frame();
}

#[test]
fn no_atlas_routes_every_glyph_to_outlines_and_allocates_nothing() {
    let mut engine = policy(true);
    assert!(!engine.is_enabled());

    let animated = next_font();
    let static_font = next_font();

    for index in 0..500_u64 {
        engine.begin_frame();
        for (font, size) in [
            (animated, animated_size(index)),
            (static_font, 16.0),
            (static_font, 512.0),
        ] {
            let route = engine.classify_run(&run_key(font, size));
            assert_eq!(
                route.outline_reason(),
                Some(OutlineReason::Disabled),
                "the kill switch answers before any other rule, so the reason \
                 is always the switch itself"
            );
        }
        let pass = engine.build(raster);
        assert!(pass.is_empty(), "a killed atlas has no pass to run");
        engine.end_frame();
    }

    assert_eq!(engine.entry_count(), 0);
    assert_eq!(
        engine.page_count(),
        0,
        "a killed atlas allocates no texture memory at all"
    );
    assert_eq!(
        engine.tracked_sizes(),
        0,
        "and keeps no size history, since it can never act on one"
    );
}

#[test]
fn caching_a_glyph_never_moves_it_off_the_size_the_display_list_asked_for() {
    // The pixel half of the kill switch's contract. Toggling
    // `FRUST_ENGINE_NO_ATLAS` swaps *where* a glyph is composited from, not
    // what was rasterized: the only parameter the policy changes on the way to
    // the rasterizer is the font size, and only by quantization. So the
    // strongest statement testable without a device is the bound on that one
    // parameter — exact for a whole size, an eighth of a pixel otherwise.
    let mut engine = policy(false);

    let mut worst = 0.0_f32;
    let mut sizes = 0;
    let mut size = 6.0_f32;
    while size <= MAX_CACHED_FONT_SIZE {
        engine.begin_frame();
        let route = engine.classify_run(&run_key(next_font(), size));
        let run = route.atlas().expect("a first appearance is cached");
        let cached = run.size();
        let key = run.key(HELLO[0], 0.0);
        assert_eq!(
            f32::from_bits(key.size_bits),
            cached,
            "the key must carry the quantized size, never the requested one"
        );

        if size.fract() == 0.0 {
            assert_eq!(
                cached, size,
                "a whole size reaches the rasterizer untouched, so ordinary \
                 text is byte-identical either way"
            );
        }
        worst = worst.max((cached - size).abs());
        sizes += 1;

        engine.build(raster);
        engine.end_frame();
        size += 0.13;
    }

    assert!(sizes > 900, "fixture precondition: the sweep is dense");
    assert!(
        worst <= SIZE_QUANTUM / 2.0 + f32::EPSILON,
        "the atlas moved a run's size by {worst} px"
    );
}

#[test]
fn the_cache_runs_on_the_documented_numbers() {
    let config = glyph_cache_config();
    assert_eq!(config.max_entry_age, 64);
    assert_eq!(config.eviction_frequency, 64);
    assert!((config.max_cached_font_size - 128.0).abs() < f32::EPSILON);
    assert_eq!(SIZE_QUANTUM, 0.25);
}

#[test]
fn page_geometry_is_the_adapters_own_tier_budget() {
    // The one half of the policy reachable through the crate's public API, and
    // the half a device actually pays for. `AtlasBudget::for_caps` is what
    // `text::glyph_atlas_policy` hands the policy, so pinning it here pins the
    // geometry the policy runs on.
    let mobile = AtlasBudget::for_caps(&TierCaps::fake(DownlevelProfile::WebGl2)).config();
    assert_eq!(mobile.atlas_size, (1024, 1024));
    assert_eq!(mobile.max_atlases, 4);

    let desktop = AtlasBudget::for_caps(&TierCaps::fake(DownlevelProfile::Full)).config();
    assert_eq!(desktop.atlas_size, (2048, 2048));
    assert_eq!(desktop.max_atlases, 8);

    // Nothing is allocated until a glyph needs it, on either tier.
    for geometry in [mobile, desktop] {
        assert_eq!(geometry.initial_atlas_count, 0);
        let engine = AtlasPolicy::new(geometry, false);
        assert_eq!(engine.page_count(), 0);
        assert_eq!(engine.pages().atlas_size, geometry.atlas_size);
    }
}

#[test]
fn a_display_list_run_is_keyed_by_the_font_blob_it_already_carries() {
    let font = FontHandle::new(FontData::new(Blob::new(Arc::new(LATIN_FONT)), 0));
    let run = GlyphRun {
        font: font.clone(),
        font_size: 18.0,
        brush: Brush::Solid(WHITE),
        transform: Affine::IDENTITY,
        glyphs: HELLO
            .iter()
            .map(|id| Glyph {
                id: *id,
                x: 0.0,
                y: 0.0,
            })
            .collect(),
    };

    let key = RunKey::for_run(&run, false, BLACK);
    assert_eq!(key.font_id, font.font().data.id());
    assert_eq!(key.font_index, 0);
    assert_eq!(key.font_size, 18.0);
    assert!(!key.hinted);

    // A second run built from a clone of the same handle is the same font, so
    // two runs of one paragraph share every entry rather than each minting its
    // own.
    let second = GlyphRun {
        font,
        font_size: 24.0,
        brush: Brush::Solid(WHITE),
        transform: Affine::translate((0.0, 40.0)),
        glyphs: Vec::new(),
    };
    assert_eq!(RunKey::for_run(&second, false, BLACK).font_id, key.font_id);

    // Hinting is part of the key: a hinted outline is a different bitmap.
    let mut engine = policy(false);
    engine.begin_frame();
    let plain = engine.classify_run(&RunKey::for_run(&run, false, BLACK));
    let hinted = engine.classify_run(&RunKey::for_run(&run, true, BLACK));
    let plain = plain.atlas().expect("a first appearance is cached");
    let hinted = hinted.atlas().expect("hinting does not change the size");
    assert_ne!(
        plain.key(HELLO[0], 0.0),
        hinted.key(HELLO[0], 0.0),
        "a hinted glyph must not reuse the unhinted entry"
    );
    engine.end_frame();
}
