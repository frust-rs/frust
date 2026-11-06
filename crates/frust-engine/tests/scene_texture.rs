//! `Command::SceneTexture` on real hardware: a texture the engine never
//! created, sampled straight out of the caller's own allocation.
//!
//! Every claim here is one a device-free test cannot make. An external texture
//! is not packed, not uploaded and not addressed by an atlas rectangle — it is
//! bound into the strip pipeline's second image slot and read by `textureLoad`
//! — so the whole path is bind-group construction, a source-kind bit the
//! fragment stage branches on, and a device-to-texel transform. None of that
//! produces a wrong *value* when it is wrong; it produces a validation error,
//! an untouched target, or the wrong texture's pixels, and only a real queue
//! and a real read-back tell those apart.
//!
//! What each case pins:
//!
//! - **A registered texture draws its own texels, in place.** A checkerboard
//!   blitted one-for-one onto a translated destination rectangle, read back
//!   block by block. A transform composed in the wrong direction, an offset
//!   dropped, or an axis swapped fails here rather than merely looking odd.
//! - **An unregistered id draws nothing.** The frame reads back as its base
//!   colour, everywhere. This is the display list's documented contract, and
//!   the one that must not degrade into sampling whatever the binding last
//!   held.
//! - **Unbinding is immediate.** The same scene stops drawing the texture on
//!   the very next frame, which is what says the bind groups naming it were
//!   dropped rather than retained across frames.
//! - **Two textures in one frame each draw their own pixels.** The pipeline has
//!   one external binding, so this only holds if the frame's instances were
//!   split into runs and the binding re-set between them.
//!
//! The whole file runs under a validation error scope, so a bind group built
//! against the wrong derived layout, or a view `wgpu` refuses at that binding
//! type, names itself rather than showing up as a plausible-looking blank
//! frame.

#![cfg(not(target_arch = "wasm32"))]

use std::sync::{Mutex, MutexGuard, PoisonError};

use frust_engine::renderer::FramePass;
use frust_engine::{
    EngineRenderer, EngineTarget, OutputAlpha, SceneTextureFilter, SceneTextureHints,
};
use frust_gpu::{HeadlessTarget, SceneTextureId, Texture, TextureDesc, TierCaps};
use frust_scene::{Scene, SceneBuilder};
use kurbo::{Affine, Rect};
use peniko::Color;
use peniko::color::palette::css::{BLACK, BLUE, GREEN, RED};

/// Target extent every case renders at.
const SIZE: u32 = 64;

/// The target format. `Rgba8Unorm` reads back R, G, B, A in that order, so a
/// read-back needs no swizzling.
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Extent of every source texture, in texels.
const TEXTURE: u32 = 32;

/// Side of one checkerboard square, in texels. Four squares per axis at
/// [`TEXTURE`], which is enough that a sample taken at a square's centre is far
/// from every edge — so the assertions are about which texel was read, never
/// about how the filter blended two of them.
const BLOCK: u32 = 8;

/// Where the source texture is drawn, one-for-one and offset from the origin on
/// both axes: a translation dropped from the composed transform lands the
/// checkerboard at `(0, 0)` and fails, while a scale error changes the block
/// boundaries.
const DEST: Rect = Rect::new(8.0, 8.0, 40.0, 40.0);

/// Serializes every test in this binary that creates a GPU device: these tests
/// each build their own `wgpu::Device`, and a driver that serializes device
/// teardown on a process-global mutex deadlocks when two of them tear down at
/// once. Poison is ignored deliberately — one test's failure must not cascade
/// into every sibling.
static RENDER_LOCK: Mutex<()> = Mutex::new(());

fn render_lock() -> MutexGuard<'static, ()> {
    RENDER_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Blocks on `future` by polling it to completion.
