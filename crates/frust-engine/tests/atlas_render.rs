//! The render-to-atlas pass on real hardware: glyph pixels produced *by the
//! GPU*, into one layer of the atlas array, ahead of the frame that samples
//! them.
//!
//! Every claim here is one a device-free test cannot make. The atlas replay is
//! a pass whose colour attachment is a single array layer of a texture the
//! scene pass also samples, drawn through a pipeline derived from the strip
//! shader, and submitted on an encoder of its own — none of which produces a
//! wrong *value* when it is wrong. It produces a validation error, an untouched
//! layer, or a glyph in the wrong place, and only a real queue and a real
//! read-back tell those apart.
//!
//! What each case pins:
//!
//! - **A replayed outline lands in its slot and nowhere else.** Coverage inside
//!   the slot rectangle, exactly zero outside it. A pass whose viewport uniform
//!   named the frame's extent instead of the page's, or whose attachment named
//!   the wrong layer, fails here.
//! - **A clear zeroes exactly its rectangle.** Its neighbours keep their texels,
//!   which is what makes an eviction observable as absence rather than as a
//!   hole punched through the page.
//! - **A clear ordered against a replay of the same rectangle.** The queue write
//!   flushes before the submit's own command buffers, so the glyph survives and
//!   the clear does not erase it — the ordering the whole design rests on, and
//!   the one that silently reverses if the clear ever moves into an encoder.
//! - **Two layers are independent.** Ink on layer one leaves layer zero alone,
//!   in both directions.
//! - **The scene target is never touched.** A separate render target, painted
//!   before the replay runs, reads back byte-identical afterwards.
//!
//! The whole file runs under a validation error scope, so a bind group built
//! against the wrong derived layout, an attachment whose format disagrees with
//! its pipeline, or an out-of-range draw names itself rather than showing up as
//! a plausible-looking blank page.

#![cfg(not(target_arch = "wasm32"))]

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use frust_engine::gpu::atlas::{AtlasArray, AtlasPageBuffers, AtlasRenderer, push_solid_strips};
use frust_engine::gpu::pipelines::atlas_strip_desc;
use frust_engine::gpu::{ATLAS_FORMAT, EngineShaders};
use frust_engine::{EngineDraw, SceneCompiler};
use frust_gpu::{HeadlessTarget, PipelineCache, ShaderLibrary, TierCaps};
use frust_scene::{Scene, SceneBuilder};
use glifo::atlas::PendingClearRect;
use glifo::{AtlasCommand, AtlasCommandRecorder, AtlasPaint, DrawSink, GlyphAtlas};
use kurbo::{Affine, BezPath, Rect};
use peniko::color::palette::css::{RED, WHITE};
use peniko::{Brush, Color};
use vello_common::paint::Paint;

/// The atlas page extent every case uses — small enough that a whole layer's
/// read-back is cheap, large enough to hold two disjoint slots.
const PAGE: u32 = 256;

/// Bytes one atlas texel occupies at [`ATLAS_FORMAT`].
const TEXEL_BYTES: u32 = 4;

/// wgpu's mandatory `bytes_per_row` alignment for a texture-to-buffer copy.
const COPY_ROW_ALIGNMENT: u32 = 256;

/// The slot the single-glyph cases replay into.
const SLOT: Slot = Slot {
    x: 64,
    y: 96,
    width: 40,
    height: 48,
};

/// The second slot the two-layer case uses, disjoint from [`SLOT`] on both
/// axes so "ink in the wrong slot" and "ink in the wrong layer" cannot be
/// confused for one another.
const OTHER_SLOT: Slot = Slot {
    x: 160,
    y: 24,
    width: 40,
    height: 48,
};

/// A rectangle of one atlas page, in texels.
#[derive(Clone, Copy, Debug)]
struct Slot {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

impl Slot {
    fn contains(self, x: u32, y: u32) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }

    fn centre(self) -> (u32, u32) {
        (self.x + self.width / 2, self.y + self.height / 2)
    }
}

/// Serializes every test in this binary that creates a GPU device, for the
/// reason `encode_contract.rs` documents at length: these tests each build
/// their own `wgpu::Device`, and a driver that serializes device teardown
/// against other work on a process-global mutex deadlocks when two of them tear
/// down at once. Poison is ignored deliberately — one test's failure must not
/// cascade into every sibling.
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
            "frust-engine atlas render adapter: {:?}",
            adapter.get_info()
        );
        let caps = TierCaps::probe(&adapter);
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("frust-engine atlas render device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                ..Default::default()
            })
            .await
            .expect("failed to create the device");
        (device, queue, caps)
    })
}

