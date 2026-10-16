//! Metal device smoke for [`frust_gpu::import::apple`]: an IOSurface-backed
//! `CVPixelBuffer` the test creates and fills on the CPU is imported without a
//! copy, drawn, and read back pixel for pixel; the import's retain on the
//! buffer is shown to outlive the GPU work that sampled it; refused layouts
//! answer the right error; and a ring of buffers runs 300 frames at 30 fps,
//! reporting the import latency and whether the texture cache handed back the
//! `MTLTexture` it already had for each buffer after warm-up (a control case
//! with a fresh cache per frame shows that comparison can see a miss).
//!
//! Compiled only for Apple targets (an empty file everywhere else) and
//! `#[ignore]`d — it needs a real Metal adapter:
//!
//! ```text
//! cargo test -p frust-gpu --test import_apple -- --ignored --test-threads=1 --nocapture
//! ```
#![cfg(target_vendor = "apple")]

use std::ptr::NonNull;
use std::time::{Duration, Instant};

use frust_gpu::import::apple::{MetalImportError, MetalImporter};
use frust_gpu::{DeviceHandle, HeadlessTarget, RenderContext};
use objc2::Message;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_core_foundation::{CFDictionary, CFRetained, CFString, CFType};
use objc2_core_video::{
    CVPixelBuffer, CVPixelBufferCreate, CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow,
    CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress,
    kCVPixelBufferIOSurfacePropertiesKey, kCVPixelFormatType_32BGRA,
    kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange, kCVReturnSuccess,
};
use objc2_metal::MTLTexture;
use wgpu::hal::api::Metal;

/// The test buffer's edge, in texels: one 256-byte readback row.
const SIZE: u32 = 64;

/// Frames the stability run imports, draws and checks.
const FRAMES: u32 = 300;

/// The stability run's frame interval: 30 fps, a video's usual cadence.
const FRAME_INTERVAL: Duration = Duration::from_nanos(1_000_000_000 / 30);

/// Buffers in the stability run's ring — a decoder's usual pool depth.
const RING: usize = 3;

/// The opaque RGBA texel the pattern `seed` puts at `(x, y)` — opaque, so its
/// premultiplied and straight forms agree.
fn texel(x: u32, y: u32, seed: u32) -> [u8; 4] {
    [
        (x * 4 + seed) as u8,
        (y * 4) as u8,
        ((x + y) * 2 + seed * 3) as u8,
        255,
    ]
}

/// A `CVPixelBuffer` this test owns one reference to.
struct PixelBuffer(CFRetained<CVPixelBuffer>);

impl PixelBuffer {
    /// A `SIZE x SIZE` buffer in `format`, IOSurface-backed when `iosurface`.
    fn create(format: u32, iosurface: bool) -> Self {
        let attributes = iosurface.then(|| {
            let empty = CFDictionary::<CFString, CFType>::empty();
            // SAFETY: an immutable CoreVideo constant, initialised at load.
            let key = unsafe { kCVPixelBufferIOSurfacePropertiesKey };
            CFDictionary::<CFString, CFType>::from_slices(&[key], &[empty.as_ref()])
        });
        let mut buffer: *mut CVPixelBuffer = std::ptr::null_mut();
        // SAFETY: a CFString-keyed attribute dictionary (or none) and a valid
        // out-pointer.
        let status = unsafe {
            CVPixelBufferCreate(
                None,
                SIZE as usize,
                SIZE as usize,
                format,
                attributes.as_deref().map(CFDictionary::as_opaque),
                NonNull::from(&mut buffer),
            )
        };
        assert_eq!(status, kCVReturnSuccess, "CVPixelBufferCreate failed");
        let buffer = NonNull::new(buffer).expect("CVPixelBufferCreate returned null");
        // SAFETY: a `Create` function's result, owned (+1) by this test.
        Self(unsafe { CFRetained::from_raw(buffer) })
    }

    fn bgra() -> Self {
        Self::create(kCVPixelFormatType_32BGRA, true)
    }

    fn ptr(&self) -> *mut std::ffi::c_void {
        CFRetained::as_ptr(&self.0).as_ptr().cast()
    }

    fn retain_count(&self) -> usize {
        self.0.retain_count()
    }