///
/// wgpu's native adapter and device requests resolve without an executor
/// driving them, so a bare poll loop is enough; this crate has no async runtime
/// of its own.
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
        // The environment-aware initializer, so `WGPU_ADAPTER_NAME` picks the
        // GPU on a multi-adapter host instead of the run silently landing on
        // whichever one enumerates first.
        let adapter = wgpu::util::initialize_adapter_from_env_or_default(&instance, None)
            .await
            .expect("no compatible GPU adapter");
        println!(
            "frust-engine scene texture adapter: {:?}",
            adapter.get_info()
        );
        let caps = TierCaps::probe(&adapter);
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("frust-engine scene texture device"),
                required_features: wgpu::Features::empty(),
                required_limits: frust_gpu::test_device_limits(&adapter, &caps),
                ..Default::default()
            })
            .await
            .expect("failed to create the device");
        (device, queue, caps)
    })
}

/// The colour a checkerboard of `even`/`odd` squares holds at texel
/// `(x, y)` — the same rule the pixels are generated from and the read-back is
/// asserted against, stated once so a test cannot agree with itself by
/// repeating a mistake.
fn checker_at(x: u32, y: u32, even: Color, odd: Color) -> Color {
    if (x / BLOCK + y / BLOCK).is_multiple_of(2) {
        even
    } else {
        odd
    }
}

/// A [`TEXTURE`]-square checkerboard of `even`/`odd` squares, tightly packed
/// RGBA8.
///
/// Both colours are fully opaque, so premultiplied and straight alpha agree and
/// the read-back is about which texel was sampled rather than about how it was
/// blended.
fn checkerboard(even: Color, odd: Color) -> Vec<u8> {
    let mut pixels = Vec::with_capacity((TEXTURE * TEXTURE * 4) as usize);
    for y in 0..TEXTURE {
        for x in 0..TEXTURE {
            pixels.extend_from_slice(&checker_at(x, y, even, odd).to_rgba8().to_u8_array());
        }
    }
    pixels
}

/// Uploads `pixels` into a caller-owned texture — one the engine never sees the
/// creation of, which is the whole point of the path under test.
fn upload(device: &wgpu::Device, queue: &wgpu::Queue, pixels: &[u8]) -> Texture {
    upload_sized(device, queue, TEXTURE, pixels)
}

/// [`upload`] for a square texture `side` texels across.
fn upload_sized(device: &wgpu::Device, queue: &wgpu::Queue, side: u32, pixels: &[u8]) -> Texture {
    let desc = TextureDesc {
        width: side,
        height: side,
        format: FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        label: Some("scene texture source".to_string()),
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: desc.label.as_deref(),
        size: wgpu::Extent3d {
            width: desc.width,
            height: desc.height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: desc.format,
        usage: desc.usage,
        view_formats: &[],
    });
    queue.write_texture(
        texture.as_image_copy(),
        pixels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(side * 4),
            rows_per_image: Some(side),
        },
        wgpu::Extent3d {
            width: side,
            height: side,
            depth_or_array_layers: 1,
        },
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

    Texture::new(texture, view, desc)
}

/// One case's device, renderer and read-back target.
struct Harness {
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: EngineRenderer,
    target: HeadlessTarget,
}

impl Harness {
    fn new() -> Self {
        let (device, queue, caps) = gpu();
        let target = HeadlessTarget::new(&device, SIZE, SIZE, FORMAT);
        let renderer = EngineRenderer::new(&device, &caps, FORMAT, None)
            .expect("the engine builds on this device");

        Self {
            device,
            queue,
            renderer,
            target,
        }
    }

    /// Registers `texture` exactly as a host would: the id it minted for
    /// itself, its own extent, and a full-extent view.
    fn bind(&mut self, texture: &Texture) -> SceneTextureId {
        let id = texture.as_scene_texture();
        let (width, height) = texture.size();
        self.renderer
            .bind_texture(id, (width, height), texture.view().clone());
        id
    }

    /// [`Self::bind`] with registration-side hints.
    fn bind_with(&mut self, texture: &Texture, hints: SceneTextureHints) -> SceneTextureId {
        let id = texture.as_scene_texture();
        let (width, height) = texture.size();
        self.renderer
            .bind_texture_with(id, (width, height), texture.view().clone(), hints);
        id
    }