/// One test's device, atlas, renderer and the pipeline the replay pass draws
/// through.
struct Harness {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    atlas: AtlasArray,
    renderer: AtlasRenderer,
    /// Kept alive so the pipeline's shader module outlives the pass.
    _cache: PipelineCache,
}

impl Harness {
    fn new(layers: u32) -> Self {
        let (device, queue, caps) = gpu();

        let mut library = ShaderLibrary::new();
        let shaders = EngineShaders::register(&mut library, &device);
        let mut cache = PipelineCache::new(Arc::new(library), None);
        // The atlas pass draws through a description the warm-up list already
        // carries, so this is the same pipeline object a real renderer would
        // have compiled before its first frame.
        let pipeline = cache
            .get_or_create(&device, &atlas_strip_desc(&shaders))
            .clone();

        let atlas = AtlasArray::with_layers(&device, PAGE, PAGE, layers);
        let renderer = AtlasRenderer::new(&device, &caps);

        Self {
            device,
            queue,
            pipeline,
            atlas,
            renderer,
            _cache: cache,
        }
    }

    /// Replays every page `glyphs` has recorded, lowering each through
    /// [`lower_page`].
    fn replay(&mut self, glyphs: &mut GlyphAtlas) -> frust_engine::gpu::atlas::AtlasRenderReport {
        self.renderer.render_pending(
            &self.device,
            &self.queue,
            &self.pipeline,
            &self.atlas,
            glyphs,
            lower_page,
        )
    }

    /// One array layer's texels, tightly packed RGBA8.
    fn read_layer(&self, layer: u32) -> Vec<u8> {
        let bytes_per_row = (PAGE * TEXEL_BYTES).next_multiple_of(COPY_ROW_ALIGNMENT);
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("atlas layer readback"),
            size: u64::from(bytes_per_row) * u64::from(PAGE),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("atlas layer readback copy"),
            });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: self.atlas.texture(),
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: 0,
                    y: 0,
                    z: layer,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(PAGE),
                },
            },
            wgpu::Extent3d {
                width: PAGE,
                height: PAGE,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);

        let slice = buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("the readback poll must succeed");
        rx.recv()
            .expect("the readback channel must stay open")
            .expect("the readback buffer must map");

        let mapped = slice.get_mapped_range();
        let row = (PAGE * TEXEL_BYTES) as usize;
        let stride = bytes_per_row as usize;
        let mut pixels = Vec::with_capacity(row * PAGE as usize);
        for y in 0..PAGE as usize {
            pixels.extend_from_slice(&mapped[y * stride..y * stride + row]);
        }
        drop(mapped);
        buffer.unmap();
        pixels
    }
}

/// The RGBA bytes of `(x, y)` in a tightly packed page read-back.
fn texel(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let index = ((y * PAGE + x) * TEXEL_BYTES) as usize;
    pixels[index..index + 4]
        .try_into()
        .expect("a page row holds four bytes per texel")
}

/// Asserts every texel outside `slots` is exactly transparent black.
///
/// The strong half of the containment claim: a pass that drew into the wrong
/// layer, at the wrong scale, or with the frame's viewport instead of the
/// page's would put ink somewhere this walk reaches.
fn assert_blank_outside(pixels: &[u8], slots: &[Slot], what: &str) {
    for y in 0..PAGE {
        for x in 0..PAGE {
            if slots.iter().any(|slot| slot.contains(x, y)) {
                continue;
            }
            assert_eq!(
                texel(pixels, x, y),
                [0, 0, 0, 0],
                "{what}: ({x}, {y}) is outside every slot but is not transparent"
            );
        }
    }
}

/// How many texels of `slot` carry any coverage at all.
fn covered(pixels: &[u8], slot: Slot) -> u32 {
    let mut count = 0;
    for y in slot.y..slot.y + slot.height {
        for x in slot.x..slot.x + slot.width {
            if texel(pixels, x, y)[3] != 0 {
                count += 1;
            }
        }
    }
    count
}

/// A glyph-shaped outline in slot-local coordinates: a closed contour with two
/// curved sides, inset two texels so its anti-aliased edge stays inside the
/// slot it is replayed into.
fn outline(slot: Slot) -> BezPath {
    let w = f64::from(slot.width);
    let h = f64::from(slot.height);
    let mut path = BezPath::new();
    path.move_to((w * 0.5, 2.0));
    path.curve_to((w - 2.0, h * 0.25), (w - 2.0, h * 0.75), (w * 0.5, h - 2.0));
    path.curve_to((2.0, h * 0.75), (2.0, h * 0.25), (w * 0.5, 2.0));
    path.close_path();
    path
}

