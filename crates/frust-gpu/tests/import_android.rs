//! Android device smoke for [`frust_gpu::import::android`]: an
//! `AHardwareBuffer` the test allocates and fills on the CPU is imported
//! without a copy, drawn, and read back pixel for pixel; then the same round
//! trip runs for 300 frames, reporting the import latency.
//!
//! Compiled only for Android (an empty file everywhere else) and `#[ignore]`d.
//! It needs the hardware-buffer device, so run it with the knob on:
//!
//! ```text
//! cargo ndk -t arm64-v8a test --no-run -p frust-gpu
//! adb push <the import_android-* binary cargo printed> /data/local/tmp/import_android
//! adb shell 'cd /data/local/tmp && FRUST_ANDROID_HWBUFFER_IMPORT=1 \
//!     ./import_android --ignored --test-threads=1 --nocapture'
//! ```
#![cfg(target_os = "android")]

use std::ffi::{CStr, c_char, c_int, c_void};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use frust_gpu::import::android::import_hardware_buffer;
use frust_gpu::{DeviceHandle, HeadlessTarget, RenderContext};

/// The test buffer's edge, in texels: one 256-byte readback row.
const SIZE: u32 = 64;

/// Frames the stability run imports, draws and checks.
const FRAMES: u32 = 300;

// `unsigned long` flags: 32-bit on the 32-bit ABIs, so the casts widen there.
#[allow(clippy::unnecessary_cast)]
const USAGE_SAMPLED: u64 =
    ndk_sys::AHardwareBuffer_UsageFlags::AHARDWAREBUFFER_USAGE_GPU_SAMPLED_IMAGE.0 as u64;
#[allow(clippy::unnecessary_cast)]
const USAGE_CPU_WRITE: u64 =
    ndk_sys::AHardwareBuffer_UsageFlags::AHARDWAREBUFFER_USAGE_CPU_WRITE_OFTEN.0 as u64;

/// The opaque texel the pattern `seed` puts at `(x, y)` — opaque, so its
/// premultiplied and straight forms agree.
fn texel(x: u32, y: u32, seed: u32) -> [u8; 4] {
    [
        (x * 4 + seed) as u8,
        (y * 4) as u8,
        ((x + y) * 2 + seed * 3) as u8,
        255,
    ]
}

type Allocate = unsafe extern "C" fn(
    *const ndk_sys::AHardwareBuffer_Desc,
    *mut *mut ndk_sys::AHardwareBuffer,
) -> c_int;
type Release = unsafe extern "C" fn(*mut ndk_sys::AHardwareBuffer);
type Describe =
    unsafe extern "C" fn(*const ndk_sys::AHardwareBuffer, *mut ndk_sys::AHardwareBuffer_Desc);
type Lock = unsafe extern "C" fn(
    *mut ndk_sys::AHardwareBuffer,
    u64,
    i32,
    *const ndk_sys::ARect,
    *mut *mut c_void,
) -> c_int;
type Unlock = unsafe extern "C" fn(*mut ndk_sys::AHardwareBuffer, *mut i32) -> c_int;

/// The buffer entry points this test calls, resolved from `libandroid.so` at
/// run time — like the import itself, so the binary links against the default
/// API-21 stubs, which lack them.
struct Api {
    allocate: Allocate,
    release: Release,
    describe: Describe,
    lock: Lock,
    unlock: Unlock,
}

unsafe extern "C" {
    fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

fn api() -> &'static Api {
    static API: OnceLock<Api> = OnceLock::new();
    API.get_or_init(|| {
        // SAFETY: a public NDK library, never closed; each symbol is
        // transmuted to its NDK-declared signature.
        unsafe {
            let library = dlopen(c"libandroid.so".as_ptr(), 2);
            assert!(!library.is_null(), "dlopen(libandroid.so) failed");
            let symbol = |name: &CStr| {
                let address = dlsym(library, name.as_ptr());
                assert!(!address.is_null(), "{name:?} is missing (API 26+)");
                address
            };
            Api {
                allocate: std::mem::transmute::<*mut c_void, Allocate>(symbol(
                    c"AHardwareBuffer_allocate",
                )),
                release: std::mem::transmute::<*mut c_void, Release>(symbol(
                    c"AHardwareBuffer_release",
                )),
                describe: std::mem::transmute::<*mut c_void, Describe>(symbol(
                    c"AHardwareBuffer_describe",
                )),
                lock: std::mem::transmute::<*mut c_void, Lock>(symbol(c"AHardwareBuffer_lock")),
                unlock: std::mem::transmute::<*mut c_void, Unlock>(symbol(
                    c"AHardwareBuffer_unlock",
                )),
            }
        }
    })
}

/// An `AHardwareBuffer` this test owns one reference to.
struct Buffer(*mut ndk_sys::AHardwareBuffer);

impl Buffer {
    fn allocate() -> Self {
        let description = ndk_sys::AHardwareBuffer_Desc {
            width: SIZE,
            height: SIZE,
            layers: 1,
            format: ndk_sys::AHardwareBuffer_Format::AHARDWAREBUFFER_FORMAT_R8G8B8A8_UNORM.0,
            usage: USAGE_SAMPLED | USAGE_CPU_WRITE,
            stride: 0,
            rfu0: 0,
            rfu1: 0,
        };
        let mut buffer = std::ptr::null_mut();
        // SAFETY: a valid description and out-pointer.
        let status = unsafe { (api().allocate)(&description, &mut buffer) };
        assert_eq!(status, 0, "AHardwareBuffer_allocate failed");
        assert!(!buffer.is_null());
        Self(buffer)
    }

