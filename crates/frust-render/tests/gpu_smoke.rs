//! GPU smoke tests: render Frust scenes offscreen through
//! [`frust_render::HeadlessRenderer`] and read the pixels back. These exercise
//! the real `frust-engine` / wgpu 30 pipeline end to end, so they are
//! `#[ignore]`d and run manually on hardware with a GPU:
//!
//! ```text
//! cargo test -p frust-render --test gpu_smoke -- --ignored --nocapture
//! ```
//!
//! CI sandboxes without a GPU are expected to skip them; a local pass on a
//! deliberately selected adapter is the gate. On a multi-GPU host, name the
//! adapter (`WGPU_BACKEND=vulkan WGPU_ADAPTER_NAME=T400`) — the harness routes
//! adapter selection through the environment-aware initializer, so that knob
//! is honoured, and `FRUST_GOLDEN_EXPECT_ADAPTER`/`FRUST_GOLDEN_EXPECT_BACKEND`
//! turn "which GPU did this run on?" into a refusal rather than a footnote.
//! `--nocapture` prints the resolved adapter metadata each test records.
//!
//! Every case renders through the same public seams an app does (the harness
//! drives `frust_engine::EngineRenderer` exactly as `SurfaceRenderer::submit`
//! does), so the actual scene-to-pixels path is what gets validated, not a
//! hand-written engine frame; the harness itself brackets each render in a
//! wgpu `Validation` error scope and fails on any captured error, so an
//! uncaptured-validation-error assertion is no longer each test's own job.
//!
//! Pixels come back PREMULTIPLIED (the engine's own convention — see
//! [`frust_render::HeadlessImage`]), which the assertions below are written
//! against; every content pixel they probe is opaque, where the two
//! conventions agree, apart from the fully-erased ones the hole-punch case
//! checks.

use frust_render::{HeadlessOptions, HeadlessRenderer, HeadlessSpec};
use frust_scene::{Scene, SceneBuilder};
use peniko::Brush;
use peniko::color::palette::css::{BLACK, BLUE, GREEN, RED};

const SIZE: u32 = 64;

/// Creates the offscreen renderer and prints the GPU it resolved — the
/// provenance (`docs/TESTING.md` § GPU Run Metadata) every promoted baseline
/// and every failing artifact has to record.
async fn headless() -> HeadlessRenderer {
    let renderer = HeadlessRenderer::new(HeadlessOptions::default())
        .await
        .expect("failed to create the headless renderer");
    println!("frust gpu_smoke: {}", renderer.meta());
    renderer
}

#[test]
#[ignore = "requires a GPU; run locally with `cargo test -p frust-render -- --ignored`"]
fn red_fill_rect_produces_non_zero_pixels() {
    pollster::block_on(run());
}

async fn run() {
    let mut headless = headless().await;

    // Build a Frust scene with a full-surface red rect.
    let mut fk_scene = Scene::new();
    {
        let mut builder = SceneBuilder::new(&mut fk_scene);
        builder.fill_rect(
            kurbo::Rect::new(0.0, 0.0, SIZE as f64, SIZE as f64),
            Brush::Solid(RED),
        );
    }
    let image = headless
        .render(&fk_scene, &HeadlessSpec::new(SIZE, SIZE))
        .await
        .expect("headless render failed");

    let non_zero = image.rgba8.iter().any(|&b| b != 0);
    assert!(non_zero, "expected non-zero pixels from a red FillRect");

    // Spot-check the centre pixel is dominated by red.
    let [r, g, b, _] = image.pixel(SIZE / 2, SIZE / 2);
    assert!(
        r > g && r > b,
        "centre pixel should be red-dominant, got rgb=({r},{g},{b})"
    );
}

/// Pixel-level regression test for the Mode B hole punch: a `clear_rect`
/// recorded INSIDE a
/// clip/opacity layer group must still erase an opaque backdrop painted
/// OUTSIDE the group (a group-local erase would be sealed in by the group
/// composite), and the erase must be PIXEL-EXACT at a 16-px-tile-UNALIGNED
/// edge (both defects were first caught on-device).
#[test]
#[ignore = "requires a GPU; run locally with `cargo test -p frust-render -- --ignored`"]
fn clear_rect_punches_pixel_exact_through_a_layer_group() {
    pollster::block_on(run_clear_probe());
}