/// Records one glyph outline into `glyphs`' recorder for `page`, at `slot`'s
/// origin — the same three commands `glifo`'s outline path emits through
/// [`DrawSink`].
fn record_outline(glyphs: &mut GlyphAtlas, page: u32, slot: Slot, color: Color) {
    let recorder = glyphs.recorder_for_page(page, PAGE as u16, PAGE as u16);
    recorder.set_transform(Affine::translate((f64::from(slot.x), f64::from(slot.y))));
    recorder.set_paint(AtlasPaint::Solid(color));
    recorder.fill_path(&outline(slot));
}

/// Lowers one page's recorded commands into strip instances.
///
/// This is the caller-supplied half of the seam: `frust_engine::gpu` owns the
/// pass, the ordering and the submit, and the command-stream-to-strips walk
/// stays with the compiler, which is the layer that has a `SceneCompiler`. A
/// command this tier has no lowering for — a clip path, a blend layer, a
/// gradient paint, all of them COLR shapes — refuses the whole page rather than
/// being skipped, so a colour glyph goes *missing* rather than landing as a
/// partially-painted one.
fn lower_page(recorder: &AtlasCommandRecorder, buffers: &mut AtlasPageBuffers) -> bool {
    let mut scene = Scene::new();
    {
        let mut builder = SceneBuilder::new(&mut scene);
        let mut transform = Affine::IDENTITY;
        let mut brush = Brush::Solid(Color::BLACK);

        for command in &recorder.commands {
            match command {
                AtlasCommand::SetTransform(next) => transform = *next,
                AtlasCommand::SetPaint(AtlasPaint::Solid(color)) => brush = Brush::Solid(*color),
                AtlasCommand::FillPath(path) => {
                    builder.push_transform(transform);
                    builder.fill_path((**path).clone(), brush.clone());
                    builder.pop_transform();
                }
                AtlasCommand::FillRect(rect) => {
                    builder.push_transform(transform);
                    builder.fill_rect(*rect, brush.clone());
                    builder.pop_transform();
                }
                _ => return false,
            }
        }
    }

    let size = (PAGE as u16, PAGE as u16);
    let mut compiler = SceneCompiler::new(size.0, size.1);
    let Ok(frame) = compiler.compile(&scene, Affine::IDENTITY, size) else {
        return false;
    };

    let strips = frame.strip_buf();
    for draw in frame.draws() {
        let EngineDraw {
            paint,
            depth,
            strip_range,
        } = draw;
        let Paint::Solid(color) = paint else {
            // An indexed paint means the lowering produced a record this pass
            // binds no texture for; refusing beats painting it as black.
            return false;
        };
        let Some(run) = strips.get(strip_range.clone()) else {
            continue;
        };
        push_solid_strips(
            run,
            color.as_premul_rgba8().to_u32(),
            *depth,
            &mut buffers.instances,
        );
    }
    buffers.alphas.extend_from_slice(frame.alphas());
    true
}

#[test]
#[ignore = "requires a GPU (Vulkan/Metal); run with `cargo test -p frust-engine --test atlas_render -- --ignored`"]
fn a_replayed_outline_covers_its_slot_and_leaves_the_rest_of_the_page_blank() {
    let _serialized = render_lock();
    let mut harness = Harness::new(1);
    let scope = harness
        .device
        .push_error_scope(wgpu::ErrorFilter::Validation);

    let mut glyphs = GlyphAtlas::new();
    record_outline(&mut glyphs, 0, SLOT, WHITE);
    let report = harness.replay(&mut glyphs);

    assert_eq!(report.pages, 1, "one dirty page, one pass");
    assert_eq!(report.refused, 0, "an outline is a shape this tier lowers");
    assert!(report.instances > 0, "the page drew no strips at all");

    let pixels = harness.read_layer(0);
    let error = drain_error_scope(&harness.device, scope);
    assert!(error.is_none(), "the atlas pass raised {error:?}");

    let (cx, cy) = SLOT.centre();
    assert_eq!(
        texel(&pixels, cx, cy),
        [255, 255, 255, 255],
        "the interior of a white outline must be fully covered"
    );
    assert!(
        covered(&pixels, SLOT) > SLOT.width * SLOT.height / 4,
        "the outline covered {} of {} slot texels — the replay drew almost nothing",
        covered(&pixels, SLOT),
        SLOT.width * SLOT.height
    );
    assert_blank_outside(&pixels, &[SLOT], "a single replayed outline");
}

