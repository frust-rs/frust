//! What a desktop surface does to the engine: a 5K frame, a resize storm, two
//! surfaces on one device, a lost device, fractional display scale, and a
//! destination that is not opaque.
//!
//! Every other GPU suite in this crate renders a small frame once. The
//! pressures a laptop or a 5K display applies are different in kind — an
//! intermediate wider than any texture the engine will allocate, a window edge
//! dragged through hundreds of extents a second, more than one window on one
//! `wgpu::Device`, a GPU that goes away, a display scale that is not an
//! integer, and a compositor that reads the surface's alpha. Each is a design
//! rule this file holds the engine to:
//!
//! - **A layer wider than a page is banded, not refused** (E14), and each band
//!   renders its own column and nothing else. The planning half is host-only
//!   and runs in the ordinary workspace gate:
//!   [`schedule::pages::page_bands`](frust_engine::schedule::pages::page_bands)
//!   cuts a 5120-wide layer into equal column bands that tile it exactly. The
//!   rendering half is the `#[ignore]`d 5K case — the gate that says whether
//!   the scheduler asks for those bands — plus the two banded cases beside it,
//!   which say whether a band paints only what its own column covers. Those
//!   are separate because the 5K checkerboard is self-masking: it paints every
//!   column of the layer, so content wrongly replayed into a band is covered
//!   by the content that really belongs there and the frame still reads
//!   correctly.
//! - **Quantized, pooled intermediates survive a resize storm** (E13). Four
//!   hundred extents through `EngineRenderer::resize` must not turn the
//!   intermediate pool back into a plain allocator.
//! - **More than one surface per device** (E12). The shells are single-window
//!   today (`docs/LIMITATIONS.md`'s `desktop-single-window`), so nothing else
//!   in the tree would notice the engine growing a per-device assumption.
//! - **A lost device is an error, never a panic** (E17/R8), and a renderer
//!   built on a fresh device renders again — the pool is per surface and goes
//!   with the renderer that owned it.
//! - **Fractional display scale rides in the root transform** (E15), with the
//!   target itself sized in whole device pixels.
//! - **Destination alpha is never assumed to be one** (E16).
//!
//! ## The reference these cases compare against
//!
//! The corpus suite in `frust-testing` compares engine output against a
//! `vello_cpu` oracle. This crate cannot: `vello_cpu` is not one of its
//! dependencies, and adding one to run a stress case would put a second
//! rasterizer in the engine's own test graph. So the expected pixel here is
//! computed instead — every case paints axis-aligned, pixel-aligned,
//! solid-colour rectangles and samples their interiors, where source-over at a
//! known alpha has one arithmetic answer and antialiasing has no say. That
//! makes these cases weaker than the corpus at catching a *shape* error and
//! exactly as strong at catching the compositing, scaling and alpha errors
//! they are here for; a spatial error still shows, because every grid of
//! samples below reads a different colour per cell.

#![cfg(not(target_arch = "wasm32"))]

use std::sync::{Mutex, MutexGuard, PoisonError};

use frust_engine::schedule::PageConfig;
use frust_engine::schedule::pages::{MAX_PAGE_BANDS, PageBand, page_bands, page_ceiling};
use frust_engine::{EngineError, EngineRenderer, EngineTarget, OutputAlpha};
use frust_gpu::{
    AcquireAction, AcquireStatus, ContextOptions, HeadlessTarget, RenderContext, SurfaceEvent,
    SurfacePhase, TierCaps,
};
use frust_scene::{Scene, SceneBuilder};
use kurbo::{Affine, Rect};
use peniko::{Brush, Color};
use vello_common::geometry::RectU16;

/// The one target format every case renders into. `Rgba8Unorm` reads back R,
/// G, B, A in that order, so a readback needs no swizzling.
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// The per-channel bar every pixel assertion is made at.
///
/// Three levels covers the two rounding steps a half-alpha composite of 8-bit
/// colour can take (`255 * 0.5 = 127.5`, once per operand) and stays two
/// orders of magnitude below the distances the assertions are about — a
/// dropped layer, an unapplied scale or a forced-opaque alpha all move a
/// channel by a hundred levels or more.
const TOLERANCE: u8 = 3;

/// The 5K desktop extent the layer cases are about: an Apple 5K display's
/// native panel, and the first surface size in ordinary use whose root layer
/// exceeds the engine's own page ceiling.
const FIVE_K: (u32, u32) = (5120, 2880);

/// Opaque background every layer case composites onto.
const BACKDROP: Color = Color::from_rgba8(0, 0, 255, 255);

/// The two opaque block colours the sample grid alternates between, so a band
/// or a row read from the wrong place shows as the wrong colour rather than
/// passing by symmetry.
const BLOCKS: [Color; 2] = [
    Color::from_rgba8(255, 0, 0, 255),
    Color::from_rgba8(0, 255, 0, 255),
];

/// The opacity every isolated layer in this file composites at.
const LAYER_ALPHA: f32 = 0.5;

/// Cells per axis in the sample grid the 5K cases read.
const GRID: u32 = 4;

/// Serializes every test in this binary that creates a GPU device — the same
/// guard the crate's other GPU suites take, for the same driver-teardown
/// reason. Poison is ignored deliberately: one case's failure must not cascade
/// into its siblings.
static RENDER_LOCK: Mutex<()> = Mutex::new(());

fn render_lock() -> MutexGuard<'static, ()> {
    RENDER_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Blocks on `future` by polling it to completion.