async fn run_clear_probe() {
    let mut headless = headless().await;

    // Opaque backdrop at ROOT; the punch inside a full-surface layer group,
    // with its right edge one px past a 16-px tile boundary (SIZE/2 + 1).
    let punch_edge = (SIZE / 2 + 1) as f64;
    let mut fk_scene = Scene::new();
    {
        let mut builder = SceneBuilder::new(&mut fk_scene);
        builder.fill_rect(
            kurbo::Rect::new(0.0, 0.0, SIZE as f64, SIZE as f64),
            Brush::Solid(RED),
        );
        builder.push_layer(kurbo::Rect::new(0.0, 0.0, SIZE as f64, SIZE as f64), 1.0);
        builder.clear_rect(kurbo::Rect::new(0.0, 0.0, punch_edge, SIZE as f64));
        builder.pop_layer();
    }
    let image = headless
        .render(
            &fk_scene,
            &HeadlessSpec {
                base_color: peniko::color::palette::css::TRANSPARENT,
                ..HeadlessSpec::new(SIZE, SIZE)
            },
        )
        .await
        .expect("headless render failed");

    let px = |x: u32| {
        let [r, g, b, a] = image.pixel(x, SIZE / 2);
        (r, g, b, a)
    };
    // Inside the punch: fully erased, through the group, over the backdrop.
    assert_eq!(px(SIZE / 4), (0, 0, 0, 0), "punch centre must be erased");
    assert_eq!(
        px(SIZE / 2),
        (0, 0, 0, 0),
        "last px inside the unaligned edge must be erased"
    );
    // Just past the edge, INSIDE the same 16-px tile: the backdrop must
    // survive untouched (the Compose::Clear tile bleed this test pins down).
    for x in [SIZE / 2 + 1, SIZE / 2 + 4, SIZE / 2 + 14] {
        let (r, _, _, a) = px(x);
        assert!(
            r > 200 && a > 200,
            "backdrop at x={x} (same tile as the punch edge) must stay opaque red, got {:?}",
            px(x)
        );
    }
}

/// The inline lowering of a `Command::PushSnapshot` bracket, pixel for pixel
/// — the only lowering this crate has for one.
///
/// Reached through the public headless seam alone: the bracket's
/// presentation scale as a transform correction and its sub-unity alpha as a
/// layer, which is what every render path and every tier produces. This pins
/// the absolute arithmetic that lowering must hit and the z-order of a command
/// recorded after the bracket.
///
/// The geometry keeps every content edge on a whole device pixel, so nothing
/// here is measuring rasterizer subpixel coverage.
#[test]
#[ignore = "requires a GPU; run locally with `cargo test -p frust-render -- --ignored`"]
fn snapshot_bracket_inline_emulation_matches_the_composite_arithmetic() {
    pollster::block_on(run_snapshot());
}

/// Per-pixel tolerance: an 8-bit rounding step, no more.
const SNAPSHOT_TOLERANCE: u8 = 3;

async fn run_snapshot() {
    // The bracket: a 20x20 body in local space under a 2x device scale offset
    // by (8, 8) — device (8, 8)..(48, 48) — composited at 75% opacity.
    const ALPHA: f32 = 0.75;
    let bracket = kurbo::Affine::translate((8.0, 8.0)) * kurbo::Affine::scale(2.0);
    let rect = kurbo::Rect::new(0.0, 0.0, 20.0, 20.0);
    let overlay = peniko::Color::from_rgba8(255, 255, 0, 255);

    let mut fk_scene = Scene::new();
    {
        let mut builder = SceneBuilder::new(&mut fk_scene);
        // An opaque black backdrop, which is what makes the composite
        // arithmetic below exact, plus a marker outside the bracket.
        builder.fill_rect(
            kurbo::Rect::new(0.0, 0.0, SIZE as f64, SIZE as f64),
            Brush::Solid(BLACK),
        );
        builder.fill_rect(kurbo::Rect::new(52.0, 52.0, 60.0, 60.0), Brush::Solid(BLUE));
        builder.push_transform(bracket);
        builder.push_snapshot(1, rect, ALPHA, 1.0);
        // An opaque red block, a half-alpha green band over NOTHING (so the
        // bracket really carries partial alpha — a double premultiply is
        // invisible where alpha is 1), and a black bar standing in for a line
        // of text.
        builder.fill_rect(kurbo::Rect::new(0.0, 0.0, 20.0, 10.0), Brush::Solid(RED));
        builder.fill_rect(
            kurbo::Rect::new(0.0, 10.0, 20.0, 15.0),
            Brush::Solid(GREEN.with_alpha(0.5)),
        );
        builder.fill_rect(kurbo::Rect::new(5.0, 15.0, 15.0, 20.0), Brush::Solid(BLACK));
        builder.pop_snapshot();
        builder.pop_transform();
        // Recorded AFTER the bracket, so it must land on top of it.
        builder.fill_rect(
            kurbo::Rect::new(20.0, 20.0, 30.0, 30.0),
            Brush::Solid(overlay),
        );
    }

    let mut headless = headless().await;
    let image = headless
        .render(&fk_scene, &HeadlessSpec::new(SIZE, SIZE))
        .await
        .expect("headless render failed");

    // The expected values are the composite arithmetic over the black
    // backdrop, not observations:
    //  - the opaque red block composites at the bracket's 0.75 -> 191,
    //  - the half-alpha green band (CSS green is 0x008000) composites at
    //    128 * 0.5 * 0.75 -> 48, which is the number a doubly premultiplied
    //    page would halve again.
    let red = image.pixel(40, 12);
    assert!(
        red[0].abs_diff(191) <= SNAPSHOT_TOLERANCE,
        "the bracket's opaque block must composite at its alpha, got {red:?}"
    );
    let green = image.pixel(40, 32);
    assert!(
        green[1].abs_diff(48) <= SNAPSHOT_TOLERANCE,
        "the bracket's half-alpha band must composite at 128 * 0.5 * 0.75, got {green:?}"
    );
    assert_eq!(
        image.pixel(24, 24),
        [255, 255, 0, 255],
        "a command recorded after the bracket must land on top of it"
    );
    let marker = image.pixel(56, 56);
    assert!(
        marker[2] > 200,
        "content recorded before the bracket must survive, got {marker:?}"
    );
}

