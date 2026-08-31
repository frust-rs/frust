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
//! `frust_engine::AtlasBudget`, and is pinned that way below. (`kurbo` joins
//! that list because the policy reads a run's transform: the scale `glifo`
//! absorbs into the font size is what it keys, so the guard has to see it.)
//!
//! That the policy allocates through a *borrowed* `glifo::ImageCache` rather
//! than one of its own is what keeps it nameable here at all: the shared
//! allocator's owner is `frust_engine::cache::images::ImageResidency`, and a
//! policy that referred to it by type could not be compiled into this target.
//! It is also the point of the cross-class cases below, which pair the real
//! residency with a policy over its allocator.
//!
//! # Two halves
//!
//! The first half drives the policy directly, a frame at a time, which is what
//! lets a ten-thousand-frame sweep run in milliseconds. The second half drives
//! the *wired* path — a `frust_scene::Scene` compiled by a real
//! `SceneCompiler` — and pins what those decisions do once `glifo` is holding
//! the cache: which glyphs became atlas draws, the slot each one names, the
//! tint its paint applies, and the rectangles an eviction gave back.
//!
//! No GPU, device or surface is involved anywhere here, in either half. Both
//! structures a frame touches — `glifo`'s entry map and `vello_common`'s
//! rectangle packer — are plain host-side bookkeeping, so a slot is allocated,
//! aged and reclaimed in these cases exactly as it would be behind a real
//! texture; what a real texture adds is the pixels, which are produced by the
//! render-to-atlas pass and belong with the GPU cases in `atlas_render.rs`.

use std::collections::HashSet;
use std::sync::Arc;

use frust_engine::cache::images::{ImageResidency, MAX_UNSEEN_FRAMES};
use frust_engine::compile::CompiledFrame;
use frust_engine::{AtlasBudget, EngineRenderer, EngineTarget, OutputAlpha, SceneCompiler};
use frust_gpu::{DownlevelProfile, HeadlessTarget, TierCaps};
use frust_scene::{FontHandle, Glyph, GlyphRun, Scene, SceneBuilder};
use glifo::{AtlasConfig, GlyphCacheKey, ImageCache, RasterMetrics};
use kurbo::{Affine, Rect};
use peniko::color::palette::css::{BLACK, WHITE};
use peniko::{Blob, Brush, FontData, ImageAlphaType, ImageData, ImageFormat};
use vello_common::encode::EncodedPaint;
use vello_common::multi_atlas::AllocationStrategy;
use vello_common::paint::TintMode;

#[path = "../src/text/atlas_policy.rs"]
mod atlas_policy;