#[test]
#[ignore = "requires a GPU (Vulkan/Metal); run with `cargo test -p frust-engine --test atlas_render -- --ignored`"]
fn a_clear_rect_zeroes_exactly_its_rectangle() {
    let _serialized = render_lock();
    let harness = Harness::new(1);
    let scope = harness
        .device
        .push_error_scope(wgpu::ErrorFilter::Validation);

    // A block of opaque texels standing in for a page full of resident glyphs,
    // and a clear rect over its middle standing in for one of them being reaped.
    let filled = frust_engine::AtlasRegion {
        layer: 0,
        offset: [SLOT.x, SLOT.y],
        size: [SLOT.width, SLOT.height],
    };
    assert!(
        harness
            .atlas
            .write_region(&harness.queue, filled, &vec![0xFF_u8; filled.byte_len()])
    );

    let cleared = PendingClearRect {
        page_index: 0,
        x: (SLOT.x + 8) as u16,
        y: (SLOT.y + 8) as u16,
        width: 16,
        height: 16,
    };
    assert!(
        harness
            .renderer
            .clear_rect(&harness.queue, &harness.atlas, cleared)
    );

    let pixels = harness.read_layer(0);
    let error = drain_error_scope(&harness.device, scope);
    assert!(error.is_none(), "the clear raised {error:?}");

    for y in 0..PAGE {
        for x in 0..PAGE {
            let inside_clear = x >= u32::from(cleared.x)
                && x < u32::from(cleared.x) + u32::from(cleared.width)
                && y >= u32::from(cleared.y)
                && y < u32::from(cleared.y) + u32::from(cleared.height);
            let expected = if !inside_clear && SLOT.contains(x, y) {
                [0xFF; 4]
            } else {
                [0; 4]
            };
            assert_eq!(
                texel(&pixels, x, y),
                expected,
                "({x}, {y}): a clear must zero its own rectangle and nothing else"
            );
        }
    }
}

#[test]
#[ignore = "requires a GPU (Vulkan/Metal); run with `cargo test -p frust-engine --test atlas_render -- --ignored`"]
fn a_clear_of_a_rectangle_lands_before_the_replay_that_reuses_it() {
    let _serialized = render_lock();
    let mut harness = Harness::new(1);
    let scope = harness
        .device
        .push_error_scope(wgpu::ErrorFilter::Validation);

    // The slot starts full of a previous tenant's opaque red.
    let filled = frust_engine::AtlasRegion {
        layer: 0,
        offset: [SLOT.x, SLOT.y],
        size: [SLOT.width, SLOT.height],
    };
    assert!(
        harness
            .atlas
            .write_region(&harness.queue, filled, &vec![0xFF_u8; filled.byte_len()])
    );

    // The eviction frees it and a new glyph is replayed into it on the same
    // call. The clear is a queue write and the replay is a command buffer, so
    // the flush order decides which of the two the layer ends up holding.
    assert!(harness.renderer.clear_rect(
        &harness.queue,
        &harness.atlas,
        PendingClearRect {
            page_index: 0,
            x: SLOT.x as u16,
            y: SLOT.y as u16,
            width: SLOT.width as u16,
            height: SLOT.height as u16,
        }
    ));
    let mut glyphs = GlyphAtlas::new();
    record_outline(&mut glyphs, 0, SLOT, RED);
    assert_eq!(harness.replay(&mut glyphs).pages, 1);

    let pixels = harness.read_layer(0);
    let error = drain_error_scope(&harness.device, scope);
    assert!(
        error.is_none(),
        "the ordered clear and replay raised {error:?}"
    );

    let (cx, cy) = SLOT.centre();
    assert_eq!(
        texel(&pixels, cx, cy),
        [255, 0, 0, 255],
        "the replayed glyph must survive the clear that preceded it"
    );
    // The corners of the slot are outside the outline, so the clear reached
    // them and the replay did not — which is what "the clear ran first, and ran
    // in full" looks like from the outside.
    assert_eq!(
        texel(&pixels, SLOT.x, SLOT.y),
        [0, 0, 0, 0],
        "the clear must have zeroed the parts of the slot the glyph does not cover"
    );
    assert_blank_outside(&pixels, &[SLOT], "a cleared and replayed slot");
}