    /// Writes pattern `seed` (as BGRA bytes) through a CPU lock. The GPU must
    /// be done with every texture over the buffer (each round trip below
    /// waits on its readback first).
    fn fill(&self, seed: u32) {
        // SAFETY: a live buffer, locked for writing.
        let status = unsafe { CVPixelBufferLockBaseAddress(&self.0, CVPixelBufferLockFlags(0)) };
        assert_eq!(
            status, kCVReturnSuccess,
            "CVPixelBufferLockBaseAddress failed"
        );
        let base = CVPixelBufferGetBaseAddress(&self.0).cast::<u8>();
        let stride = CVPixelBufferGetBytesPerRow(&self.0);
        assert!(!base.is_null());
        for y in 0..SIZE {
            for x in 0..SIZE {
                let [r, g, b, a] = texel(x, y, seed);
                let offset = y as usize * stride + x as usize * 4;
                // SAFETY: inside the locked `stride * SIZE`-byte mapping.
                unsafe {
                    std::ptr::copy_nonoverlapping([b, g, r, a].as_ptr(), base.add(offset), 4);
                }
            }
        }
        // SAFETY: the lock taken above, with the same flags.
        let status = unsafe { CVPixelBufferUnlockBaseAddress(&self.0, CVPixelBufferLockFlags(0)) };
        assert_eq!(
            status, kCVReturnSuccess,
            "CVPixelBufferUnlockBaseAddress failed"
        );
    }
}

/// Copies an imported texture into an offscreen target, texel for texel, and
/// reads the target back.
struct Drawer {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    target: HeadlessTarget,
}

const SHADER: &str = r"
@group(0) @binding(0) var source: texture_2d<f32>;

@vertex
fn vs(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    return textureLoad(source, vec2<i32>(position.xy), 0);
}
";

impl Drawer {
    fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("import smoke"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("import smoke"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("import smoke"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("import smoke"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::TextureFormat::Rgba8Unorm.into())],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let target = HeadlessTarget::new(device, SIZE, SIZE, wgpu::TextureFormat::Rgba8Unorm);
        Self {
            pipeline,
            layout,
            target,
        }
    }

    /// Records and submits the copy of `texture` into the target, without
    /// waiting for it.
    fn submit(&self, handle: &DeviceHandle, texture: &wgpu::Texture) {
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = handle.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("import smoke"),
            layout: &self.layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            }],
        });
        let mut encoder = handle
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("import smoke"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: self.target.view(),
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        handle.queue.submit([encoder.finish()]);
    }

    fn draw(&self, handle: &DeviceHandle, texture: &wgpu::Texture) -> Vec<u8> {
        self.submit(handle, texture);
        self.target.read_back(&handle.device, &handle.queue)
    }
}

fn metal_device(context: &mut RenderContext) -> DeviceHandle {
    let handle = pollster::block_on(context.device())
        .expect("device creation")
        .clone();
    assert_eq!(
        handle.adapter.get_info().backend,
        wgpu::Backend::Metal,
        "the import smoke needs a Metal adapter"
    );
    handle
}

fn import(importer: &MetalImporter, buffer: &PixelBuffer) -> (wgpu::Texture, Duration) {
    let start = Instant::now();
    // SAFETY: `buffer` is live for the call, complete (filled and unlocked)
    // and not written again until the texture is dropped.
    let (texture, size) =
        unsafe { importer.import(buffer.ptr()) }.unwrap_or_else(|e| panic!("import failed: {e}"));
    let latency = start.elapsed();
    assert_eq!(size, (SIZE, SIZE));
    (texture, latency)
}

fn assert_pattern(pixels: &[u8], seed: u32) {
    for y in 0..SIZE {
        for x in 0..SIZE {
            let offset = ((y * SIZE + x) * 4) as usize;
            assert_eq!(
                &pixels[offset..offset + 4],
                &texel(x, y, seed),
                "texel ({x}, {y}) of pattern {seed}"
            );
        }
    }
}

/// The `MTLTexture` an imported texture wraps, retained by the caller. Held
/// across frames, it pins the object's address, so the same address on a
/// later import means CoreVideo's cache handed back the texture it already
/// had rather than a new one allocated where a freed one used to be.
fn metal_texture(texture: &wgpu::Texture) -> Retained<ProtocolObject<dyn MTLTexture>> {
    // SAFETY: only takes a reference to the hal texture's raw handle.
    let hal = unsafe { texture.as_hal::<Metal>() }.expect("a Metal texture");
    hal.raw_handle().retain()
}