///
/// wgpu's native adapter and device requests resolve without an executor
/// driving them, so a bare poll loop is enough; this crate has no async
/// runtime of its own.
fn block_on<F: std::future::Future>(future: F) -> F::Output {
    use std::task::{Context as TaskContext, Poll, Waker};

    let waker = Waker::noop();
    let mut cx = TaskContext::from_waker(waker);
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
    use std::task::{Context as TaskContext, Poll, Waker};

    let waker = Waker::noop();
    let mut cx = TaskContext::from_waker(waker);
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

/// A context whose device is already created, plus the capabilities its
/// adapter reported.
///
/// Through `frust_gpu::RenderContext` rather than a bare `wgpu` request, because two
/// of these cases are about the device itself: E12 needs two surfaces proven
/// to be on *one* device, and the lost-device case needs
/// [`DeviceHandle::first_uncaptured_error`](frust_gpu::DeviceHandle::first_uncaptured_error),
/// which only the handler `RenderContext` installs ever latches.
fn gpu_context() -> RenderContext {
    let mut context = RenderContext::with_options(ContextOptions {
        device_label: "frust-engine desktop stress device".to_string(),
        backends: None,
    });
    let handle = block_on(context.device()).expect("no compatible GPU adapter");
    println!(
        "frust-engine desktop-stress adapter: {:?}",
        handle.adapter.get_info()
    );
    context
}

/// The device, queue and capabilities of `context`'s already-created device.
fn handle(context: &mut RenderContext) -> (wgpu::Device, wgpu::Queue, TierCaps) {
    let handle = block_on(context.device()).expect("the device was created already");
    (
        handle.device.clone(),
        handle.queue.clone(),
        handle.caps.clone(),
    )
}

/// The RGBA bytes at `(x, y)` of a tightly packed `width`-wide readback.
fn pixel(pixels: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let index = ((y * width + x) * 4) as usize;
    pixels[index..index + 4]
        .try_into()
        .expect("a readback row holds four bytes per pixel")
}

/// Asserts the pixel at `(x, y)` matches `expected` within [`TOLERANCE`].
fn assert_pixel(pixels: &[u8], width: u32, x: u32, y: u32, expected: [u8; 4], what: &str) {
    let actual = pixel(pixels, width, x, y);
    let off = (0..4).any(|c| actual[c].abs_diff(expected[c]) > TOLERANCE);
    assert!(
        !off,
        "{what}: pixel ({x}, {y}) is {actual:?}, expected {expected:?} +/- {TOLERANCE}"
    );
}

/// `source` composited over `dest` at `alpha`, in the premultiplied 8-bit
/// convention the engine writes.
///
/// Both operands are given as opaque colours, which is every case in this file
/// but the translucent-destination one; that case computes its own operands.
fn over(source: Color, dest: Color, alpha: f32) -> [u8; 4] {
    let src = source.components;
    let dst = dest.components;
    let channel = |i: usize| {
        let value = alpha * src[i] + (1.0 - alpha) * dst[i] * dst[3];
        (value * 255.0).round().clamp(0.0, 255.0) as u8
    };
    let out_alpha = alpha + (1.0 - alpha) * dst[3];
    [
        channel(0),
        channel(1),
        channel(2),
        (out_alpha * 255.0).round().clamp(0.0, 255.0) as u8,
    ]
}

/// `color` as the engine writes an opaque or translucent clear of it: its own
/// components, premultiplied, in 8-bit.
fn premultiplied(color: Color) -> [u8; 4] {
    color
        .premultiply()
        .components
        .map(|channel| (channel * 255.0).round().clamp(0.0, 255.0) as u8)
}

/// The centre of grid cell `(column, row)` of a `width` x `height` frame.
fn cell_centre(width: u32, height: u32, column: u32, row: u32) -> (u32, u32) {
    (
        width * (2 * column + 1) / (2 * GRID),
        height * (2 * row + 1) / (2 * GRID),
    )
}

/// Which of [`BLOCKS`] cell `(column, row)` is painted with.
fn block(column: u32, row: u32) -> Color {
    BLOCKS[((column + row) % 2) as usize]
}

/// A `width` x `height` scene: an opaque backdrop, then one isolated layer
/// spanning `layer` holding a [`GRID`] x [`GRID`] checkerboard of opaque
/// blocks.
///
/// The layer is the whole point — at [`LAYER_ALPHA`] it cannot be inlined, so
/// the scheduler has to give it a page, and at 5K that page is wider than the
/// engine will allocate.
fn checkerboard(width: u32, height: u32, layer: Rect) -> Scene {
    let mut scene = Scene::new();
    let mut builder = SceneBuilder::new(&mut scene);
    builder.fill_rect(
        Rect::new(0.0, 0.0, f64::from(width), f64::from(height)),
        Brush::Solid(BACKDROP),
    );
    builder.push_layer(layer, LAYER_ALPHA);
    for row in 0..GRID {
        for column in 0..GRID {
            let x0 = f64::from(width * column / GRID);
            let x1 = f64::from(width * (column + 1) / GRID);
            let y0 = f64::from(height * row / GRID);
            let y1 = f64::from(height * (row + 1) / GRID);
            builder.fill_rect(Rect::new(x0, y0, x1, y1), Brush::Solid(block(column, row)));
        }
    }
    builder.pop_layer();
    scene
}

/// Renders `scene` into a fresh headless target of `size` and reads it back,
/// or answers the [`EngineError`] the frame was refused with.
///
/// The whole frame path a host drives: one renderer, one caller-owned encoder,
/// one submit, one `end_frame`. Validation is scoped, so a wrong attachment or
/// bind group names itself instead of producing a plausible-looking image.
fn render(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    caps: &TierCaps,
    scene: &Scene,
    size: (u32, u32),
    base: Color,
    root: Affine,
) -> Result<Vec<u8>, EngineError> {
    let (width, height) = size;
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let target = HeadlessTarget::new(device, width, height, FORMAT);
    let mut renderer =
        EngineRenderer::new(device, caps, FORMAT, None).expect("the engine builds on this device");

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("frust-engine desktop stress frame"),
    });
    let encoded = renderer.encode(
        device,
        queue,
        &mut encoder,
        scene,
        EngineTarget {
            view: target.view(),
            format: FORMAT,
            width,
            height,
            depth: None,
            output: OutputAlpha::Premultiplied,
        },
        base,
        root,
    );
    queue.submit([encoder.finish()]);
    renderer.end_frame(queue);

    let error = drain_error_scope(device, scope);
    assert!(error.is_none(), "the frame raised {error:?}");

    encoded?;
    Ok(target.read_back(device, queue))
}