use atlas_policy::{
    AtlasPass, AtlasPolicy, EVICTION_FREQUENCY, GlyphRoute, MAX_CACHED_FONT_SIZE, MAX_ENTRY_AGE,
    OutlineReason, RunKey, RunRoute, SETTLE_FRAMES, SIZE_QUANTUM, absorbed_scale, device_font_size,
    entry_ceiling_at, glyph_cache_config, glyph_entry_ceiling, quantize_font_size,
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

/// A run identity at `size`, on its own private font, drawn unscaled.
///
/// The identity transform is the ordinary case and keeps the device size equal
/// to the asked-for one; [`scaled_run_key`] is what the transform cases use.
fn run_key(font_id: u64, size: f32) -> RunKey {
    scaled_run_key(font_id, size, Affine::IDENTITY)
}

/// A run identity at `size` drawn under `transform`, on its own private font.
fn scaled_run_key(font_id: u64, size: f32, transform: Affine) -> RunKey {
    RunKey {
        font_id,
        font_index: 0,
        font_size: size,
        transform,
        hinted: false,
        color_font: false,
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

/// The shared allocator every case packs into: a cache over [`pages`], which
/// in the engine is the image residency's own.
fn cache() -> ImageCache {
    ImageCache::new_with_config(pages())
}

/// A policy over `images`, enabled or killed by `disabled`.
fn policy(images: &ImageCache, disabled: bool) -> AtlasPolicy {
    AtlasPolicy::new(images, disabled)
}

/// An opaque `width` x `height` image, distinct from every other one this
/// module makes: the blob is filled with `tag`, so its `Blob::id` — which is
/// what residency keys on — is its own.
fn image(width: u32, height: u32, tag: u8) -> ImageData {
    let len = (width as usize) * (height as usize) * 4;
    ImageData {
        data: Blob::new(Arc::new(vec![tag; len])),
        format: ImageFormat::Rgba8,
        alpha_type: ImageAlphaType::Alpha,
        width,
        height,
    }
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
///
/// `images` is the shared allocator, handed to the two calls that allocate and
/// evict — the same pairing the engine makes between the policy and the image
/// residency's cache.
///
/// A *serviced* frame: both acknowledgements are made, exactly as
/// `EngineRenderer::encode` makes them once the writes and the replay pass have
/// really reached the array. That matters to eviction — the policy defers
/// `glifo`'s ageing pass while a replay is outstanding — so a helper that
/// skipped them would model a renderer whose every frame was refused. The
/// unserviced case has its own cases below rather than being the default here.
fn frame(engine: &mut AtlasPolicy, images: &mut ImageCache, runs: &[(u64, f32)]) -> AtlasPass {
    engine.begin_frame();
    for (font_id, size) in runs {
        collect_hello(engine, *font_id, *size);
    }
    let pass = engine.build(images, raster);
    engine.acknowledge_clears();
    engine.end_frame(images);
    engine.acknowledge_replay();
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

    let images = cache();
    let mut engine = policy(&images, false);
    engine.begin_frame();
    let route = engine.classify_run(&run_key(next_font(), f32::NAN));
    assert_eq!(route.outline_reason(), Some(OutlineReason::UnusableSize));
    assert_eq!(
        engine.tracked_sizes(),
        0,
        "a size that was never usable must not enter the animation tracker"
    );
}

/// The absorption rule, restated here and checked against the shapes `glifo`
/// itself accepts and refuses.
///
/// A transform on the accepted side has its scale folded into the font size, so
/// the size the guard tracks and the size the key is built from are the same
/// number. One on the refused side is not cached at all — and, more to the
/// point, is never even *looked up*, since `glifo` probes with an unabsorbed
/// key before it discovers it will not cache the run.
#[test]
fn the_size_a_run_is_keyed_at_is_its_font_size_with_the_transforms_scale_in_it() {
    // Absorbed: a uniform positive scale, with or without a translation.
    assert_eq!(device_font_size(16.0, Affine::IDENTITY), Some(16.0));
    assert_eq!(device_font_size(16.0, Affine::scale(2.0)), Some(32.0));
    assert_eq!(
        device_font_size(16.0, Affine::translate((40.0, -3.0)) * Affine::scale(0.5)),
        Some(8.0),
        "a translation moves a run, it does not resize it"
    );
    assert_eq!(absorbed_scale(Affine::scale(3.0)), Some(3.0));

    // Refused: everything `glifo` leaves in the draw transform instead.
    for transform in [
        Affine::rotate(0.4),
        Affine::scale_non_uniform(2.0, 3.0),
        Affine::scale(-1.0),
        Affine::scale_non_uniform(1.0, -1.0),
        Affine::skew(0.3, 0.0),
        Affine::new([f64::NAN, 0.0, 0.0, 1.0, 0.0, 0.0]),
    ] {
        assert_eq!(
            absorbed_scale(transform),
            None,
            "{transform:?} is not a scale `glifo` absorbs, so no glyph drawn \
             through it may reach — or read — the cache"
        );
    }

    // And a run drawn through one of those is routed to outlines, rather than
    // being offered a lookup that could hit a unit-scale entry and draw it flat.
    let images = cache();
    let mut engine = policy(&images, false);
    engine.begin_frame();
    assert_eq!(
        engine
            .classify_run(&scaled_run_key(next_font(), 16.0, Affine::rotate(0.4)))
            .outline_reason(),
        Some(OutlineReason::TransformUncacheable)
    );
}

/// A container scaling while its `font_size` holds still is a size animation.
///
/// The whole point of measuring in device space: nothing about the display list
/// changes across these frames, and every one of them is nevertheless a fresh
/// cache key underneath.
#[test]
fn a_scale_that_moves_reads_as_an_animation_though_the_font_size_never_does() {
    let images = cache();
    let mut engine = policy(&images, false);
    let font = next_font();

    // The first appearance is cached, as any first appearance is.
    engine.begin_frame();
    assert!(
        engine
            .classify_run(&scaled_run_key(font, 16.0, Affine::scale(1.0)))
            .atlas()
            .is_some()
    );

    // Then the container starts scaling. The size the display list states is
    // identical on every one of these frames.
    let mut animating = 0;
    for index in 1..40_u64 {
        engine.begin_frame();
        let scale = 1.0 + f64::from(index as u32) * 0.05;
        let route = engine.classify_run(&scaled_run_key(font, 16.0, Affine::scale(scale)));
        if route.outline_reason() == Some(OutlineReason::SizeAnimating) {
            animating += 1;
        }
    }
    assert_eq!(
        animating, 39,
        "every frame of a moving scale must read as animating"
    );

    // The ceiling is a device-space one too. The asked-for size is comfortably
    // under it and the size actually rasterized is well over.
    let asked = MAX_CACHED_FONT_SIZE / 2.0;
    engine.begin_frame();
    assert_eq!(
        engine
            .classify_run(&scaled_run_key(next_font(), asked, Affine::scale(3.0)))
            .outline_reason(),
        Some(OutlineReason::SizeTooLarge),
        "the ceiling has to be compared against the size `glifo` rasterizes at, \
         not the one the display list asked for"
    );
    engine.begin_frame();
    assert!(
        engine
            .classify_run(&scaled_run_key(next_font(), asked, Affine::IDENTITY))
            .atlas()
            .is_some(),
        "fixture precondition: the same size unscaled is well inside the ceiling"
    );
}

/// Glyph residency is bounded independently of the LRU, so it cannot spend the
/// allocator the images are packed into.
#[test]
fn the_atlas_route_closes_once_glyph_residency_reaches_its_ceiling() {
    // The ceiling scales with the array and is clamped at both ends.
    // Small enough that the ceiling clamps to its floor, and roomy enough that
    // the *packer* is not what refuses first — the case's subject is the
    // policy's own bound, not `vello_common`'s.
    let mut small = pages();
    small.atlas_size = (256, 256);
    small.max_atlases = 1;
    assert_eq!(
        glyph_entry_ceiling(small),
        256,
        "a tiny atlas still has to cache something, or the bound would turn the \
         cache off on exactly the devices it was budgeted for"
    );

    let mobile = AtlasBudget::MOBILE.config();
    let desktop = AtlasBudget::DESKTOP.config();
    assert!(
        glyph_entry_ceiling(mobile) < glyph_entry_ceiling(desktop),
        "a bigger array admits more glyphs"
    );
    assert!(
        glyph_entry_ceiling(desktop) <= 65_536,
        "the entry map's own bookkeeping is not free"
    );

    // And the route really closes: fill the ceiling, then ask for one more.
    let mut images = ImageCache::new_with_config(small);
    let mut engine = policy(&images, false);
    let ceiling = engine.max_entries();

    // Filled in a handful of wide frames rather than one glyph at a time: the
    // LRU reaps anything unseen for `MAX_ENTRY_AGE` frames, so a slow fill
    // would age its own head off and never reach any ceiling at all.
    let font = next_font();
    let mut next_glyph = 0_u32;
    for _ in 0..8 {
        engine.begin_frame();
        let Some(run) = engine.classify_run(&run_key(font, 16.0)).atlas().copied() else {
            break;
        };
        for _ in 0..64 {
            // A fresh glyph id each time, so every one is a fresh key.
            engine.collect_glyph(&run, next_glyph, 0.0);
            next_glyph += 1;
        }
        // One texel apiece, so the *packer* is never what refuses.
        engine.build(&mut images, |_| {
            Some(RasterMetrics {
                width: 1,
                height: 1,
                bearing_x: 0,
                bearing_y: 1,
            })
        });
        engine.acknowledge_clears();
        engine.end_frame(&mut images);
        engine.acknowledge_replay();
        if engine.entry_count() >= ceiling {
            break;
        }
    }

    assert!(
        engine.entry_count() >= ceiling,
        "fixture precondition: residency reaches the ceiling (got {})",
        engine.entry_count()
    );
    engine.begin_frame();
    assert_eq!(
        engine.classify_run(&run_key(font, 16.0)).outline_reason(),
        Some(OutlineReason::ResidencyFull),
        "a full glyph residency degrades to outlines rather than taking the \
         images' share of the shared allocator"
    );
}

/// The ceiling is stated in *typical* glyphs, so it has to be restated in the
/// size class actually being asked about — otherwise it counts map keys while
/// the thing it is rationing is texels.
#[test]
fn the_residency_ceiling_is_read_in_the_size_class_it_is_asked_about() {
    let typical = glyph_entry_ceiling(AtlasBudget::MOBILE.config());

    assert_eq!(
        entry_ceiling_at(typical, 16.0),
        typical,
        "the ceiling is stated in 16 px glyphs, so 16 px reads it unchanged"
    );
    assert_eq!(
        entry_ceiling_at(typical, 8.0),
        typical,
        "a smaller glyph does not earn a bigger population — the entry map's own \
         bookkeeping is what bounds the small end"
    );
    assert_eq!(
        entry_ceiling_at(typical, 32.0),
        typical / 4,
        "twice the size is four times the texels apiece"
    );
    assert_eq!(
        entry_ceiling_at(typical, MAX_CACHED_FONT_SIZE),
        typical / 64,
        "and the largest cacheable glyph is worth sixty-four typical ones"
    );
    assert_eq!(
        entry_ceiling_at(1, MAX_CACHED_FONT_SIZE),
        1,
        "never zero: a ceiling that admits nothing is the cache switched off"
    );
}

/// Large text may not spend the whole shared array before an entry count
/// notices, because the images are packed into the same one.
#[test]
fn sustained_large_text_leaves_the_images_their_share_of_the_allocator() {
    let mut residency = ImageResidency::new(AtlasBudget {
        atlas_size: (512, 512),
        max_atlases: 2,
    });
    let mut engine = AtlasPolicy::new(residency.allocator(), false);
    let font = next_font();

    // A page of the largest cacheable text, minting fresh glyphs every frame,
    // and no image asking for anything yet: text has every chance to take the
    // whole array before the image arrives.
    let mut next_glyph = 0_u32;
    let mut worst_entries = 0_usize;
    for _ in 0..40 {
        residency.begin_frame();
        engine.begin_frame();
        let classified = engine.classify_run(&run_key(font, MAX_CACHED_FONT_SIZE));
        let route = engine.admit_run(classified);
        if let Some(run) = route.atlas() {
            let run = *run;
            for _ in 0..4 {
                engine.collect_glyph(&run, next_glyph, 0.0);
                next_glyph += 1;
            }
        }
        engine.build(residency.allocator_mut(), raster);
        engine.acknowledge_clears();
        engine.end_frame(residency.allocator_mut());
        engine.acknowledge_replay();
        worst_entries = worst_entries.max(engine.entry_count());
    }

    let ceiling = entry_ceiling_at(engine.max_entries(), MAX_CACHED_FONT_SIZE);
    assert!(
        worst_entries <= ceiling + 4,
        "large text left {worst_entries} entries resident against a size-class \
         ceiling of {ceiling} (plus the one run in flight when it was reached)"
    );

    // And the image, asking last, still finds room.
    residency.begin_frame();
    assert!(
        residency.resolve(&image(256, 256, 0x11)).is_ok(),
        "an image arriving after sustained large text is refused the allocator \
         the text was only ever entitled to half of"
    );
    assert_eq!(residency.skipped(), 0);
}

/// A route decided before this frame's own insertions is re-tested against
/// them, so one frame cannot overshoot the ceiling by its whole self.
#[test]
fn a_route_is_re_tested_against_the_residency_the_frame_itself_created() {
    let mut small = pages();
    small.atlas_size = (256, 256);
    small.max_atlases = 1;
    let mut images = ImageCache::new_with_config(small);
    let mut engine = policy(&images, false);
    let ceiling = engine.max_entries();
    let font = next_font();

    engine.begin_frame();
    // `glifo` inserts nothing until a run is *drawn*, so every run of a frame is
    // classified against the population the frame opened with — here, empty.
    let first = engine.classify_run(&run_key(font, 16.0));
    let run = *first
        .atlas()
        .expect("an empty residency admits the first run");
    let later = engine.classify_run(&run_key(font, 16.0));
    assert!(
        later.atlas().is_some(),
        "fixture precondition: the collect walk answers both runs from the \
         frame-open count"
    );

    // The frame's own text, which in the engine is what `glifo` inserts as each
    // of those runs is drawn.
    for glyph in 0..(ceiling as u32 + 8) {
        engine.collect_glyph(&run, glyph, 0.0);
    }
    engine.build(&mut images, |_| {
        Some(RasterMetrics {
            width: 1,
            height: 1,
            bearing_x: 0,
            bearing_y: 1,
        })
    });
    assert!(
        engine.entry_count() >= ceiling,
        "fixture precondition: the frame's own text reached the ceiling (got {})",
        engine.entry_count()
    );

    assert_eq!(
        engine.admit_run(later).outline_reason(),
        Some(OutlineReason::ResidencyFull),
        "a route consumed after the ceiling was reached is narrowed to outlines \
         rather than honoured because the frame happened to start below it"
    );
}

/// A colour face is refused the atlas route outright, whatever it is drawn at.
#[test]
fn a_face_carrying_colour_glyphs_is_never_offered_the_atlas() {
    let images = cache();
    let mut engine = policy(&images, false);
    let font = next_font();

    engine.begin_frame();
    let mut key = run_key(font, 16.0);
    key.color_font = true;
    assert_eq!(
        engine.classify_run(&key).outline_reason(),
        Some(OutlineReason::ColorFont)
    );
    assert_eq!(
        engine.tracked_sizes(),
        0,
        "a face that can never take the route is not worth tracking sizes for"
    );

    // The same face at a size the guard would otherwise have cached on sight.
    engine.begin_frame();
    assert_eq!(
        engine.classify_run(&key).outline_reason(),
        Some(OutlineReason::ColorFont)
    );
    assert_eq!(engine.entry_count(), 0);
}

#[test]
fn a_static_page_uploads_on_its_first_frame_and_does_nothing_on_its_second() {
    let mut images = cache();
    let mut engine = policy(&images, false);
    let font = next_font();

    engine.begin_frame();
    let route = collect_hello(&mut engine, font, 16.0);
    assert!(
        route.atlas().is_some(),
        "a font drawn at a size nothing preceded is a first appearance, not a change"
    );
    let first = engine.build(&mut images, raster);
    assert_eq!(
        first.uploads.len(),
        4,
        "one upload per distinct glyph; `Hello`'s two `l`s share a key"
    );
    assert!(first.clears.is_empty(), "nothing has been evicted yet");
    assert_eq!(first.refused, 0);
    engine.end_frame(&mut images);

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
    let second = engine.build(&mut images, raster);
    assert!(
        second.is_empty(),
        "an unchanged page's atlas pass has nothing to do and can be skipped whole"
    );
    engine.end_frame(&mut images);

    // And it stays free.
    for _ in 0..200 {
        let pass = frame(&mut engine, &mut images, &[(font, 16.0)]);
        assert!(pass.is_empty(), "a page that never changes never uploads");
    }
    assert_eq!(engine.entry_count(), 4);
    assert_eq!(images.atlas_count(), 1);
}

#[test]
fn ten_thousand_frames_of_an_animated_size_keep_the_cache_bounded() {
    let mut images = cache();
    let mut engine = policy(&images, false);
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
        let pass = engine.build(&mut images, raster);
        uploads += pass.uploads.len();
        assert_eq!(pass.refused, 0, "a 2048-square page is not under pressure");
        engine.end_frame(&mut images);

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
        images.atlas_count(),
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
    let mut images = cache();
    let mut engine = policy(&images, false);
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
    let pass = engine.build(&mut images, raster);
    assert_eq!(
        pass.uploads.len(),
        1,
        "a key collected many times is allocated once"
    );
    engine.end_frame(&mut images);
}

#[test]
fn two_concurrent_sizes_of_one_font_settle_instead_of_reading_as_an_animation() {
    // A heading and its body text share a face and never move. Nothing here may
    // be mistaken for an animation, or the most ordinary screen in the
    // framework would never cache a glyph.
    let mut images = cache();
    let mut engine = policy(&images, false);
    let font = next_font();

    // The frame a second size first appears *is* a change, and is treated as
    // one — there is no way to tell it from the first frame of a size sweep.
    // What matters is that it settles.
    for _ in 0..(SETTLE_FRAMES + 4) {
        frame(&mut engine, &mut images, &[(font, 32.0), (font, 16.0)]);
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
        let pass = engine.build(&mut images, raster);
        uploads += pass.uploads.len();
        engine.end_frame(&mut images);
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
    let mut images = cache();
    let mut engine = policy(&images, false);

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
    let pass = engine.build(&mut images, raster);
    assert!(
        pass.uploads.is_empty(),
        "an oversized run collects nothing to upload"
    );
    engine.end_frame(&mut images);
}

#[test]
fn an_evicted_slot_is_cleared_before_its_rectangle_can_be_reused() {
    let mut images = cache();
    let mut engine = policy(&images, false);
    let font = next_font();

    frame(&mut engine, &mut images, &[(font, 16.0)]);
    assert_eq!(engine.entry_count(), 4);

    // The screen navigates away. `glifo` sweeps on its own schedule, so the
    // eviction frame is found rather than predicted — and until it happens
    // there is nothing to clear.
    let mut idle = 0;
    loop {
        let pass = frame(&mut engine, &mut images, &[]);
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
    let pass = frame(&mut engine, &mut images, &[(font, 16.0)]);
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

    // And a clear is not re-reported *once it has been acknowledged*: the queue
    // is offered rather than drained, and `frame` acknowledges every frame, so
    // the rects the previous pass carried were dropped when it said the writes
    // had really been issued. An unacknowledged frame keeps being offered the
    // same rects — that is the contract's whole point, and its own case below.
    let next = frame(&mut engine, &mut images, &[(font, 16.0)]);
    assert!(next.clears.is_empty());
}

#[test]
fn a_full_atlas_falls_back_to_outlines_rather_than_dropping_a_glyph() {
    // One tiny page, no growth: the smallest atlas that can hold a glyph or two
    // and then must refuse.
    let mut images = ImageCache::new_with_config(AtlasConfig {
        initial_atlas_count: 0,
        max_atlases: 1,
        atlas_size: (64, 64),
        auto_grow: false,
        allocation_strategy: AllocationStrategy::FirstFit,
    });
    let mut engine = AtlasPolicy::new(&images, false);
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
    let pass = engine.build(&mut images, raster);

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
    engine.end_frame(&mut images);
}

#[test]
fn nothing_resolves_to_a_slot_before_the_frame_has_been_built() {
    let mut images = cache();
    let mut engine = policy(&images, false);
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

    engine.build(&mut images, raster);
    assert!(
        engine.slot(&key).is_some(),
        "and has one immediately afterwards"
    );

    engine.end_frame(&mut images);
    assert!(
        engine.slot(&key).is_none(),
        "and cannot be resolved outside a frame at all"
    );
}

#[test]
fn collecting_outside_the_collect_phase_routes_to_outlines_rather_than_silently_failing() {
    let mut images = cache();
    let mut engine = policy(&images, false);
    let font = next_font();

    // Before any frame is opened.
    assert_eq!(
        engine.classify_run(&run_key(font, 16.0)).outline_reason(),
        Some(OutlineReason::NotCollecting)
    );

    engine.begin_frame();
    let route = engine.classify_run(&run_key(font, 16.0));
    let run = *route.atlas().expect("a first appearance is cached");
    engine.build(&mut images, raster);
    // And after the collect phase has closed.
    assert_eq!(
        engine.collect_glyph(&run, HELLO[0], 0.0),
        GlyphRoute::Outline(OutlineReason::NotCollecting)
    );
    engine.end_frame(&mut images);
}

#[test]
fn no_atlas_routes_every_glyph_to_outlines_and_allocates_nothing() {
    let mut images = cache();
    let mut engine = policy(&images, true);
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
        let pass = engine.build(&mut images, raster);
        assert!(pass.is_empty(), "a killed atlas has no pass to run");
        engine.end_frame(&mut images);
    }

    assert_eq!(engine.entry_count(), 0);
    assert_eq!(
        images.atlas_count(),
        0,
        "a killed atlas allocates no texture memory at all, and takes none out \
         of the shared allocator either"
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
    let mut images = cache();
    let mut engine = policy(&images, false);

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

        engine.build(&mut images, raster);
        engine.end_frame(&mut images);
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
    // sizes the residency's allocator, and `text::glyph_atlas_policy` hands
    // that same allocator to the policy — so pinning it here pins the geometry
    // the policy runs on.
    let mobile = AtlasBudget::for_caps(&TierCaps::fake(DownlevelProfile::WebGl2)).config();
    assert_eq!(mobile.atlas_size, (1024, 1024));
    assert_eq!(mobile.max_atlases, 4);

    let desktop = AtlasBudget::for_caps(&TierCaps::fake(DownlevelProfile::Full)).config();
    assert_eq!(desktop.atlas_size, (2048, 2048));
    assert_eq!(desktop.max_atlases, 8);

    // Nothing is allocated until a glyph or an image needs it, on either tier,
    // and the policy reports the allocator's geometry rather than a second copy
    // of the tier decision it could disagree with.
    for geometry in [mobile, desktop] {
        assert_eq!(geometry.initial_atlas_count, 0);
        let images = ImageCache::new_with_config(geometry);
        let engine = AtlasPolicy::new(&images, false);
        assert_eq!(images.atlas_count(), 0);
        assert_eq!(engine.pages().atlas_size, geometry.atlas_size);
        assert_eq!(engine.pages().max_atlases, geometry.max_atlases);
    }
}

#[test]
fn a_glyph_slot_and_an_image_slot_can_never_be_handed_the_same_image_id() {
    // The defect the shared allocator exists to make unrepresentable. An
    // `ImageId` is a slot index into one `ImageCache`, and the strip shader has
    // exactly one atlas texture array to resolve it against; two caches over
    // one geometry would mint `ImageId(0)` for a glyph and `ImageId(0)` for an
    // image and pack both into the same corner of layer 0.
    let mut residency = ImageResidency::new(AtlasBudget::DESKTOP);
    let mut engine = AtlasPolicy::new(residency.allocator(), false);
    let font = next_font();

    let mut ids: HashSet<u32> = HashSet::new();
    for round in 0..8_u8 {
        residency.begin_frame();
        // Distinct blobs, so each round makes a genuinely new image rather than
        // hitting the previous round's residency.
        for tag in 0..4_u8 {
            let resident = residency
                .resolve(&image(24, 24, round * 4 + tag))
                .expect("a 24-square image fits a desktop layer");
            assert!(
                ids.insert(resident.id.as_u32()),
                "image slot {:?} was already handed out",
                resident.id
            );
        }
        residency.acknowledge_plan();

        engine.begin_frame();
        collect_hello(&mut engine, font, 16.0 + f32::from(round));
        let pass = engine.build(residency.allocator_mut(), raster);
        for upload in &pass.uploads {
            assert!(
                ids.insert(upload.slot.image_id.as_u32()),
                "glyph slot {:?} collides with a slot already in use",
                upload.slot.image_id
            );
        }
        engine.end_frame(residency.allocator_mut());
    }

    assert!(
        ids.len() > 32,
        "fixture precondition: both classes really allocated ({} slots)",
        ids.len()
    );
}

#[test]
fn glyphs_and_images_are_held_against_one_shared_layer_budget() {
    // Two allocators over one geometry would each believe the whole budget was
    // theirs, so the array would need twice the layers the budget names — and
    // the shader addresses `atlas_index` in eight bits over one array. Sharing
    // makes the ceiling mean what it says: whichever class asks last is the one
    // refused.
    let mut residency = ImageResidency::new(AtlasBudget {
        atlas_size: (64, 64),
        max_atlases: 1,
    });
    let mut engine = AtlasPolicy::new(residency.allocator(), false);

    residency.begin_frame();
    residency
        .resolve(&image(64, 64, 1))
        .expect("one image fills the only layer");
    assert_eq!(residency.layers(), 1);

    engine.begin_frame();
    let route = engine.classify_run(&run_key(next_font(), 16.0));
    let run = *route.atlas().expect("a first appearance is cached");
    for glyph in HELLO {
        engine.collect_glyph(&run, glyph, 0.0);
    }
    let pass = engine.build(residency.allocator_mut(), raster);

    assert!(pass.uploads.is_empty(), "the image took the whole budget");
    assert_eq!(
        pass.refused, 4,
        "every collected glyph is refused against the shared ceiling rather \
         than allocated out of a private one"
    );
    assert_eq!(
        residency.layers(),
        1,
        "and no second layer appeared behind the budget's back"
    );
    engine.end_frame(residency.allocator_mut());

    // The converse: glyph pages are the residency's pages, so a layer text
    // created is one the image array has to be deep enough for.
    let mut residency = ImageResidency::new(AtlasBudget {
        atlas_size: (64, 64),
        max_atlases: 4,
    });
    let mut engine = AtlasPolicy::new(residency.allocator(), false);
    assert_eq!(residency.layers(), 0, "text has drawn nothing yet");

    engine.begin_frame();
    let route = engine.classify_run(&run_key(next_font(), 48.0));
    let run = *route.atlas().expect("a first appearance is cached");
    for glyph in 0..8_u32 {
        engine.collect_glyph(&run, glyph, 0.0);
    }
    let pass = engine.build(residency.allocator_mut(), raster);
    assert!(!pass.uploads.is_empty());
    assert!(
        residency.layers() >= 2,
        "fixture precondition: 48 px glyphs outgrow one 64-square layer"
    );
    engine.end_frame(residency.allocator_mut());
}

#[test]
fn evicting_one_class_leaves_the_others_slots_exactly_where_they_were() {
    // The two evictions are on separate clocks — `glifo`'s LRU serial and the
    // residency's frame age — and each frees only handles from its own entry
    // map. If either reached the other's, a live occupant's rectangle would be
    // handed to the next allocation while it was still being sampled.
    let mut residency = ImageResidency::new(AtlasBudget::DESKTOP);
    let mut engine = AtlasPolicy::new(residency.allocator(), false);
    let font = next_font();
    let picture = image(32, 32, 7);

    residency.begin_frame();
    let reaped = residency
        .resolve(&picture)
        .expect("fits a desktop layer")
        .region;
    residency.acknowledge_plan();
    let glyph_slots = frame(&mut engine, residency.allocator_mut(), &[(font, 16.0)])
        .uploads
        .iter()
        .map(|upload| (upload.key.clone(), upload.slot))
        .collect::<Vec<_>>();
    assert_eq!(glyph_slots.len(), 4, "fixture precondition: `Hello` cached");

    // The image goes unseen long enough to be reaped, while the text keeps
    // drawing.
    for _ in 0..=MAX_UNSEEN_FRAMES {
        residency.begin_frame();
        frame(&mut engine, residency.allocator_mut(), &[(font, 16.0)]);
    }
    assert_eq!(residency.entry_count(), 0, "the image aged out");
    assert_eq!(
        residency.evictions(),
        &[reaped],
        "the image class freed exactly its own rectangle"
    );
    // Resolved inside a frame, since a slot is only answerable in the draw
    // phase — and this frame must find every glyph already resident.
    engine.begin_frame();
    collect_hello(&mut engine, font, 16.0);
    let pass = engine.build(residency.allocator_mut(), raster);
    assert!(
        pass.uploads.is_empty(),
        "an image eviction re-rasterized text that never left the screen"
    );
    for (key, slot) in &glyph_slots {
        assert_eq!(
            engine
                .slot(key)
                .map(|live| (live.page_index, live.x, live.y)),
            Some((slot.page_index, slot.x, slot.y)),
            "an image eviction moved a glyph that was still on screen"
        );
    }
    engine.end_frame(residency.allocator_mut());

    // And the other way: the screen stops drawing text but keeps the image, so
    // `glifo`'s own sweep runs while the image is resident.
    residency.begin_frame();
    let resident = residency.resolve(&picture).expect("still fits");
    residency.acknowledge_plan();
    let mut idle = 0;
    while engine.entry_count() > 0 {
        residency.begin_frame();
        residency.resolve(&picture).expect("drawn every frame");
        frame(&mut engine, residency.allocator_mut(), &[]);
        idle += 1;
        assert!(idle < 500, "fixture precondition: eviction runs eventually");
    }

    let live = residency
        .allocator()
        .get(resident.id)
        .expect("a glyph eviction must not free an image's slot");
    assert_eq!(live.offsets(), resident.region.offset);
    assert_eq!(live.size(), resident.region.size);
    assert_eq!(
        residency.resolve(&picture).map(|again| again.region),
        Ok(resident.region),
        "and the image is still resident at the rectangle it was given"
    );
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

    let key = RunKey::for_run(&run, Affine::IDENTITY, false, false, BLACK);
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
    assert_eq!(
        RunKey::for_run(&second, Affine::IDENTITY, false, false, BLACK).font_id,
        key.font_id
    );

    // Hinting is part of the key: a hinted outline is a different bitmap.
    let mut images = cache();
    let mut engine = policy(&images, false);
    engine.begin_frame();
    let plain = engine.classify_run(&RunKey::for_run(
        &run,
        Affine::IDENTITY,
        false,
        false,
        BLACK,
    ));
    let hinted = engine.classify_run(&RunKey::for_run(&run, Affine::IDENTITY, true, false, BLACK));
    let plain = plain.atlas().expect("a first appearance is cached");
    let hinted = hinted.atlas().expect("hinting does not change the size");
    assert_ne!(
        plain.key(HELLO[0], 0.0),
        hinted.key(HELLO[0], 0.0),
        "a hinted glyph must not reuse the unhinted entry"
    );
    engine.end_frame(&mut images);
}

// ---------------------------------------------------------------------------
// The wired path: the same policy driven by the real `SceneCompiler`.
//
// Everything above pins the policy's *decisions* against a hand-driven frame.
// The cases below pin what those decisions do once a `frust_scene::Scene` is
// compiled through them — that a settled run reaches the atlas at all, that its
// second frame costs nothing, that a colour glyph is not tinted by the text
// colour, and that the kill switch really does put the outline path back.
//
// Still device-free. A cached glyph's *pixels* are produced by the render-to-
// atlas pass (`crate::gpu::atlas`), which needs a queue and lives with the GPU
// cases; what a compiled frame carries — which glyphs were routed to the
// atlas, the slot each draw names, the tint its paint applies and the
// rectangles an eviction freed — is the plan that pass executes, and is pinned
// here.
// ---------------------------------------------------------------------------

/// Viewport the wired cases compile against.
const VIEWPORT: (u16, u16) = (128, 64);

/// Font size the wired cases draw at: a whole number, so quantization is the
/// identity and the size a glyph is cached at is the size it was asked for.
const WIRED_SIZE: f32 = 24.0;

/// Noto Emoji, subsetted to one COLRv1 colour glyph — the same fixture
/// `text_color_hint.rs` names its colour cases against.
const EMOJI_FONT: &[u8] = include_bytes!("../../../testing/fonts/NotoEmoji-COLRv1-Subset.ttf");

/// [`EMOJI_FONT`]'s U+1F600, a COLRv1 base glyph with no outline of its own.
const EMOJI: u32 = 4;

/// A compiler with hinting off, so a run's route depends only on the policy.
fn wired_compiler() -> SceneCompiler {
    let mut compiler = SceneCompiler::new(VIEWPORT.0, VIEWPORT.1);
    compiler.set_hint_text(false);
    compiler
}

/// The bundled Latin face, as one process-wide handle.
///
/// One handle rather than one per scene, because a `peniko::Blob`'s id is what
/// the policy keys a font's *size history* on: two handles over the same bytes
/// are two fonts, and a sweep drawn through fresh handles would read as a
/// first appearance on every frame rather than as an animation.
fn latin_font() -> FontHandle {
    static FONT: std::sync::OnceLock<FontHandle> = std::sync::OnceLock::new();
    FONT.get_or_init(|| FontHandle::new(FontData::new(Blob::new(Arc::new(LATIN_FONT)), 0)))
        .clone()
}

/// The bundled COLRv1 emoji face, on the same terms as [`latin_font`].
fn emoji_font() -> FontHandle {
    static FONT: std::sync::OnceLock<FontHandle> = std::sync::OnceLock::new();
    FONT.get_or_init(|| FontHandle::new(FontData::new(Blob::new(Arc::new(EMOJI_FONT)), 0)))
        .clone()
}

/// A scene drawing `ids` from `font` at `size`, in `brush`, placed on the
/// baseline the wired cases use.
fn text_scene(font: FontHandle, ids: &[u32], size: f32, brush: Brush) -> Scene {
    text_scene_at(font, ids, size, brush, Affine::translate((8.0, 44.0)))
}

/// A scene drawing `ids` from `font` at `size`, in `brush`, under `transform`.
fn text_scene_at(
    font: FontHandle,
    ids: &[u32],
    size: f32,
    brush: Brush,
    transform: Affine,
) -> Scene {
    let run = GlyphRun {
        font,
        font_size: size,
        brush,
        transform,
        glyphs: ids
            .iter()
            .enumerate()
            .map(|(index, id)| Glyph {
                id: *id,
                x: index as f32 * 18.0,
                y: 0.0,
            })
            .collect(),
    };

    let mut scene = Scene::new();
    let mut builder = SceneBuilder::new(&mut scene);
    builder.draw_glyph_run(run);
    scene
}

/// `Hello` in black at [`WIRED_SIZE`].
fn hello_scene() -> Scene {
    text_scene(latin_font(), &HELLO, WIRED_SIZE, Brush::Solid(BLACK))
}

/// Compile `scene` through `compiler`, expecting it to be in range.
///
/// The compile alone. A frame the caller goes on to *service* also acknowledges
/// the atlas work it carried, which is [`compile_serviced`]'s job — this
/// variant is what a case modelling a refused frame wants.
fn compile(compiler: &mut SceneCompiler, scene: &Scene) -> CompiledFrame {
    compiler
        .compile(scene, Affine::IDENTITY, VIEWPORT)
        .expect("an in-range scene compiles")
}

/// Compile `scene` and acknowledge what a serviced frame acknowledges.
///
/// The ordinary case, and what `EngineRenderer::encode` does on every frame it
/// does not refuse: the evicted rectangles were zeroed and the recorded pages
/// were replayed, so the compiler may stop re-offering them. It matters to
/// eviction in particular — the policy defers `glifo`'s ageing pass while a
/// replay is outstanding, so a case that never acknowledged would be modelling
/// a surface whose every frame was refused.
fn compile_serviced(compiler: &mut SceneCompiler, scene: &Scene) -> CompiledFrame {
    let frame = compile(compiler, scene);
    compiler.acknowledge_glyph_clears();
    compiler.acknowledge_glyph_replay();
    frame
}

/// How many atlas pages `compiler` has recorded rasterization commands for,
/// draining them the way the render-to-atlas pass would — and acknowledging
/// the replay the same way it does.
fn drain_dirty_pages(compiler: &mut SceneCompiler) -> usize {
    let mut pages = 0;
    compiler
        .glyph_atlas_mut()
        .replay_pending_atlas_commands(|_| pages += 1);
    compiler.acknowledge_glyph_replay();
    pages
}

/// The tint `frame`'s one image paint applies, and whether there was exactly
/// one.
fn only_image_tint(frame: &CompiledFrame) -> Option<Option<vello_common::paint::Tint>> {
    let mut tints = frame.encoded_paints.iter().filter_map(|paint| match paint {
        EncodedPaint::Image(image) => Some(image.tint),
        _ => None,
    });
    let first = tints.next()?;
    tints.next().is_none().then_some(first)
}

#[test]
fn a_settled_run_draws_every_glyph_out_of_the_atlas() {
    let mut compiler = wired_compiler();
    let frame = compile(&mut compiler, &hello_scene());

    assert_eq!(frame.glyph_draws, 5, "every glyph of `Hello` is inked");
    assert_eq!(
        frame.atlas_glyph_draws, frame.glyph_draws,
        "a font drawn at a size nothing preceded is cached on its first frame, \
         so every glyph of it is an atlas draw"
    );
    assert_eq!(
        frame.glyph_slots.len(),
        frame.atlas_glyph_draws as usize,
        "every atlas draw reports the slot it names, so a recycled handle can \
         never be resolved against a previous occupant"
    );
    assert_eq!(
        compiler.glyph_atlas_entries(),
        4,
        "one entry per distinct glyph; `Hello`'s two `l`s share a key"
    );
    assert!(
        frame.atlas_layers >= 1,
        "the array has to be deep enough for the page text created"
    );

    // Every atlas draw is an image paint over the slot, not the run's solid.
    let images = frame
        .encoded_paints
        .iter()
        .filter(|paint| matches!(paint, EncodedPaint::Image(_)))
        .count();
    assert_eq!(images, frame.atlas_glyph_draws as usize);
}

#[test]
fn a_static_page_rasterizes_on_its_first_frame_and_nothing_on_its_second() {
    let mut compiler = wired_compiler();
    let scene = hello_scene();

    let first = compile(&mut compiler, &scene);
    assert!(first.atlas_glyph_draws > 0);
    assert_eq!(
        drain_dirty_pages(&mut compiler),
        1,
        "the first frame misses every glyph, so exactly one page is dirtied"
    );
    let entries = compiler.glyph_atlas_entries();

    // The whole point of the cache: the second frame of an unchanged page
    // rasterizes nothing at all, and every glyph is still drawn.
    for frame_index in 0..200 {
        let frame = compile(&mut compiler, &scene);
        assert_eq!(
            frame.atlas_glyph_draws, 5,
            "frame {frame_index} lost a glyph out of the atlas"
        );
        assert_eq!(
            drain_dirty_pages(&mut compiler),
            0,
            "frame {frame_index} re-rasterized a page that never changed"
        );
        assert!(
            frame.glyph_clears.is_empty(),
            "frame {frame_index} evicted a glyph it is still drawing"
        );
        assert_eq!(compiler.glyph_atlas_entries(), entries);
    }
}

#[test]
fn the_kill_switch_puts_the_outline_path_back() {
    let mut compiler = wired_compiler();
    // The same thing `FRUST_ENGINE_NO_ATLAS` selects, reached without a process
    // -global environment variable a test cannot un-set for its siblings.
    compiler.set_image_residency(ImageResidency::disabled(AtlasBudget::MOBILE));
    assert!(!compiler.glyph_atlas_enabled());

    let frame = compile(&mut compiler, &hello_scene());

    assert_eq!(
        frame.glyph_draws, 5,
        "a killed atlas costs pixels nothing: every glyph is still drawn"
    );
    assert_eq!(frame.atlas_glyph_draws, 0, "…as outline strips, not slots");
    assert!(frame.glyph_slots.is_empty());
    assert_eq!(compiler.glyph_atlas_entries(), 0);
    assert!(
        frame.encoded_paints.is_empty(),
        "a solid run drawn as outlines encodes no side-table paint at all"
    );
    assert!(
        frame
            .draws()
            .iter()
            .all(|draw| matches!(draw.paint, vello_common::paint::Paint::Solid(_))),
        "every glyph paints with the run's own inline colour"
    );
}

#[test]
fn an_animating_size_never_reaches_the_atlas_through_the_compiler() {
    let mut compiler = wired_compiler();

    // The first frame is a first appearance, not a change, so it caches.
    let first = compile(&mut compiler, &hello_scene());
    assert!(first.atlas_glyph_draws > 0);
    let settled = compiler.glyph_atlas_entries();

    // Then the size moves, and every frame of the sweep is outlines.
    for index in 0..SETTLE_FRAMES {
        let size = WIRED_SIZE + 0.5 * (index as f32 + 1.0);
        let scene = text_scene(latin_font(), &HELLO, size, Brush::Solid(BLACK));
        let frame = compile(&mut compiler, &scene);
        assert_eq!(
            frame.atlas_glyph_draws, 0,
            "a size that moved on frame {index} must not mint a fresh entry"
        );
        assert_eq!(frame.glyph_draws, 5, "…and must still draw every glyph");
    }
    assert_eq!(
        compiler.glyph_atlas_entries(),
        settled,
        "the sweep added nothing to the cache"
    );
}

/// A run inside a snapshot bracket is routed against the transform it is drawn
/// through, so a genuinely skewed one cannot reach the atlas by way of the
/// bracket's presentation scale.
///
/// The bracket is where the frame's two walks could disagree: the draw walk
/// enters a `PushSnapshot` whose composed transform leaves the device grid with
/// a neutral presentation instead of the recorded one, and a collect walk that
/// entered with the recorded one would classify every run inside against a
/// different affine. `absorbed_scale`'s skew tolerance is absolute, so the two
/// magnitudes can land on opposite sides of it — and an `Atlas` answer for a
/// skewed run is not a slower draw but a wrong one: `glifo` falls back to the
/// full transform while still probing its cache with the unabsorbed size, and a
/// hit there paints an unrotated bitmap.
#[test]
fn a_skewed_run_inside_a_snapshot_bracket_never_routes_to_the_atlas() {
    let bracket = Rect::new(0.0, 0.0, 128.0, 64.0);
    // A skew well past the 1/4096 tolerance at either magnitude, so the answer
    // is "uncacheable" and not a rounding accident.
    let skewed = Affine::translate((8.0, 44.0)) * Affine::skew(0.4, 0.0);

    let mut scene = Scene::new();
    let mut builder = SceneBuilder::new(&mut scene);
    // A presentation scale, so the bracket really does install a correction —
    // without one `snapshot_correction` is the identity and the case would pass
    // for the wrong reason.
    builder.push_snapshot(1, bracket, 1.0, 1.5);
    builder.draw_glyph_run(GlyphRun {
        font: latin_font(),
        font_size: WIRED_SIZE,
        brush: Brush::Solid(BLACK),
        transform: skewed,
        glyphs: HELLO
            .iter()
            .enumerate()
            .map(|(index, id)| Glyph {
                id: *id,
                x: index as f32 * 18.0,
                y: 0.0,
            })
            .collect(),
    });
    builder.pop_snapshot();

    let mut compiler = wired_compiler();
    let frame = compile_serviced(&mut compiler, &scene);

    assert_eq!(
        frame.atlas_glyph_draws, 0,
        "a skewed run is refused the atlas route whatever the bracket around it \
         presents at"
    );
    assert!(frame.glyph_draws > 0, "…and still draws, as outlines");
    assert_eq!(
        compiler.glyph_atlas_entries(),
        0,
        "a refused run mints no entry either — the route is refused before \
         `glifo` is ever handed the cache"
    );

    // Negative control: the same bracket, the same size, an unskewed run. If
    // this did not cache, the case above would be proving nothing.
    let mut scene = Scene::new();
    let mut builder = SceneBuilder::new(&mut scene);
    builder.push_snapshot(1, bracket, 1.0, 1.5);
    builder.draw_glyph_run(GlyphRun {
        font: latin_font(),
        font_size: WIRED_SIZE,
        brush: Brush::Solid(BLACK),
        transform: Affine::translate((8.0, 44.0)),
        glyphs: HELLO
            .iter()
            .enumerate()
            .map(|(index, id)| Glyph {
                id: *id,
                x: index as f32 * 18.0,
                y: 0.0,
            })
            .collect(),
    });
    builder.pop_snapshot();

    let mut compiler = wired_compiler();
    let frame = compile_serviced(&mut compiler, &scene);
    assert!(
        frame.atlas_glyph_draws > 0,
        "fixture precondition: the bracket itself does not close the atlas route"
    );
}

#[test]
fn an_atlas_glyph_carries_the_text_colour_as_an_alpha_mask_tint() {
    // An outline glyph is a coverage mask: the shader fills it with the run's
    // own colour, which is what an alpha-mask tint means.
    let mut compiler = wired_compiler();
    let frame = compile_serviced(
        &mut compiler,
        &text_scene(latin_font(), &HELLO[..1], WIRED_SIZE, Brush::Solid(WHITE)),
    );
    let tint = only_image_tint(&frame)
        .expect("one glyph, one atlas draw, one image paint")
        .expect("an outline glyph is tinted with the run's colour");
    assert_eq!(tint.mode, TintMode::AlphaMask);
    assert_eq!(tint.color, WHITE);
}

/// The defect this file's colour cases exist for, on the wired path: a
/// COLR-carrying face must never reach the atlas at all, and its glyphs must
/// still be drawn.
///
/// Cached, a COLR glyph is recorded by `glifo` as `push_clip_path` around a
/// colour-layer stream, into the command recorder *shared by every glyph on
/// its atlas page*. The engine's replay lowers solid fills and nothing else, so
/// it refuses that page whole — and `glifo` clears a recorder's commands
/// whether or not the replay took them, while leaving every entry on the page
/// resident. The Latin glyphs sharing that page therefore keep resolving to
/// slots whose texels were never written: a transparent glyph, on every frame
/// after the first, for as long as the entry survives. `glifo` 0.3.0 exposes no
/// way to withdraw an entry after the fact, so the only place to close it is
/// before the insertion happens.
///
/// The mixed frame is the point. A case drawing the emoji alone would pass on a
/// fix that merely dropped the colour glyph; what has to hold is that the
/// ordinary text sharing the frame is unharmed.
#[test]
fn a_colour_face_never_reaches_the_atlas_and_its_glyphs_still_draw() {
    let mut compiler = wired_compiler();

    // One frame, two runs, both newly cached: a COLR emoji and a line of Latin
    // — exactly the pairing that would have shared one page.
    let mut scene = Scene::new();
    {
        let mut builder = SceneBuilder::new(&mut scene);
        builder.draw_glyph_run(GlyphRun {
            font: emoji_font(),
            font_size: WIRED_SIZE,
            brush: Brush::Solid(WHITE),
            transform: Affine::translate((8.0, 44.0)),
            glyphs: vec![Glyph {
                id: EMOJI,
                x: 0.0,
                y: 0.0,
            }],
        });
        builder.draw_glyph_run(GlyphRun {
            font: latin_font(),
            font_size: WIRED_SIZE,
            brush: Brush::Solid(BLACK),
            transform: Affine::translate((40.0, 44.0)),
            glyphs: HELLO
                .iter()
                .enumerate()
                .map(|(index, id)| Glyph {
                    id: *id,
                    x: index as f32 * 14.0,
                    y: 0.0,
                })
                .collect(),
        });
    }

    let first = compile_serviced(&mut compiler, &scene);

    assert_eq!(
        compiler.glyph_atlas_entries(),
        4,
        "only the four distinct Latin glyphs may be resident — a COLR entry \
         here is an entry whose page the replay cannot lower"
    );
    assert_eq!(
        first.atlas_glyph_draws, 5,
        "the Latin run keeps the atlas; the colour glyph does not take it"
    );
    // The colour glyph is still drawn, as engine strips through `text::color`'s
    // layer recombination — degraded to the outline path, never to nothing.
    assert!(
        first.glyph_draws > first.atlas_glyph_draws,
        "the colour glyph must still produce draws of its own"
    );
    assert_eq!(
        first.skipped_glyphs, 0,
        "a colour glyph routed to outlines is drawn, not skipped"
    );

    // And the frame after, which is where the defect showed: every glyph the
    // atlas holds still resolves, and the page is not re-rasterized.
    assert_eq!(
        drain_dirty_pages(&mut compiler),
        1,
        "the Latin glyphs dirtied exactly one page, and it lowers"
    );
    let second = compile_serviced(&mut compiler, &scene);
    assert_eq!(
        second.atlas_glyph_draws, 5,
        "a glyph that was cached on frame one must still resolve on frame two"
    );
    assert_eq!(
        drain_dirty_pages(&mut compiler),
        0,
        "nothing missed on the second frame, so no page is dirtied"
    );
    assert_eq!(second.glyph_draws, first.glyph_draws);
    assert_eq!(second.skipped_glyphs, 0);
}

#[test]
fn an_idle_glyph_is_evicted_and_its_rectangle_carried_into_the_next_frames_clears() {
    let mut compiler = wired_compiler();
    let first = compile_serviced(&mut compiler, &hello_scene());
    assert!(first.atlas_glyph_draws > 0);
    assert_eq!(compiler.glyph_atlas_entries(), 4);

    // Nothing draws for long enough that `glifo`'s own LRU reaps the page.
    let empty = Scene::new();
    let mut clears = 0;
    for _ in 0..(MAX_ENTRY_AGE + EVICTION_FREQUENCY + 2) {
        let frame = compile_serviced(&mut compiler, &empty);
        clears += frame.glyph_clears.len();
    }

    assert_eq!(
        compiler.glyph_atlas_entries(),
        0,
        "an idle page must be given back to the shared allocator"
    );
    assert_eq!(
        clears, 4,
        "every evicted rectangle is carried into a later frame's clears, so it \
         is zeroed before whatever is handed it next composites onto it"
    );
}

/// `glifo` dirties a page when it *inserts*, so a run drawn entirely off the
/// target still leaves commands that have to reach the array.
///
/// The counterexample the replay gate was written from: gating the render-to-
/// atlas pass on surviving draws skips a scrolled-away run's whole page, and
/// the commands stay recorded. Left there long enough, the entries they name
/// age out, their rectangles are freed and re-let, and the replay finally runs
/// — painting the old run's ink into whichever glyphs were given those
/// rectangles.
#[test]
fn a_run_whose_draws_are_all_culled_still_leaves_a_page_to_replay() {
    let mut compiler = wired_compiler();
    // Scrolled far below the viewport. Every glyph is still resolved, keyed and
    // inserted — the culling happens downstream, on the rectangle each atlas
    // draw would have covered.
    let scene = text_scene_at(
        latin_font(),
        &HELLO,
        WIRED_SIZE,
        Brush::Solid(BLACK),
        Affine::translate((8.0, 4_000.0)),
    );

    let frame = compile(&mut compiler, &scene);

    assert_eq!(
        frame.atlas_glyph_draws, 0,
        "fixture precondition: no draw of this run survives the target's bounds"
    );
    assert_eq!(
        compiler.glyph_atlas_entries(),
        4,
        "fixture precondition: the run was still routed to the atlas and cached"
    );
    assert!(
        compiler.glyph_replay_pending(),
        "a page dirtied at insertion has pixel work outstanding however many of \
         its draws survived"
    );
    assert_eq!(
        drain_dirty_pages(&mut compiler),
        1,
        "the culled run recorded exactly one page's worth of fills"
    );
}

/// The frame's *own* recording counts as unreplayed too, because the replay
/// pass runs after the compile that records it.
///
/// The residual of the same defect: a deferral gated only on the *latched*
/// previous state lets every frame whose predecessor was acknowledged run
/// `glifo`'s eviction pass against its own fresh page commands — freeing and
/// re-letting the very rectangles those commands still name, which is the
/// ordering the deferral exists to make impossible.
#[test]
fn a_frames_own_recording_defers_its_own_eviction_pass() {
    let mut images = cache();
    let mut engine = policy(&images, false);
    let font = next_font();

    // Every frame mints a fresh key, and every frame acknowledges the previous
    // frame's replay — so the latched flag is clear at each `end_frame` and the
    // only thing outstanding is what this frame itself just recorded.
    let frames = MAX_ENTRY_AGE + EVICTION_FREQUENCY + 8;
    for index in 0..frames {
        engine.begin_frame();
        let run = *engine
            .classify_run(&run_key(font, 16.0))
            .atlas()
            .expect("residency is nowhere near its ceiling");
        // A fresh glyph id per frame, so every frame misses and every frame
        // therefore closes with page commands of its own outstanding.
        engine.collect_glyph(&run, index as u32, 0.0);
        let pass = engine.build(&mut images, raster);
        assert!(
            pass.clears.is_empty(),
            "a rectangle freed while this frame's own commands are unreplayed is \
             a rectangle those commands can be replayed into"
        );
        engine.acknowledge_clears();
        engine.end_frame(&mut images);
        engine.acknowledge_replay();
    }

    assert_eq!(
        engine.entry_count(),
        frames as usize,
        "nothing may be reaped on a frame whose own recording is still \
         outstanding, however long ago the previous replay landed"
    );

    // And ageing resumes the moment a frame records nothing of its own.
    let mut clears = 0;
    for _ in 0..(MAX_ENTRY_AGE + EVICTION_FREQUENCY + 2) {
        clears += frame(&mut engine, &mut images, &[]).clears.len();
    }
    assert!(
        clears > 0,
        "an idle frame leaves nothing outstanding, so the LRU runs again"
    );
}

/// A rectangle whose ink has not been replayed yet may not be freed.
///
/// The other half of the same defect: `glifo` clears a recorder's commands only
/// when they are replayed, so an unreplayed page keeps naming its slots. If the
/// LRU reaps those entries in the meantime the rectangles are handed to other
/// glyphs, and the replay — whenever it finally runs — writes the old glyph's
/// ink over them. Ageing is therefore deferred until the replay is
/// acknowledged, and resumes the moment it is.
#[test]
fn eviction_waits_for_a_replay_that_has_not_happened_yet() {
    let mut compiler = wired_compiler();

    // A frame that missed every glyph and was then refused before its replay.
    compile(&mut compiler, &hello_scene());
    let entries = compiler.glyph_atlas_entries();
    assert_eq!(entries, 4, "fixture precondition: the page was cached");
    assert!(compiler.glyph_replay_pending());

    // Idle for twice as long as it takes the LRU to reap a whole page.
    let empty = Scene::new();
    for _ in 0..(MAX_ENTRY_AGE + EVICTION_FREQUENCY) * 2 {
        let frame = compile(&mut compiler, &empty);
        assert!(
            frame.glyph_clears.is_empty(),
            "a rectangle freed under an unreplayed command is a rectangle that \
             command can be replayed into"
        );
    }
    assert_eq!(
        compiler.glyph_atlas_entries(),
        entries,
        "no entry may be reaped while the commands naming its slot are still \
         recorded"
    );

    // The replay lands. Ageing resumes and the idle page is given back.
    assert_eq!(drain_dirty_pages(&mut compiler), 1);
    let mut clears = 0;
    for _ in 0..(MAX_ENTRY_AGE + EVICTION_FREQUENCY + 2) {
        clears += compile_serviced(&mut compiler, &empty).glyph_clears.len();
    }
    assert_eq!(
        compiler.glyph_atlas_entries(),
        0,
        "deferral must delay the reap, not cancel it"
    );
    assert_eq!(clears, entries);
}

/// A compiled frame the caller never encodes must not consume the eviction
/// clears.
///
/// Symmetric to `ImageResidency`'s pending plan, and for the identical reason:
/// the clears are the only thing that makes an eviction observable as absence
/// rather than as the dead glyph's pixels, and a frame that took them and was
/// then refused leaves the rectangle holding those pixels for as long as
/// whatever is let it next samples them.
#[test]
fn a_frame_dropped_before_encode_leaves_the_eviction_clears_pending() {
    let mut compiler = wired_compiler();
    compile_serviced(&mut compiler, &hello_scene());

    // Idle until the reap reports its rectangles — and drop that frame on the
    // floor, acknowledging only the replay, which is not this case's subject.
    let empty = Scene::new();
    let mut dropped = Vec::new();
    for _ in 0..(MAX_ENTRY_AGE + EVICTION_FREQUENCY + 2) {
        let frame = compile(&mut compiler, &empty);
        compiler.acknowledge_glyph_replay();
        if !frame.glyph_clears.is_empty() {
            dropped = frame.glyph_clears.clone();
            break;
        }
    }
    assert_eq!(
        dropped.len(),
        4,
        "fixture precondition: the idle page is evicted and reports its rects"
    );
    assert!(
        compiler.glyph_clears_pending(),
        "nothing wrote those rectangles, so they are still owed"
    );

    // The next frame is offered exactly the same rectangles.
    let next = compile(&mut compiler, &empty);
    assert_eq!(
        rect_keys(&next.glyph_clears),
        rect_keys(&dropped),
        "a refused frame's clears must be re-offered, not lost"
    );

    // Acknowledged, and only then, they stop being offered.
    compiler.acknowledge_glyph_clears();
    let after = compile(&mut compiler, &empty);
    assert!(
        after.glyph_clears.is_empty(),
        "an acknowledged clear is written and done — re-offering it forever \
         would zero a rectangle its next occupant had already moved into"
    );
}

/// A uniform scale animation over a text container keeps the atlas bounded and
/// the images resident.
///
/// The counterexample the device-space guard was written from. `glifo` absorbs
/// a run's uniform scale into the font size *before* it keys anything, so a
/// container scaling 1x to 3x mints a fresh key every frame while the display
/// list's own `font_size` never moves. A guard watching `font_size` sees a
/// settled run, routes it to the atlas on all 200 frames, and the entries pile
/// up in the allocator the images are packed into — which is how a text
/// animation turns into a missing image.
#[test]
fn a_scale_animation_over_text_bounds_the_atlas_and_keeps_images_resident() {
    let mut compiler = wired_compiler();

    let picture = image(64, 64, 0x5A);
    let mut scene = Scene::new();
    {
        let mut builder = SceneBuilder::new(&mut scene);
        builder.draw_image(&picture, kurbo::Rect::new(0.0, 0.0, 64.0, 64.0));
        builder.draw_glyph_run(GlyphRun {
            font: latin_font(),
            font_size: WIRED_SIZE,
            brush: Brush::Solid(BLACK),
            transform: Affine::translate((4.0, 40.0)),
            glyphs: HELLO
                .iter()
                .enumerate()
                .map(|(index, id)| Glyph {
                    id: *id,
                    x: index as f32 * 14.0,
                    y: 0.0,
                })
                .collect(),
        });
    }

    let mut worst_entries = 0_usize;
    for index in 0..200_u64 {
        // A sawtooth over 1x..3x that does not close on itself, so successive
        // cycles land on different `f32`s exactly as a wall-clock-interpolated
        // animation does.
        let scale = 1.0 + 2.0 * ((index as f64) * 0.037_182_8).fract();
        let frame = compiler
            .compile(&scene, Affine::scale(scale), VIEWPORT)
            .expect("an in-range scene compiles");
        compiler.acknowledge_image_plan();
        compiler.acknowledge_glyph_clears();
        compiler.acknowledge_glyph_replay();

        assert_eq!(
            frame.skipped_images, 0,
            "frame {index}: the image lost its atlas rectangle — glyph residency \
             spent the allocator the two classes share"
        );
        worst_entries = worst_entries.max(compiler.glyph_atlas_entries());
    }

    assert!(
        worst_entries <= ENTRY_BOUND,
        "a scale animation left {worst_entries} entries resident (bound \
         {ENTRY_BOUND}) — the size the guard watches is not the size `glifo` keys"
    );
    assert_eq!(
        compiler.images().skipped(),
        0,
        "no image may be refused residency across the animation"
    );
}

/// The identifying fields of each rect, for comparing two offers of the same
/// eviction — `PendingClearRect` carries no `PartialEq` of its own.
fn rect_keys(rects: &[glifo::PendingClearRect]) -> Vec<(u32, u16, u16, u16, u16)> {
    rects
        .iter()
        .map(|rect| (rect.page_index, rect.x, rect.y, rect.width, rect.height))
        .collect()
}

// ---------------------------------------------------------------------------
// The wired path on real hardware.
//
// One case, and it is the one a device-free assertion cannot make: that a
// glyph the atlas holds is *drawn* — rasterized by the replay pass into an
// array layer, then sampled back out by the scene pass through a paint the
// renderer resolved from the slot the frame reported. Every step of that is a
// value no host test observes: a page replayed into the wrong layer, a paint
// resolved against a stale rectangle, a tint applied in the wrong mode and a
// slot never registered at all are all blank or wrong *pixels* and nothing
// else.
//
// It is also where the atlas path is measured against the outline one. The two
// are not byte-identical by construction — a cached glyph is rasterized once at
// its quantized size into a pixel-aligned slot and sampled at nearest, while an
// outline glyph is rasterized per frame at its exact subpixel position — so the
// bar is a band, and the band is the measurement.
// ---------------------------------------------------------------------------

/// Serializes every case in this binary that creates a GPU device, for the
/// reason `encode_contract.rs` documents at length: a driver that serializes
/// device teardown on a process-global mutex deadlocks when two of them tear
/// down at once. Poison is ignored deliberately — one case's failure must not
/// cascade into its siblings.
static RENDER_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn render_lock() -> std::sync::MutexGuard<'static, ()> {
    RENDER_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The target extent the GPU case renders at.
const GPU_SIZE: u32 = 128;

/// The target format the GPU case renders to.
const GPU_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Blocks on `future` by polling it to completion — this crate has no async
/// runtime, and wgpu's native requests resolve without one.
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

/// Pops a validation error scope, pumping the device until the pop resolves.
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

/// A device plus the capabilities probed off the adapter it came from.
fn gpu() -> (wgpu::Device, wgpu::Queue, TierCaps) {
    block_on(async {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = wgpu::util::initialize_adapter_from_env_or_default(&instance, None)
            .await
            .expect("no compatible GPU adapter");
        println!("atlas churn adapter: {:?}", adapter.get_info());
        let caps = TierCaps::probe(&adapter);
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("frust-engine atlas churn device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                ..Default::default()
            })
            .await
            .expect("failed to create the device");
        (device, queue, caps)
    })
}

/// Renders `frames` frames of `scene` on one renderer and returns each frame's
/// pixels, with the glyph atlas on or off.
///
/// One renderer across the frames on purpose: the second frame of a page is
/// the one the whole cache exists for, and it is only meaningful against a
/// renderer that kept the first frame's atlas.
fn render_frames(scene: &Scene, atlas: bool, frames: usize) -> Vec<Vec<u8>> {
    let (device, queue, caps) = gpu();
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);

    let target = HeadlessTarget::new(&device, GPU_SIZE, GPU_SIZE, GPU_FORMAT);
    let mut renderer = EngineRenderer::new(&device, &caps, GPU_FORMAT, None)
        .expect("the engine builds on this device");
    if !atlas {
        renderer.set_image_residency(ImageResidency::disabled(renderer.atlas_budget()));
    }

    let mut captured = Vec::with_capacity(frames);
    for index in 0..frames {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("frust-engine atlas churn frame"),
        });
        renderer
            .encode(
                &device,
                &queue,
                &mut encoder,
                scene,
                EngineTarget {
                    view: target.view(),
                    format: GPU_FORMAT,
                    width: GPU_SIZE,
                    height: GPU_SIZE,
                    depth: None,
                    output: OutputAlpha::Premultiplied,
                },
                WHITE,
                Affine::IDENTITY,
            )
            .unwrap_or_else(|error| panic!("frame {index} was refused: {error:?}"));
        queue.submit([encoder.finish()]);
        renderer.end_frame(&queue);
        captured.push(target.read_back(&device, &queue));
    }

    let error = drain_error_scope(&device, scope);
    assert!(error.is_none(), "the frame raised {error:?}");
    captured
}

/// How many pixels of `pixels` are not the untouched white backdrop.
fn inked(pixels: &[u8]) -> usize {
    pixels.chunks_exact(4).filter(|p| *p != [255; 4]).count()
}

/// The largest per-channel difference between two frames, and how many pixels
/// differ at all.
fn divergence(left: &[u8], right: &[u8]) -> (u8, usize) {
    let mut worst = 0_u8;
    let mut differing = 0_usize;
    for (a, b) in left.chunks_exact(4).zip(right.chunks_exact(4)) {
        if a != b {
            differing += 1;
        }
        for (x, y) in a.iter().zip(b.iter()) {
            worst = worst.max(x.abs_diff(*y));
        }
    }
    (worst, differing)
}

/// The most a cached glyph's pixels may differ from the outline path's, per
/// channel: the corpus's own P1 bar.
///
/// Measured at **zero** on the NVIDIA T400 (Vulkan) rig — this fixture draws at
/// a whole font size on a whole-pixel baseline, so every glyph lands in subpixel
/// bucket zero and the slot the atlas rasterized carries the same coverage the
/// outline path produces, texel for texel. Stated as a band rather than as
/// byte-identity because that agreement is a property of *this* fixture: a
/// fractional baseline resolves through one of four subpixel buckets and would
/// round differently. Two units is what the golden corpus calls P1, and is
/// narrow enough that a glyph landing a whole pixel out, sampled from the wrong
/// slot, or tinted in the wrong mode fails it outright.
const GLYPH_DIVERGENCE_CHANNEL: u8 = 2;

/// The most of the frame a cached glyph's pixels may differ from the outline
/// path's, as a fraction of the whole target.
///
/// Also measured at zero. Half a percent of a 128-square frame is 82 pixels,
/// well under the run's own 431 inked ones, so a glyph shifted or missing on one
/// path fails this even where every individual channel stayed inside the bar
/// above.
const GLYPH_DIVERGENCE_PIXELS: f64 = 0.005;

#[test]
#[ignore = "requires a GPU (Vulkan/Metal); run with `cargo test -p frust-engine --test atlas_churn -- --ignored`"]
fn a_cached_glyph_is_drawn_from_the_atlas_and_stays_put_on_the_next_frame() {
    let _serialized = render_lock();
    let scene = hello_scene();

    let atlas = render_frames(&scene, true, 2);
    let outline = render_frames(&scene, false, 2);
    let total = (GPU_SIZE * GPU_SIZE) as usize;

    // The claim nothing device-free can make: the replay pass really produced
    // pixels, and the scene pass really sampled them.
    let atlas_ink = inked(&atlas[0]);
    assert!(
        atlas_ink > 0,
        "an atlas-routed run drew no ink at all — the slot was never filled, \
         never registered, or never sampled"
    );
    assert!(
        inked(&outline[0]) > 0,
        "fixture precondition: the outline path draws this run"
    );

    // The steady state, end to end: the second frame re-samples the slots the
    // first frame filled and lands on exactly the same pixels.
    assert_eq!(
        atlas[0], atlas[1],
        "a page held still must render identically on its second frame"
    );

    let (worst, differing) = divergence(&atlas[0], &outline[0]);
    let fraction = differing as f64 / total as f64;
    println!(
        "atlas vs outline: max |delta| {worst}, {differing} of {total} pixels differ \
         ({:.4}%), atlas ink {atlas_ink}",
        fraction * 100.0
    );
    assert!(
        worst <= GLYPH_DIVERGENCE_CHANNEL,
        "a cached glyph diverged from the outline path by {worst} per channel"
    );
    assert!(
        fraction <= GLYPH_DIVERGENCE_PIXELS,
        "a cached glyph moved {:.4}% of the frame — that is ink somewhere the \
         outline path put none, not resampling",
        fraction * 100.0
    );
}

/// The same defect in pixels: a colour emoji and Latin text newly cached on
/// the same frame both render, and both keep rendering.
///
/// The device-free case above pins the *decision* — that the colour face is
/// never offered the atlas. This pins the consequence nothing device-free can
/// see. Routed to the atlas, `glifo` records the emoji as a clip bracket into
/// the page recorder every glyph of that frame shares; the replay declines the
/// page whole, `glifo` clears the recorder regardless, and every entry on the
/// page — the Latin glyphs included — resolves to a slot whose texels were
/// never written. On screen that is a paragraph of *transparent* glyphs, which
/// is precisely the failure a compiled frame's counters cannot show: they are
/// atlas draws, they name real slots, and they paint nothing.
#[test]
#[ignore = "requires a GPU (Vulkan/Metal); run with `cargo test -p frust-engine --test atlas_churn -- --ignored`"]
fn a_colour_glyph_beside_latin_text_leaves_neither_of_them_transparent() {
    let _serialized = render_lock();

    let mut scene = Scene::new();
    {
        let mut builder = SceneBuilder::new(&mut scene);
        builder.draw_glyph_run(GlyphRun {
            font: emoji_font(),
            font_size: WIRED_SIZE,
            brush: Brush::Solid(BLACK),
            transform: Affine::translate((8.0, 48.0)),
            glyphs: vec![Glyph {
                id: EMOJI,
                x: 0.0,
                y: 0.0,
            }],
        });
        builder.draw_glyph_run(GlyphRun {
            font: latin_font(),
            font_size: WIRED_SIZE,
            brush: Brush::Solid(BLACK),
            transform: Affine::translate((8.0, 92.0)),
            glyphs: HELLO
                .iter()
                .enumerate()
                .map(|(index, id)| Glyph {
                    id: *id,
                    x: index as f32 * 16.0,
                    y: 0.0,
                })
                .collect(),
        });
    }

    // Three frames: the one that caches, and two that have to keep resolving
    // what it cached.
    let frames = render_frames(&scene, true, 3);
    let outline = render_frames(&scene, false, 1);

    let baseline = inked(&outline[0]);
    assert!(
        baseline > 0,
        "fixture precondition: this scene draws ink with the atlas off"
    );

    for (index, frame) in frames.iter().enumerate() {
        let ink = inked(frame);
        assert!(
            ink > 0,
            "frame {index} drew nothing at all — every glyph sampled a slot the \
             replay never wrote"
        );
        // Half the outline path's ink is far below anything a subpixel or
        // rasterization difference accounts for, and far above what losing
        // either run would leave.
        assert!(
            ink * 2 >= baseline,
            "frame {index} lost most of its ink ({ink} of {baseline}) — a run \
             went transparent rather than falling back to outlines"
        );
    }

    // And the frames after the caching one are the steady state, not a decay.
    assert_eq!(
        frames[1], frames[2],
        "a page held still must render identically once it has settled"
    );
}