/// Whether `held` is the very `MTLTexture` object `texture` wraps.
fn same_metal_texture(
    held: &Retained<ProtocolObject<dyn MTLTexture>>,
    texture: &wgpu::Texture,
) -> bool {
    Retained::as_ptr(held) == Retained::as_ptr(&metal_texture(texture))
}

#[test]
#[ignore = "needs a Metal adapter (see the module docs)"]
fn an_imported_pixel_buffer_samples_as_its_cpu_pattern() {
    let mut context = RenderContext::new();
    let handle = metal_device(&mut context);
    let importer = MetalImporter::new(&handle.device).expect("texture cache");
    let drawer = Drawer::new(&handle.device);
    let buffer = PixelBuffer::bgra();
    buffer.fill(7);

    let (texture, latency) = import(&importer, &buffer);
    assert_eq!(texture.width(), SIZE);
    assert_eq!(texture.height(), SIZE);
    assert_eq!(texture.format(), wgpu::TextureFormat::Bgra8Unorm);
    assert_eq!(texture.usage(), wgpu::TextureUsages::TEXTURE_BINDING);
    let pixels = drawer.draw(&handle, &texture);
    println!("frust-gpu Metal import smoke: first import {latency:?}");
    assert_pattern(&pixels, 7);
    drop(texture);
    assert_eq!(handle.first_uncaptured_error(), None);
}

#[test]
#[ignore = "needs a Metal adapter (see the module docs)"]
fn the_import_holds_the_pixel_buffer_until_the_gpu_finished_with_it() {
    let mut context = RenderContext::new();
    let handle = metal_device(&mut context);
    let importer = MetalImporter::new(&handle.device).expect("texture cache");
    let drawer = Drawer::new(&handle.device);
    let buffer = PixelBuffer::bgra();
    buffer.fill(3);
    let baseline = buffer.retain_count();

    let (texture, _) = import(&importer, &buffer);
    let imported = buffer.retain_count();
    assert!(
        imported > baseline,
        "the import retains the buffer ({baseline} -> {imported})"
    );

    // The control: a texture no submission used gives the buffer back the
    // moment it is dropped, so the count below can see a release happen.
    drop(texture);
    assert!(
        buffer.retain_count() < imported,
        "an unused texture releases the buffer on drop"
    );

    let (texture, _) = import(&importer, &buffer);
    assert_eq!(buffer.retain_count(), imported);

    // Submitted but not waited on: dropping the texture now must not let go
    // of the buffer — `wgpu` keeps it until the submission finished and a
    // poll saw that.
    drawer.submit(&handle, &texture);
    drop(texture);
    assert_eq!(
        buffer.retain_count(),
        imported,
        "the buffer is still held while the submission that samples it is pending"
    );

    handle
        .device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("device poll");
    let released = buffer.retain_count();
    assert!(
        released < imported,
        "the texture's drop callback gave the buffer back once the GPU finished \
         ({imported} -> {released})"
    );
    drop(importer);
    assert_eq!(
        buffer.retain_count(),
        baseline,
        "nothing keeps the buffer once the importer is gone"
    );
    assert_eq!(handle.first_uncaptured_error(), None);
}

#[test]
#[ignore = "needs a Metal adapter (see the module docs)"]
fn refused_buffers_answer_the_matching_error() {
    let mut context = RenderContext::new();
    let handle = metal_device(&mut context);
    let importer = MetalImporter::new(&handle.device).expect("texture cache");

    // SAFETY: a null pointer is allowed by the contract.
    let null = unsafe { importer.import(std::ptr::null_mut()) };
    assert!(matches!(null, Err(MetalImportError::Failed(_))), "{null:?}");

    let nv12 = PixelBuffer::create(kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange, true);
    // SAFETY: a live buffer this test holds.
    let refused = unsafe { importer.import(nv12.ptr()) };
    match refused {
        Err(MetalImportError::Unsupported(reason)) => assert!(reason.contains("420v"), "{reason}"),
        other => panic!("an NV12 buffer is unsupported, got {other:?}"),
    }

    let plain = PixelBuffer::create(kCVPixelFormatType_32BGRA, false);
    // SAFETY: a live buffer this test holds.
    let refused = unsafe { importer.import(plain.ptr()) };
    assert!(
        matches!(refused, Err(MetalImportError::Failed(_))),
        "a buffer with no IOSurface cannot alias a texture: {refused:?}"
    );

    // A CoreFoundation object of another type is refused by its type id.
    let other = CFString::from_str("not a pixel buffer");
    // SAFETY: a live CoreFoundation object this test holds.
    let refused = unsafe {
        importer.import(
            CFRetained::as_ptr(&other)
                .as_ptr()
                .cast::<std::ffi::c_void>(),
        )
    };
    assert!(
        matches!(refused, Err(MetalImportError::Failed(_))),
        "{refused:?}"
    );
    assert_eq!(handle.first_uncaptured_error(), None);
}