// ---------------------------------------------------------------------------
// Host-only: the band plan a 5K root layer needs (E14).
// ---------------------------------------------------------------------------

#[test]
fn a_5k_root_layer_plans_column_bands_that_tile_it_exactly() {
    let caps = TierCaps::fake(frust_gpu::DownlevelProfile::Full);
    let config = PageConfig::default();
    let ceiling = page_ceiling(&config, &caps);
    let bounds = RectU16::new(
        0,
        0,
        u16::try_from(FIVE_K.0).expect("5120 is inside the device grid"),
        u16::try_from(FIVE_K.1).expect("2880 is inside the device grid"),
    );

    assert!(
        u32::from(bounds.width()) > ceiling,
        "the case only means anything while a 5K root layer really is over the ceiling"
    );

    let bands: Vec<PageBand> = page_bands(bounds, &config, &caps).expect("a 5K layer bands");
    assert!(bands.len() <= MAX_PAGE_BANDS);

    let mut x = bounds.x0;
    for band in &bands {
        assert_eq!(band.bounds.x0, x, "the bands abut with no gap or overlap");
        assert_eq!(band.bounds.y0, bounds.y0);
        assert_eq!(band.bounds.y1, bounds.y1);
        assert!(u32::from(band.bounds.width()) <= ceiling);
        assert_eq!(
            band.size, bands[0].size,
            "every band asks the pool for one extent, so they share one texture"
        );
        x = band.bounds.x1;
    }
    assert_eq!(x, bounds.x1, "the bands cover the layer exactly");
}

// ---------------------------------------------------------------------------
// (a) 5120x2880 with a root opacity layer.
// ---------------------------------------------------------------------------

/// The widest allocation the engine is asked for anywhere, under the layer
/// shape a 5K desktop actually records — and the gate that says the scheduler
/// bands it rather than refusing the frame.
///
/// A secondary case for *which column* a band holds, deliberately: the
/// checkerboard covers every column of the layer, so a band that also replayed
/// the columns left of it would have its ghosts overpainted by the blocks that
/// really belong there. The two banded cases below are the ones that see that,
/// and they render a surface that is as wide and far shorter.
#[test]
#[ignore = "requires a GPU (Metal/Vulkan) and a 5K-sized allocation; run with `cargo test -p frust-engine --test desktop_stress -- --ignored`"]
fn a_5k_frame_under_a_root_opacity_layer_matches_the_reference_on_a_sample_grid() {
    let _guard = render_lock();
    let mut context = gpu_context();
    let (device, queue, caps) = handle(&mut context);
    let (width, height) = FIVE_K;

    let scene = checkerboard(
        width,
        height,
        Rect::new(0.0, 0.0, f64::from(width), f64::from(height)),
    );
    let pixels = match render(
        &device,
        &queue,
        &caps,
        &scene,
        (width, height),
        BACKDROP,
        Affine::IDENTITY,
    ) {
        Ok(pixels) => pixels,
        Err(error) => panic!(
            "a {width}x{height} frame under one root opacity layer was refused with {error:?}. \
             The layer is wider than a single page, so `Schedule::build` bands it into column \
             pages rather than sizing it through `page_size`. An \
             `IntermediateTextureTooLarge` here is `page_bands` refusing the split itself — a \
             layer taller than the page ceiling (bands are columns, so height has no split to be \
             served by), or one needing more than `MAX_PAGE_BANDS` bands to cover. A \
             `SchedulerEscalation` is `band_rounds` finding no page group for a band, which it \
             asks `make_room` to cut an open round for before each one."
        ),
    };

    for row in 0..GRID {
        for column in 0..GRID {
            let (x, y) = cell_centre(width, height, column, row);
            assert_pixel(
                &pixels,
                width,
                x,
                y,
                over(block(column, row), BACKDROP, LAYER_ALPHA),
                "a 5K root opacity layer",
            );
        }
    }
}