    /// Renders `scene` over an opaque black base and reads the target back.
    fn render(&mut self, scene: &Scene) -> Vec<u8> {
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frust-engine scene texture frame"),
            });
        self.renderer
            .encode(
                &self.device,
                &self.queue,
                &mut encoder,
                scene,
                EngineTarget {
                    view: self.target.view(),
                    format: FORMAT,
                    width: SIZE,
                    height: SIZE,
                    depth: None,
                    output: OutputAlpha::Premultiplied,
                },
                BLACK,
                Affine::IDENTITY,
            )
            .expect("a well-formed frame encodes");
        self.queue.submit([encoder.finish()]);
        self.renderer.end_frame(&self.queue);

        let error = drain_error_scope(&self.device, scope);
        assert!(error.is_none(), "the frame raised {error:?}");

        self.target.read_back(&self.device, &self.queue)
    }
}

/// A scene built by `record`, ready to render.
fn scene_of(record: impl FnOnce(&mut SceneBuilder<'_>)) -> Scene {
    let mut scene = Scene::new();
    let mut builder = SceneBuilder::new(&mut scene);
    record(&mut builder);
    scene
}

/// A scene drawing the texture `id` names over `dest`.
fn scene_drawing(id: SceneTextureId, dest: Rect) -> Scene {
    scene_of(|builder| builder.scene_texture(id.get(), dest))
}

/// The RGBA the read-back holds at device pixel `(x, y)`.
fn pixel(frame: &[u8], x: u32, y: u32) -> [u8; 4] {
    let start = ((y * SIZE + x) * 4) as usize;
    frame[start..start + 4]
        .try_into()
        .expect("a read-back pixel is four bytes")
}

fn rgba(color: Color) -> [u8; 4] {
    color.to_rgba8().to_u8_array()
}

/// Asserts that `frame` holds the checkerboard of `even`/`odd` squares over
/// [`DEST`], sampled at each square's centre.
fn assert_checkerboard(frame: &[u8], even: Color, odd: Color) {
    let origin = (DEST.x0 as u32, DEST.y0 as u32);
    let squares = TEXTURE / BLOCK;
    for by in 0..squares {
        for bx in 0..squares {
            let texel = (bx * BLOCK + BLOCK / 2, by * BLOCK + BLOCK / 2);
            let device = (origin.0 + texel.0, origin.1 + texel.1);
            assert_eq!(
                pixel(frame, device.0, device.1),
                rgba(checker_at(texel.0, texel.1, even, odd)),
                "square ({bx}, {by}) at device pixel {device:?}"
            );
        }
    }
}

/// Asserts that nothing outside [`DEST`] was painted.
fn assert_only_dest_painted(frame: &[u8]) {
    for probe in [(1, 1), (SIZE - 2, 1), (1, SIZE - 2), (SIZE - 2, SIZE - 2)] {
        assert_eq!(
            pixel(frame, probe.0, probe.1),
            rgba(BLACK),
            "the frame outside the destination rectangle is the base colour at {probe:?}"
        );
    }
}

#[test]
#[ignore = "requires a GPU (Vulkan/Metal); run with `cargo test -p frust-engine --test scene_texture -- --ignored`"]
fn a_registered_texture_draws_its_own_texels_over_the_destination() {
    let _guard = render_lock();
    let mut harness = Harness::new();
    let source = upload(&harness.device, &harness.queue, &checkerboard(RED, BLUE));
    let id = harness.bind(&source);
    assert_eq!(harness.renderer.bound_texture_count(), 1);

    let frame = harness.render(&scene_drawing(id, DEST));

    assert_checkerboard(&frame, RED, BLUE);
    assert_only_dest_painted(&frame);
}

#[test]
#[ignore = "requires a GPU (Vulkan/Metal); run with `cargo test -p frust-engine --test scene_texture -- --ignored`"]
fn an_unregistered_id_draws_only_the_base_colour() {
    let _guard = render_lock();
    let mut harness = Harness::new();
    // Minted but never registered: the scene names a texture the renderer has
    // no view for, which the display list documents as drawing nothing.
    let source = upload(&harness.device, &harness.queue, &checkerboard(RED, BLUE));
    assert_eq!(harness.renderer.bound_texture_count(), 0);

    let frame = harness.render(&scene_drawing(source.as_scene_texture(), DEST));

    assert!(
        frame
            .as_chunks::<4>()
            .0
            .iter()
            .all(|texel| *texel == rgba(BLACK)),
        "an unregistered id leaves the frame as its base colour"
    );
}

#[test]
#[ignore = "requires a GPU (Vulkan/Metal); run with `cargo test -p frust-engine --test scene_texture -- --ignored`"]
fn unbinding_a_texture_stops_the_very_next_frame_drawing_it() {
    let _guard = render_lock();
    let mut harness = Harness::new();
    let source = upload(&harness.device, &harness.queue, &checkerboard(RED, BLUE));
    let id = harness.bind(&source);
    let scene = scene_drawing(id, DEST);

    let drawn = harness.render(&scene);
    assert_checkerboard(&drawn, RED, BLUE);

    assert!(harness.renderer.unbind_texture(id).is_some());
    assert_eq!(harness.renderer.bound_texture_count(), 0);
    let after = harness.render(&scene);

    assert!(
        after
            .as_chunks::<4>()
            .0
            .iter()
            .all(|texel| *texel == rgba(BLACK)),
        "an unbound texture is not drawn from a retained bind group"
    );
}

#[test]
#[ignore = "requires a GPU (Vulkan/Metal); run with `cargo test -p frust-engine --test scene_texture -- --ignored`"]
fn two_textures_in_one_frame_each_draw_their_own_pixels() {
    let _guard = render_lock();
    let mut harness = Harness::new();
    let first = upload(&harness.device, &harness.queue, &checkerboard(RED, BLUE));
    let second = upload(&harness.device, &harness.queue, &checkerboard(GREEN, BLUE));
    let first_id = harness.bind(&first);
    let second_id = harness.bind(&second);
    assert_eq!(harness.renderer.bound_texture_count(), 2);

    // Side by side, each at half width, so one binding leaking into the other
    // run shows up as the wrong colour rather than as nothing at all.
    let left = Rect::new(0.0, 16.0, 32.0, 48.0);
    let right = Rect::new(32.0, 16.0, 64.0, 48.0);
    let scene = scene_of(|builder| {
        builder.scene_texture(first_id.get(), left);
        builder.scene_texture(second_id.get(), right);
    });

    let frame = harness.render(&scene);

    // The first square of each source, sampled at its centre: `RED` and
    // `GREEN` are what tell the two checkerboards apart.
    assert_eq!(
        pixel(&frame, 4, 20),
        rgba(RED),
        "the left rectangle draws the first texture"
    );
    assert_eq!(
        pixel(&frame, 36, 20),
        rgba(GREEN),
        "the right rectangle draws the second texture"
    );
}

#[test]
#[ignore = "requires a GPU (Vulkan/Metal); run with `cargo test -p frust-engine --test scene_texture -- --ignored`"]
fn a_nearest_hint_magnifies_without_filtering() {
    let _guard = render_lock();
    let mut harness = Harness::new();
    // An 8x8 checker of 1-texel squares, drawn at 4x over a 32x32 rectangle.
    let pixels: Vec<u8> = (0..8_u32)
        .flat_map(|y| {
            (0..8_u32).flat_map(move |x| {
                let color = if (x + y) % 2 == 0 { RED } else { BLUE };
                color.to_rgba8().to_u8_array()
            })
        })
        .collect();
    let source = upload_sized(&harness.device, &harness.queue, 8, &pixels);
    let id = harness.bind_with(
        &source,
        SceneTextureHints::new().with_filter(SceneTextureFilter::Nearest),
    );

    let frame = harness.render(&scene_drawing(id, DEST));

    let (ox, oy) = (DEST.x0 as u32, DEST.y0 as u32);
    for by in 0..8_u32 {
        for bx in 0..8_u32 {
            let expected = rgba(if (bx + by) % 2 == 0 { RED } else { BLUE });
            for dy in 0..4 {
                for dx in 0..4 {
                    assert_eq!(
                        pixel(&frame, ox + bx * 4 + dx, oy + by * 4 + dy),
                        expected,
                        "block ({bx}, {by}) is uniform at offset ({dx}, {dy})"
                    );
                }
            }
        }
    }
}

#[test]
#[ignore = "requires a GPU (Vulkan/Metal); run with `cargo test -p frust-engine --test scene_texture -- --ignored`"]
fn an_opaque_hint_routes_through_the_opaque_pass() {
    let _guard = render_lock();
    let mut harness = Harness::new();
    let source = upload(&harness.device, &harness.queue, &checkerboard(RED, BLUE));
    // Unmerged, the opaque strips are a pass of their own and so observable.
    harness.renderer.set_pass_merge(false);

    let id = harness.bind(&source);
    let frame = harness.render(&scene_drawing(id, DEST));
    assert_checkerboard(&frame, RED, BLUE);
    assert!(
        !harness
            .renderer
            .last_frame_passes()
            .contains(&FramePass::Opaque),
        "an unhinted texture is always blended: {:?}",
        harness.renderer.last_frame_passes()
    );

    harness.bind_with(&source, SceneTextureHints::new().assume_opaque(true));
    let frame = harness.render(&scene_drawing(id, DEST));
    assert!(
        harness
            .renderer
            .last_frame_passes()
            .contains(&FramePass::Opaque),
        "an asserted-opaque texture takes the depth-writing pass: {:?}",
        harness.renderer.last_frame_passes()
    );
    assert_checkerboard(&frame, RED, BLUE);
    assert_only_dest_painted(&frame);
}

#[test]
#[ignore = "requires a GPU (Vulkan/Metal); run with `cargo test -p frust-engine --test scene_texture -- --ignored`"]
fn rebinding_with_new_hints_at_the_same_extent_drops_the_held_plan() {
    let _guard = render_lock();
    let mut harness = Harness::new();
    let source = upload(&harness.device, &harness.queue, &checkerboard(RED, BLUE));
    let id = harness.bind(&source);
    let scene = scene_drawing(id, DEST);
    let usage = harness.target.texture().usage();

    let prepare = |harness: &mut Harness| {
        harness
            .renderer
            .prepare(&scene, SIZE, SIZE, usage, Affine::IDENTITY)
            .expect("the frame plans");
    };
    let encode_prepared = |harness: &mut Harness| {
        let mut encoder = harness
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frust-engine scene texture prepared frame"),
            });
        harness
            .renderer
            .encode_prepared(
                &harness.device,
                &harness.queue,
                &mut encoder,
                &scene,
                EngineTarget {
                    view: harness.target.view(),
                    format: FORMAT,
                    width: SIZE,
                    height: SIZE,
                    depth: None,
                    output: OutputAlpha::Premultiplied,
                },
                BLACK,
                Affine::IDENTITY,
            )
            .expect("a prepared frame encodes");
        harness.queue.submit([encoder.finish()]);
        harness.renderer.end_frame(&harness.queue);
    };

    // The same hints again leave the plan alone.
    prepare(&mut harness);
    let planned_at = harness.renderer.retained_frame();
    harness.bind(&source);
    encode_prepared(&mut harness);
    assert_eq!(harness.renderer.retained_frame(), planned_at);

    // New hints at the same extent make it stale: the frame is planned again.
    for hints in [
        SceneTextureHints::new().with_filter(SceneTextureFilter::Nearest),
        SceneTextureHints::new().assume_opaque(true),
    ] {
        prepare(&mut harness);
        let planned_at = harness.renderer.retained_frame();
        harness.bind_with(&source, hints);
        encode_prepared(&mut harness);
        assert_eq!(
            harness.renderer.retained_frame(),
            planned_at + 1,
            "{hints:?} changed what the plan encodes"
        );
    }
}