#[test]
#[ignore = "needs a Metal adapter (see the module docs)"]
fn pixel_buffer_import_runs_three_hundred_frames_at_thirty_fps() {
    let mut context = RenderContext::new();
    let handle = metal_device(&mut context);
    let importer = MetalImporter::new(&handle.device).expect("texture cache");
    let drawer = Drawer::new(&handle.device);
    let ring: Vec<PixelBuffer> = (0..RING).map(|_| PixelBuffer::bgra()).collect();

    let mut latencies = Vec::with_capacity(FRAMES as usize);
    let mut held: Vec<Option<Retained<ProtocolObject<dyn MTLTexture>>>> = vec![None; RING];
    let mut misses_after_warm_up = 0;
    let start = Instant::now();
    for frame in 0..FRAMES {
        let deadline = start + FRAME_INTERVAL * frame;
        if let Some(wait) = deadline.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        }
        let slot = frame as usize % RING;
        let buffer = &ring[slot];
        let seed = frame % 61;
        buffer.fill(seed);
        let (texture, latency) = import(&importer, buffer);
        latencies.push(latency);
        let warmed_up = frame as usize >= RING;
        let hit = held[slot]
            .as_ref()
            .is_some_and(|held| same_metal_texture(held, &texture));
        if warmed_up && !hit {
            misses_after_warm_up += 1;
        }
        held[slot] = Some(metal_texture(&texture));
        let pixels = drawer.draw(&handle, &texture);
        assert_pattern(&pixels, seed);
        // The readback waited for the frame, so the texture is idle and its
        // drop gives the cache binding and the buffer back at once.
        drop(texture);
    }
    let elapsed = start.elapsed();
    handle
        .device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("final device poll");
    assert_eq!(handle.first_uncaptured_error(), None);

    latencies.sort();
    let total: Duration = latencies.iter().sum();
    println!(
        "frust-gpu Metal import smoke: {FRAMES} imports over a {RING}-buffer ring in {elapsed:?}, \
         mean {:?}, p50 {:?}, p95 {:?}, max {:?}; texture-cache misses after warm-up: \
         {misses_after_warm_up}",
        total / FRAMES,
        latencies[latencies.len() / 2],
        latencies[latencies.len() * 95 / 100],
        latencies[latencies.len() - 1],
    );
    assert_eq!(
        misses_after_warm_up, 0,
        "every import after warm-up is a texture-cache hit"
    );
}

/// The control for the cache-hit count above: an importer built afresh for
/// every frame has an empty cache each time, so every import must read as a
/// miss — proving the held-texture comparison can see one.
#[test]
#[ignore = "needs a Metal adapter (see the module docs)"]
fn a_fresh_texture_cache_per_frame_reads_as_a_miss_every_time() {
    let mut context = RenderContext::new();
    let handle = metal_device(&mut context);
    let drawer = Drawer::new(&handle.device);
    let buffer = PixelBuffer::bgra();
    let mut held: Option<Retained<ProtocolObject<dyn MTLTexture>>> = None;
    let mut hits = 0;
    for frame in 0..30 {
        let importer = MetalImporter::new(&handle.device).expect("texture cache");
        buffer.fill(frame);
        let (texture, _) = import(&importer, &buffer);
        if held
            .as_ref()
            .is_some_and(|held| same_metal_texture(held, &texture))
        {
            hits += 1;
        }
        held = Some(metal_texture(&texture));
        assert_pattern(&drawer.draw(&handle, &texture), frame);
        drop(texture);
    }
    assert_eq!(hits, 0, "a fresh cache never hands back an earlier texture");
    assert_eq!(handle.first_uncaptured_error(), None);
}