#[test]
#[ignore = "requires a GPU (Metal/Vulkan) and a 5K-sized allocation; run with `cargo test -p frust-engine --test desktop_stress -- --ignored`"]
fn a_5k_target_renders_a_layer_that_fits_one_page_and_leaves_the_rest_of_the_frame_alone() {
    let _guard = render_lock();
    let mut context = gpu_context();
    let (device, queue, caps) = handle(&mut context);
    let (width, height) = FIVE_K;

    // The same frame with the layer narrowed to what one page holds, so the 5K
    // *target* — the widest colour attachment, depth attachment and readback
    // the engine is asked for anywhere — is proven independently of whether the
    // scheduler bands an over-wide layer.
    let ceiling = page_ceiling(&PageConfig::default(), &caps);
    let layer_width = ceiling.min(width);
    let scene = checkerboard(
        width,
        height,
        Rect::new(0.0, 0.0, f64::from(layer_width), f64::from(height)),
    );
    let pixels = render(
        &device,
        &queue,
        &caps,
        &scene,
        (width, height),
        BACKDROP,
        Affine::IDENTITY,
    )
    .expect("a 5K frame whose layer fits one page renders");

    for row in 0..GRID {
        for column in 0..GRID {
            let (x, y) = cell_centre(width, height, column, row);
            let expected = if x < layer_width {
                over(block(column, row), BACKDROP, LAYER_ALPHA)
            } else {
                // Outside the layer's own rectangle nothing is drawn at all, so
                // the backdrop is what a 5K attachment has to hold.
                premultiplied(BACKDROP)
            };
            assert_pixel(&pixels, width, x, y, expected, "a 5K target");
        }
    }
}

// ---------------------------------------------------------------------------
// (a3) A banded layer renders each column band from its own column alone.
// ---------------------------------------------------------------------------

/// The narrowest surface wide enough to band a layer drawn over all of it.
///
/// A band is a full-height *column*, so a case about which column a band holds
/// buys nothing from a 5K-tall attachment — [`FIVE_K`]'s own two cases above
/// are what prove the widest allocation. These are as wide as a 5K display and
/// as short as a page's floor, which keeps a case that renders several bands
/// cheap enough to sample densely.
const BANDED: (u32, u32) = (5120, 64);

/// Rows every banded case samples at: one in each strip row of the surface's
/// own tile grid, so a band that shifted a strip vertically as well as
/// horizontally cannot pass by reading one row twice.
const BANDED_ROWS: [u32; 2] = [8, 56];

/// A [`BANDED`] scene: an opaque backdrop, then one isolated layer over all of
/// it holding each `(x0, x1, colour)` as a full-height opaque fill.
///
/// Full-height and pixel-aligned deliberately — the case is about which
/// *column* a band paints, so every sample lands in a rectangle's interior
/// where source-over at [`LAYER_ALPHA`] has one arithmetic answer.
fn banded_scene(rects: &[(f64, f64, Color)]) -> Scene {
    let (width, height) = BANDED;
    let mut scene = Scene::new();
    let mut builder = SceneBuilder::new(&mut scene);
    builder.fill_rect(
        Rect::new(0.0, 0.0, f64::from(width), f64::from(height)),
        Brush::Solid(BACKDROP),
    );
    builder.push_layer(
        Rect::new(0.0, 0.0, f64::from(width), f64::from(height)),
        LAYER_ALPHA,
    );
    for (x0, x1, color) in rects {
        builder.fill_rect(
            Rect::new(*x0, 0.0, *x1, f64::from(height)),
            Brush::Solid(*color),
        );
    }
    builder.pop_layer();
    scene
}

/// What a sample at device `x` has to hold: the last rectangle covering it,
/// composited at [`LAYER_ALPHA`], and the bare backdrop where none does.
///
/// The reference this file computes rather than renders (see the module
/// header): every rectangle is opaque and pixel-aligned, so the last one to
/// cover a pixel is the only one that can be seen through the layer.
fn banded_expectation(rects: &[(f64, f64, Color)], x: u32) -> [u8; 4] {
    rects
        .iter()
        .rev()
        .find(|(x0, x1, _)| f64::from(x) >= *x0 && f64::from(x) < *x1)
        .map_or(premultiplied(BACKDROP), |(_, _, color)| {
            over(*color, BACKDROP, LAYER_ALPHA)
        })
}

/// Renders [`banded_scene`] and asserts every sample `samples` asks for against
/// [`banded_expectation`], failing first if the layer is not actually banded.
///
/// `samples` is handed the live adapter's own band plan, so a case can sample
/// around a band edge without hardcoding where the split falls.
fn assert_banded_columns(
    rects: &[(f64, f64, Color)],
    samples: impl FnOnce(&[PageBand]) -> Vec<u32>,
    what: &str,
) {
    let _guard = render_lock();
    let mut context = gpu_context();
    let (device, queue, caps) = handle(&mut context);
    let (width, height) = BANDED;

    let bounds = RectU16::new(
        0,
        0,
        u16::try_from(width).expect("5120 is inside the device grid"),
        u16::try_from(height).expect("64 is inside the device grid"),
    );
    let bands = page_bands(bounds, &PageConfig::default(), &caps)
        .expect("a layer over a 5K-wide surface bands");
    assert!(
        bands.len() > 1,
        "{what}: the case only means anything while the layer really is split \
         into columns"
    );

    let scene = banded_scene(rects);
    let pixels = match render(
        &device,
        &queue,
        &caps,
        &scene,
        BANDED,
        BACKDROP,
        Affine::IDENTITY,
    ) {
        Ok(pixels) => pixels,
        Err(error) => panic!("{what}: a {width}x{height} banded frame was refused with {error:?}"),
    };

    for x in samples(&bands) {
        for row in BANDED_ROWS {
            assert_pixel(&pixels, width, x, row, banded_expectation(rects, x), what);
        }
    }
}