    /// Writes pattern `seed` through a CPU lock. The GPU must be done with
    /// every texture over the buffer (each round trip below waits on its
    /// readback first).
    fn fill(&self, seed: u32) {
        // SAFETY: a live buffer; `describe` only writes the descriptor.
        let stride = unsafe {
            let mut description = std::mem::zeroed::<ndk_sys::AHardwareBuffer_Desc>();
            (api().describe)(self.0, &mut description);
            description.stride
        };
        let mut address = std::ptr::null_mut();
        // SAFETY: a live buffer allocated with CPU-write usage, no fence, the
        // whole buffer.
        let status =
            unsafe { (api().lock)(self.0, USAGE_CPU_WRITE, -1, std::ptr::null(), &mut address) };
        assert_eq!(status, 0, "AHardwareBuffer_lock failed");
        let base = address.cast::<u8>();
        for y in 0..SIZE {
            for x in 0..SIZE {
                let offset = ((y * stride + x) * 4) as usize;
                // SAFETY: inside the locked `stride * SIZE * 4`-byte mapping.
                unsafe {
                    std::ptr::copy_nonoverlapping(texel(x, y, seed).as_ptr(), base.add(offset), 4);
                }
            }
        }
        // SAFETY: the lock taken above; a null fence pointer waits for the
        // unlock to complete.
        let status = unsafe { (api().unlock)(self.0, std::ptr::null_mut()) };
        assert_eq!(status, 0, "AHardwareBuffer_unlock failed");
    }
}

impl Drop for Buffer {
    fn drop(&mut self) {
        // SAFETY: the reference `allocate` returned, released once.
        unsafe { (api().release)(self.0) };
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

    fn draw(&self, handle: &DeviceHandle, texture: &wgpu::Texture) -> Vec<u8> {
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
        self.target.read_back(&handle.device, &handle.queue)
    }
}

/// The hardware-buffer device, or a panic naming the knob that opens it.
fn hardware_buffer_device(context: &mut RenderContext) -> DeviceHandle {
    let handle = pollster::block_on(context.device())
        .expect("device creation")
        .clone();
    assert!(
        handle.caps.hwbuffer_import,
        "the device was opened without the AHardwareBuffer import extensions: run with \
         FRUST_ANDROID_HWBUFFER_IMPORT=1 (or `setprop debug.frust.hwbuffer_import 1`), and see \
         the frust-gpu warning in logcat for why a knob-on open was refused"
    );
    handle
}

fn import(handle: &DeviceHandle, buffer: &Buffer) -> (wgpu::Texture, Duration) {
    let start = Instant::now();
    // SAFETY: `buffer` is live for the call, this test is the device's only
    // submitter, and the buffer is complete (filled and unlocked) and not
    // written again until the texture is dropped.
    let texture = unsafe { import_hardware_buffer(&handle.device, buffer.0) }
        .unwrap_or_else(|e| panic!("import failed: {e}"));
    (texture, start.elapsed())
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

#[test]
#[ignore = "needs an Android Vulkan device with FRUST_ANDROID_HWBUFFER_IMPORT=1 (see the module docs)"]
fn an_imported_hardware_buffer_samples_as_its_cpu_pattern() {
    let mut context = RenderContext::new();
    let handle = hardware_buffer_device(&mut context);
    let drawer = Drawer::new(&handle.device);
    let buffer = Buffer::allocate();
    buffer.fill(7);

    let (texture, latency) = import(&handle, &buffer);
    assert_eq!(texture.width(), SIZE);
    assert_eq!(texture.height(), SIZE);
    assert_eq!(texture.format(), wgpu::TextureFormat::Rgba8Unorm);
    let pixels = drawer.draw(&handle, &texture);
    println!("frust-gpu import smoke: first import {latency:?}");
    assert_pattern(&pixels, 7);
    drop(texture);
    assert_eq!(handle.first_uncaptured_error(), None);
}

#[test]
#[ignore = "needs an Android Vulkan device with FRUST_ANDROID_HWBUFFER_IMPORT=1 (see the module docs)"]
fn hardware_buffer_import_survives_three_hundred_frames() {
    let mut context = RenderContext::new();
    let handle = hardware_buffer_device(&mut context);
    let drawer = Drawer::new(&handle.device);
    let buffer = Buffer::allocate();

    let mut latencies = Vec::with_capacity(FRAMES as usize);
    for frame in 0..FRAMES {
        let seed = frame % 61;
        buffer.fill(seed);
        let (texture, latency) = import(&handle, &buffer);
        latencies.push(latency);
        let pixels = drawer.draw(&handle, &texture);
        assert_pattern(&pixels, seed);
        // The readback waited for the frame, so the texture is idle and its
        // drop releases the image, memory and buffer reference at once.
        drop(texture);
    }
    handle
        .device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("final device poll");
    assert_eq!(handle.first_uncaptured_error(), None);

    latencies.sort();
    let total: Duration = latencies.iter().sum();
    println!(
        "frust-gpu import smoke: {FRAMES} imports, mean {:?}, p50 {:?}, p95 {:?}, max {:?}",
        total / FRAMES,
        latencies[latencies.len() / 2],
        latencies[latencies.len() * 95 / 100],
        latencies[latencies.len() - 1],
    );
}