#[test]
#[ignore = "requires a GPU (Vulkan/Metal); run with `cargo test -p frust-engine --test atlas_render -- --ignored`"]
fn two_layers_render_independently() {
    let _serialized = render_lock();
    let mut harness = Harness::new(2);
    let scope = harness
        .device
        .push_error_scope(wgpu::ErrorFilter::Validation);

    let mut glyphs = GlyphAtlas::new();
    record_outline(&mut glyphs, 0, SLOT, WHITE);
    record_outline(&mut glyphs, 1, OTHER_SLOT, RED);
    let report = harness.replay(&mut glyphs);

    assert_eq!(report.pages, 2, "one pass per dirty page");
    assert_eq!(report.refused, 0);

    let zero = harness.read_layer(0);
    let one = harness.read_layer(1);
    let error = drain_error_scope(&harness.device, scope);
    assert!(error.is_none(), "the two-layer replay raised {error:?}");

    let (cx, cy) = SLOT.centre();
    assert_eq!(texel(&zero, cx, cy), [255, 255, 255, 255]);
    assert_eq!(
        covered(&zero, OTHER_SLOT),
        0,
        "layer zero must not carry layer one's glyph"
    );
    assert_blank_outside(&zero, &[SLOT], "layer zero");

    let (ox, oy) = OTHER_SLOT.centre();
    assert_eq!(texel(&one, ox, oy), [255, 0, 0, 255]);
    assert_eq!(
        covered(&one, SLOT),
        0,
        "layer one must not carry layer zero's glyph"
    );
    assert_blank_outside(&one, &[OTHER_SLOT], "layer one");
}

#[test]
#[ignore = "requires a GPU (Vulkan/Metal); run with `cargo test -p frust-engine --test atlas_render -- --ignored`"]
fn the_atlas_pass_never_touches_the_scene_target() {
    let _serialized = render_lock();
    let mut harness = Harness::new(1);
    let scope = harness
        .device
        .push_error_scope(wgpu::ErrorFilter::Validation);

    // A target the atlas renderer is never handed, painted a colour nothing
    // else in this file produces.
    let target = HeadlessTarget::new(&harness.device, PAGE, PAGE, ATLAS_FORMAT);
    let mut encoder = harness
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("scene target paint"),
        });
    drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("scene target paint pass"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target.view(),
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color {
                    r: 0.0,
                    g: 1.0,
                    b: 0.0,
                    a: 1.0,
                }),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    }));
    harness.queue.submit([encoder.finish()]);
    let before = target.read_back(&harness.device, &harness.queue);

    let mut glyphs = GlyphAtlas::new();
    record_outline(&mut glyphs, 0, SLOT, WHITE);
    assert_eq!(harness.replay(&mut glyphs).pages, 1);

    let after = target.read_back(&harness.device, &harness.queue);
    let error = drain_error_scope(&harness.device, scope);
    assert!(error.is_none(), "the atlas pass raised {error:?}");

    assert_eq!(
        before, after,
        "the atlas pass wrote the scene's own target — its only attachment is an atlas layer"
    );
    // And the pass really did run: an assertion that nothing changed is worth
    // nothing if nothing happened.
    assert!(covered(&harness.read_layer(0), SLOT) > 0);
}

#[test]
#[ignore = "requires a GPU (Vulkan/Metal); run with `cargo test -p frust-engine --test atlas_render -- --ignored`"]
fn a_page_the_lowering_declines_is_refused_rather_than_drawn() {
    let _serialized = render_lock();
    let mut harness = Harness::new(1);
    let scope = harness
        .device
        .push_error_scope(wgpu::ErrorFilter::Validation);

    // A blend layer is a COLR shape this tier has no lowering for, so the whole
    // page is declined — the glyph goes missing rather than landing wrong.
    let mut glyphs = GlyphAtlas::new();
    {
        let recorder = glyphs.recorder_for_page(0, PAGE as u16, PAGE as u16);
        recorder.set_transform(Affine::translate((f64::from(SLOT.x), f64::from(SLOT.y))));
        recorder.set_paint(AtlasPaint::Solid(WHITE));
        recorder.push_blend_layer(peniko::BlendMode::default());
        recorder.fill_rect(&Rect::new(0.0, 0.0, 8.0, 8.0));
        recorder.pop_layer();
    }

    let report = harness.replay(&mut glyphs);
    assert_eq!(report.pages, 0, "nothing may be drawn from a declined page");
    assert_eq!(report.instances, 0);
    assert_eq!(report.refused, 1, "the refusal must be counted, not silent");

    let pixels = harness.read_layer(0);
    let error = drain_error_scope(&harness.device, scope);
    assert!(error.is_none(), "a declined page raised {error:?}");
    assert_blank_outside(&pixels, &[], "a declined page");
}