#[test]
#[ignore = "requires a GPU (Metal/Vulkan) and a 5K-wide allocation; run with `cargo test -p frust-engine --test desktop_stress -- --ignored`"]
fn a_banded_layer_leaves_the_columns_its_own_geometry_never_covered_untouched() {
    // Deliberately NOT self-masking: the checkerboard case above paints every
    // column of the layer, so a band that also replayed the columns left of it
    // is overpainted by the content that really belongs there and the frame
    // still reads correctly. Here the layer's two rectangles are at its far
    // ends with nothing between them, so a band replaying the whole layer has
    // nothing to hide behind — the left rectangle would land at the second
    // band's own origin, in columns the scene drew nothing in.
    let rects = [(0.0, 240.0, BLOCKS[0]), (4880.0, 5120.0, BLOCKS[1])];
    assert_banded_columns(
        &rects,
        |bands| {
            let edge = u32::from(bands[1].bounds.x0);
            let mut samples = vec![120, 600, 1600, 2400, 5000];
            // The band's own first columns and the ones a clamped replay of
            // the left rectangle would reach, all of which the scene left
            // empty.
            samples.extend((0..8).map(|step| edge + step * 30));
            samples.extend([edge + 300, edge + 600, edge + 1200, edge + 2000]);
            samples
        },
        "a banded layer's empty columns",
    );
}

#[test]
#[ignore = "requires a GPU (Metal/Vulkan) and a 5K-wide allocation; run with `cargo test -p frust-engine --test desktop_stress -- --ignored`"]
fn a_rect_straddling_a_band_edge_renders_continuously_across_it() {
    // One rectangle crossing the split, sampled pixel by pixel either side of
    // it: the half in each band has to land at the same device columns it
    // would have on one whole-layer page, so the seam is invisible. The two
    // markers at the surface's edges are what carries the layer past the page
    // ceiling, so the middle rectangle is banded at all.
    //
    // The alpha-column half of the same straddle — a strip cut by a band edge
    // that falls *inside* a coverage tile — is asserted host-side, in
    // `renderer`'s own plan tests: a band edge here lands on the tile grid, so
    // no strip's coverage is cut in two at 5K.
    let rects = [
        (0.0, 40.0, BLOCKS[1]),
        (1280.0, 3840.0, BLOCKS[0]),
        (5080.0, 5120.0, BLOCKS[1]),
    ];
    assert_banded_columns(
        &rects,
        |bands| {
            let edge = u32::from(bands[1].bounds.x0);
            let mut samples: Vec<u32> = (edge.saturating_sub(8)..edge + 8).collect();
            // Both of the rectangle's own edges, the columns outside it that
            // a clamped replay would reach, and the far marker.
            samples.extend([20, 600, 1279, 1280, 2000, 3839, 3840, 4000, 4600, 5100]);
            samples
        },
        "a rect straddling a band edge",
    );
}

// ---------------------------------------------------------------------------
// (b) The resize storm (E13).
// ---------------------------------------------------------------------------

/// Extents the storm walks between: a small window and a 5K display.
const STORM_MIN: (u32, u32) = (800, 800);
const STORM_STEPS: usize = 400;

/// The most textures the intermediate pool may allocate across the whole
/// storm.
///
/// The layer the storm renders is one size throughout — a card fading in does
/// not resize with the window — so quantization puts every one of its pages on
/// one pool key and a pool that survives the storm allocates for it exactly
/// once. The budget is an order of magnitude above that and an order of
/// magnitude below the step count, which is the only bar worth setting: the
/// failure it guards against is not "a few extra allocations" but the pool
/// degenerating into a plain allocator, and that costs one allocation *per
/// step*.
const STORM_ALLOCATION_BUDGET: u64 = 40;

/// A deterministic 32-bit xorshift, so a failing storm replays exactly.
struct Storm(u32);

impl Storm {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0
    }

    /// The next extent, uniform enough over `[STORM_MIN, FIVE_K]` that the
    /// walk visits both ends and everything between.
    fn extent(&mut self) -> (u32, u32) {
        let width = STORM_MIN.0 + self.next() % (FIVE_K.0 - STORM_MIN.0 + 1);
        let height = STORM_MIN.1 + self.next() % (FIVE_K.1 - STORM_MIN.1 + 1);
        (width, height)
    }
}