/// The readback's row-padding path, on real hardware.
///
/// Every case above renders 64 px wide, where `64 * 4` is exactly wgpu's
/// 256-byte `copy_texture_to_buffer` row alignment and the padding arithmetic
/// is a no-op — which is why they were written at that size in the first
/// place. A width whose `width * 4` is NOT a multiple of 256 makes each copied
/// row carry trailing padding bytes, and a harness that failed to strip them
/// per row would return an image whose every row after the first is shifted:
/// still plausible pixels, silently wrong geometry. 97 px pads 388 bytes up to
/// 512, so a slip is visible on the very first row boundary.
///
/// The same renderer then renders a second, differently sized frame, covering
/// the device/engine-renderer/target reuse across renders.
#[test]
#[ignore = "requires a GPU; run locally with `cargo test -p frust-render -- --ignored`"]
fn unaligned_width_reads_back_without_row_padding() {
    pollster::block_on(run_row_padding());
}

async fn run_row_padding() {
    // 97 * 4 = 388 bytes of pixels per row, padded to 512.
    const PAD_WIDTH: u32 = 97;
    const PAD_HEIGHT: u32 = 33;
    /// Right edge of the red block, on a whole device pixel so the column
    /// either side of it is exact rather than anti-aliased.
    const EDGE: u32 = 48;

    let mut headless = headless().await;

    let mut fk_scene = Scene::new();
    {
        let mut builder = SceneBuilder::new(&mut fk_scene);
        builder.fill_rect(
            kurbo::Rect::new(0.0, 0.0, EDGE as f64, PAD_HEIGHT as f64),
            Brush::Solid(RED),
        );
    }
    let image = headless
        .render(&fk_scene, &HeadlessSpec::new(PAD_WIDTH, PAD_HEIGHT))
        .await
        .expect("headless render failed");

    assert_eq!(
        image.rgba8.len(),
        (PAD_WIDTH * PAD_HEIGHT * 4) as usize,
        "the image must carry tightly packed rows, with no padding"
    );
    for y in 0..PAD_HEIGHT {
        let inside = image.pixel(EDGE - 1, y);
        assert!(
            inside[0] > 200 && inside[1] < 64 && inside[2] < 64,
            "row {y} must still be red up to the block's edge, got {inside:?}"
        );
        assert_eq!(
            image.pixel(EDGE, y),
            [0, 0, 0, 255],
            "row {y} must be the base color just past the block's edge"
        );
        assert_eq!(
            image.pixel(PAD_WIDTH - 1, y),
            [0, 0, 0, 255],
            "row {y}'s last pixel must be the base color, not a neighbouring row's"
        );
    }

    // A second render at a different size, through the same device and engine
    // renderer, must be just as correct.
    let mut square = Scene::new();
    {
        let mut builder = SceneBuilder::new(&mut square);
        builder.fill_rect(
            kurbo::Rect::new(0.0, 0.0, SIZE as f64, SIZE as f64),
            Brush::Solid(RED),
        );
    }
    let reused = headless
        .render(&square, &HeadlessSpec::new(SIZE, SIZE))
        .await
        .expect("headless render failed on a reused renderer");
    let [r, g, b, _] = reused.pixel(SIZE / 2, SIZE / 2);
    assert!(
        r > 200 && g < 64 && b < 64,
        "a reused renderer must still render, got rgb=({r},{g},{b})"
    );
}