#[test]
#[ignore = "requires a GPU (Metal/Vulkan) and renders 400 frames up to 5K; run with `cargo test -p frust-engine --test desktop_stress -- --ignored`"]
fn a_resize_storm_keeps_the_intermediate_pool_inside_its_allocation_budget() {
    let _guard = render_lock();
    let mut context = gpu_context();
    let (device, queue, caps) = handle(&mut context);
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);

    let mut renderer = EngineRenderer::new(&device, &caps, FORMAT, None)
        .expect("the engine builds on this device");

    // A fixed-size layer under a resizing window: the shape a fading card or a
    // dismissing sheet records, and the one that says whether the pool is
    // reused across a drag. It never changes size, so every page it asks for
    // quantizes to one key.
    let card = Rect::new(64.0, 64.0, 704.0, 544.0);
    let mut scene = Scene::new();
    {
        let mut builder = SceneBuilder::new(&mut scene);
        builder.push_layer(card, LAYER_ALPHA);
        builder.fill_rect(card, Brush::Solid(BLOCKS[0]));
        builder.pop_layer();
    }

    let mut storm = Storm(0x5eed_1234);
    let mut peak_keys = 0;
    let mut frames: Vec<f64> = Vec::with_capacity(STORM_STEPS);
    for step in 0..STORM_STEPS {
        let (width, height) = storm.extent();
        renderer.resize(&device, width, height);

        let target = HeadlessTarget::new(&device, width, height, FORMAT);
        // Timed from here: the frame itself — compile, schedule, record,
        // submit and wait — with the step's own target allocation left out,
        // since a real surface hands its swapchain texture over rather than
        // creating one.
        let started = std::time::Instant::now();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("frust-engine resize storm frame"),
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
                BACKDROP,
                Affine::IDENTITY,
            )
            .unwrap_or_else(|error| {
                panic!("storm step {step} at {width}x{height} was refused with {error:?}")
            });
        queue.submit([encoder.finish()]);
        renderer.end_frame(&queue);
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        frames.push(started.elapsed().as_secs_f64() * 1000.0);

        peak_keys = peak_keys.max(renderer.targets().stats().keys);
    }

    let error = drain_error_scope(&device, scope);
    assert!(error.is_none(), "the storm raised {error:?}");

    frames.sort_by(f64::total_cmp);
    let at = |q: f64| frames[((frames.len() - 1) as f64 * q) as usize];
    let stats = renderer.targets().stats();
    println!(
        "resize storm: {STORM_STEPS} steps, pool created={} reused={} evicted={} peak_keys={}, \
         frame p50={:.2} ms p95={:.2} ms max={:.2} ms",
        stats.created,
        stats.reused,
        stats.evicted,
        peak_keys,
        at(0.50),
        at(0.95),
        at(1.0)
    );
    assert!(
        stats.created <= STORM_ALLOCATION_BUDGET,
        "{STORM_STEPS} resizes allocated {} intermediates (budget {STORM_ALLOCATION_BUDGET}); \
         the pool is not surviving the storm",
        stats.created
    );
    assert!(
        peak_keys <= 2,
        "the scheduler keeps at most two live pages, so the pool should never hold more than two \
         parked keys at once; it held {peak_keys}"
    );
}

// ---------------------------------------------------------------------------
// (c) Two surfaces on one device (E12).
// ---------------------------------------------------------------------------

/// Frames the two-surface case alternates for.
const ALTERNATING_FRAMES: usize = 100;

#[test]
#[ignore = "requires a GPU (Metal/Vulkan); run with `cargo test -p frust-engine --test desktop_stress -- --ignored`"]
fn two_headless_targets_on_one_device_render_alternately_without_interfering() {
    let _guard = render_lock();
    let mut context = gpu_context();
    let (device, queue, caps) = handle(&mut context);
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);

    // One device, two surfaces, a renderer each — the arrangement a two-window
    // desktop app has, and the one the engine must not assume away.
    let size = (320, 240);
    let targets = [
        HeadlessTarget::new(&device, size.0, size.1, FORMAT),
        HeadlessTarget::new(&device, size.0, size.1, FORMAT),
    ];
    let mut renderers = [
        EngineRenderer::new(&device, &caps, FORMAT, None).expect("the first window's renderer"),
        EngineRenderer::new(&device, &caps, FORMAT, None).expect("the second window's renderer"),
    ];

    // Each window paints its own colour under its own layer, so a frame that
    // leaked from one renderer's pool into the other's target shows up as the
    // wrong colour rather than as nothing at all.
    let window = Rect::new(32.0, 32.0, 288.0, 208.0);
    let scenes: Vec<Scene> = BLOCKS
        .iter()
        .map(|colour| {
            let mut scene = Scene::new();
            {
                let mut builder = SceneBuilder::new(&mut scene);
                builder.fill_rect(
                    Rect::new(0.0, 0.0, f64::from(size.0), f64::from(size.1)),
                    Brush::Solid(BACKDROP),
                );
                builder.push_layer(window, LAYER_ALPHA);
                builder.fill_rect(window, Brush::Solid(*colour));
                builder.pop_layer();
            }
            scene
        })
        .collect();

    for frame in 0..ALTERNATING_FRAMES {
        let which = frame % 2;
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("frust-engine two-window frame"),
        });
        renderers[which]
            .encode(
                &device,
                &queue,
                &mut encoder,
                &scenes[which],
                EngineTarget {
                    view: targets[which].view(),
                    format: FORMAT,
                    width: size.0,
                    height: size.1,
                    depth: None,
                    output: OutputAlpha::Premultiplied,
                },
                BACKDROP,
                Affine::IDENTITY,
            )
            .unwrap_or_else(|error| panic!("window {which} frame {frame} refused: {error:?}"));
        queue.submit([encoder.finish()]);
        renderers[which].end_frame(&queue);
    }

    let error = drain_error_scope(&device, scope);
    assert!(error.is_none(), "the alternating frames raised {error:?}");

    for (which, target) in targets.iter().enumerate() {
        let pixels = target.read_back(&device, &queue);
        assert_pixel(
            &pixels,
            size.0,
            size.0 / 2,
            size.1 / 2,
            over(BLOCKS[which], BACKDROP, LAYER_ALPHA),
            "a window's own content after alternating frames",
        );
        assert_pixel(
            &pixels,
            size.0,
            4,
            4,
            premultiplied(BACKDROP),
            "a window's backdrop outside its layer",
        );
    }

    // Each renderer keeps its own pool: neither one's counters were advanced by
    // the other's frames, which is what "per surface" means here.
    for (which, renderer) in renderers.iter().enumerate() {
        let stats = renderer.targets().stats();
        println!(
            "window {which}: pool created={} reused={} in_use={}",
            stats.created, stats.reused, stats.in_use
        );
        assert_eq!(
            stats.in_use, 0,
            "every page was released at its round's end"
        );
        assert!(
            stats.created >= 1,
            "each window rendered its own isolated layer, so each pool allocated at least once"
        );
    }
}

// ---------------------------------------------------------------------------
// (d) A lost device (E17, R8).
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires a GPU (Metal/Vulkan) and destroys a device; run with `cargo test -p frust-engine --test desktop_stress -- --ignored`"]
fn a_destroyed_device_is_reported_rather_than_panicked_and_a_fresh_renderer_renders_again() {
    let _guard = render_lock();
    let size = (256, 192);
    let window = Rect::new(32.0, 32.0, 224.0, 160.0);
    let mut scene = Scene::new();
    {
        let mut builder = SceneBuilder::new(&mut scene);
        builder.fill_rect(
            Rect::new(0.0, 0.0, f64::from(size.0), f64::from(size.1)),
            Brush::Solid(BACKDROP),
        );
        builder.push_layer(window, LAYER_ALPHA);
        builder.fill_rect(window, Brush::Solid(BLOCKS[0]));
        builder.pop_layer();
    }

    let expected = over(BLOCKS[0], BACKDROP, LAYER_ALPHA);

    // A good frame first, so the failure below is the device going away and
    // not a scene the engine never rendered.
    let mut context = gpu_context();
    let (device, queue, caps) = handle(&mut context);
    let target = HeadlessTarget::new(&device, size.0, size.1, FORMAT);
    let mut renderer = EngineRenderer::new(&device, &caps, FORMAT, None)
        .expect("the engine builds on this device");
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("frust-engine pre-loss frame"),
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
                width: size.0,
                height: size.1,
                depth: None,
                output: OutputAlpha::Premultiplied,
            },
            BACKDROP,
            Affine::IDENTITY,
        )
        .expect("the frame before the loss renders");
    queue.submit([encoder.finish()]);
    renderer.end_frame(&queue);
    let before = target.read_back(&device, &queue);
    assert_pixel(
        &before,
        size.0,
        size.0 / 2,
        size.1 / 2,
        expected,
        "the frame before the device is destroyed",
    );
    assert!(
        block_on(context.device())
            .expect("the device is still live")
            .first_uncaptured_error()
            .is_none(),
        "a sound frame raises no uncaptured error"
    );

    // The loss. `Device::destroy` is the closest a test can get to a driver
    // reset without one: the handles stay valid and every operation through
    // them fails from here on.
    device.destroy();

    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("frust-engine post-loss frame"),
    });
    let refused = renderer.encode(
        &device,
        &queue,
        &mut encoder,
        &scene,
        EngineTarget {
            view: target.view(),
            format: FORMAT,
            width: size.0,
            height: size.1,
            depth: None,
            output: OutputAlpha::Premultiplied,
        },
        BACKDROP,
        Affine::IDENTITY,
    );
    queue.submit([encoder.finish()]);
    renderer.end_frame(&queue);
    let polled = device.poll(wgpu::PollType::wait_indefinitely());
    let scoped = drain_error_scope(&device, scope);
    let latched = block_on(context.device())
        .expect("the handle survives its device")
        .first_uncaptured_error()
        .map(str::to_string);
    println!(
        "post-loss: engine returned {refused:?}, scope {scoped:?}, poll {polled:?}, latched \
         {latched:?}"
    );

    // What a destroyed device does *not* do is raise: WebGPU makes every
    // operation on a lost device a no-op that raises no error, so the engine's
    // own result, the validation scope, the poll and the uncaptured-error
    // handler are all silent by specification — the queue simply drains empty.
    // The claim this case can therefore make about the engine is E17's: the
    // whole frame path returned rather than panicking, and it recorded
    // nothing, which is what keeps a lost device from taking the process with
    // it. The signal a host recovers on is the surface's, below.
    assert!(
        !matches!(polled, Ok(wgpu::PollStatus::WaitSucceeded)),
        "a destroyed device's queue must not report work still in flight; it reported {polled:?}"
    );

    // The surface half, which is what a shell actually watches: a `Lost`
    // acquire is classified, decided on, and moves the lifecycle machine to
    // `SurfaceLost`, where frames stop being rendered until the shell installs
    // a fresh surface. Driven here as pure values — `frust-render` maps its
    // `wgpu::SurfaceError` onto exactly this machine, and the engine crate
    // cannot depend on `frust-render` to watch it do so.
    assert_eq!(
        frust_gpu::lifecycle::decide_acquire(AcquireStatus::Lost, 0),
        AcquireAction::Lose
    );
    let phase = frust_gpu::lifecycle::next_phase(SurfacePhase::SurfaceReady, SurfaceEvent::Lost);
    assert_eq!(phase, SurfacePhase::SurfaceLost);
    assert!(
        !phase.can_render(),
        "no frame is encoded again until the shell installs a fresh surface"
    );

    // R8: the pool is per surface and went with the renderer that owned it, so
    // a renderer built on a fresh device renders the same scene again.
    drop(renderer);
    drop(target);
    drop(context);

    let mut fresh = gpu_context();
    let (device, queue, caps) = handle(&mut fresh);
    let after = render(
        &device,
        &queue,
        &caps,
        &scene,
        size,
        BACKDROP,
        Affine::IDENTITY,
    )
    .expect("a renderer on a fresh device renders again");
    assert_pixel(
        &after,
        size.0,
        size.0 / 2,
        size.1 / 2,
        expected,
        "the frame after recovery",
    );
}

// ---------------------------------------------------------------------------
// (e) Fractional display scale (E15).
// ---------------------------------------------------------------------------

/// The logical extent the display-scale case lays its page out at.
const LOGICAL: (f64, f64) = (400.0, 300.0);

/// The logical rectangle the case paints, and samples inside and outside of.
const SCALED_RECT: Rect = Rect::new(100.0, 75.0, 300.0, 225.0);

#[test]
#[ignore = "requires a GPU (Metal/Vulkan); run with `cargo test -p frust-engine --test desktop_stress -- --ignored`"]
fn a_fractional_display_scale_rides_in_the_root_transform_over_whole_device_pixels() {
    let _guard = render_lock();
    let mut context = gpu_context();
    let (device, queue, caps) = handle(&mut context);

    for dpr in [1.5_f64, 2.25] {
        // Whole device pixels with the fraction in the transform, never a
        // fractional attachment extent.
        let width = (LOGICAL.0 * dpr).round() as u32;
        let height = (LOGICAL.1 * dpr).round() as u32;

        let mut scene = Scene::new();
        {
            let mut builder = SceneBuilder::new(&mut scene);
            builder.push_layer(SCALED_RECT, LAYER_ALPHA);
            builder.fill_rect(SCALED_RECT, Brush::Solid(BLOCKS[0]));
            builder.pop_layer();
        }

        let pixels = render(
            &device,
            &queue,
            &caps,
            &scene,
            (width, height),
            BACKDROP,
            Affine::scale(dpr),
        )
        .unwrap_or_else(|error| panic!("the DPR {dpr} frame was refused with {error:?}"));

        let expected = over(BLOCKS[0], BACKDROP, LAYER_ALPHA);
        let backdrop = premultiplied(BACKDROP);

        // Well inside the scaled rectangle, and — the discriminating part —
        // also outside where the same rectangle would land unscaled, so a root
        // transform that never reached the draw shows as a backdrop pixel.
        let inside = (
            ((SCALED_RECT.x1 * dpr + SCALED_RECT.x1) / 2.0) as u32,
            ((SCALED_RECT.y1 * dpr + SCALED_RECT.y1) / 2.0) as u32,
        );
        assert!(
            f64::from(inside.0) > SCALED_RECT.x1 && f64::from(inside.0) < SCALED_RECT.x1 * dpr,
            "the sample must lie between the unscaled and scaled right edges"
        );
        assert_pixel(
            &pixels,
            width,
            inside.0,
            inside.1,
            expected,
            "inside the scaled rectangle but outside the unscaled one",
        );

        // Comfortably outside the scaled rectangle: still the backdrop, so an
        // over-applied scale shows too.
        let outside = (
            (SCALED_RECT.x1 * dpr) as u32 + 8,
            (SCALED_RECT.y1 * dpr) as u32 + 8,
        );
        assert!(outside.0 < width && outside.1 < height);
        assert_pixel(
            &pixels,
            width,
            outside.0,
            outside.1,
            backdrop,
            "outside the scaled rectangle",
        );
    }
}

// ---------------------------------------------------------------------------
// (f) A destination that is not opaque (E16).
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires a GPU (Metal/Vulkan); run with `cargo test -p frust-engine --test desktop_stress -- --ignored`"]
fn a_translucent_destination_is_composited_onto_rather_than_assumed_opaque() {
    let _guard = render_lock();
    let mut context = gpu_context();
    let (device, queue, caps) = handle(&mut context);

    let size = (256, 192);
    // A half-transparent backdrop — the surface a translucent window or an
    // Android `TranslucentPreferred` surface presents through — and a
    // half-transparent draw over it, so both operands carry alpha and neither
    // side can be right by accident.
    let base = Color::from_rgba8(0, 0, 255, 128);
    let source = Color::from_rgba8(255, 255, 255, 255);
    let rect = Rect::new(64.0, 48.0, 192.0, 144.0);

    let mut scene = Scene::new();
    {
        let mut builder = SceneBuilder::new(&mut scene);
        builder.fill_rect(rect, Brush::Solid(source.with_alpha(LAYER_ALPHA)));
    }

    let pixels = render(&device, &queue, &caps, &scene, size, base, Affine::IDENTITY)
        .expect("a translucent-destination frame renders");

    // Where nothing was drawn the frame keeps the backdrop's own alpha: a
    // renderer that forced the destination opaque would read 255 here.
    let untouched = premultiplied(base);
    assert_pixel(
        &pixels,
        size.0,
        8,
        8,
        untouched,
        "an undrawn translucent pixel",
    );

    // Where the half-alpha white landed, source-over against a destination
    // whose alpha is 0.5 — not 1.
    assert_pixel(
        &pixels,
        size.0,
        size.0 / 2,
        size.1 / 2,
        over(source, base, LAYER_ALPHA),
        "half-alpha white over a half-alpha backdrop",
    );
}
