//! GPU tests for [`frust_gpu::pipeline::PipelineCache`]'s warm-up on a real
//! device: waiting for the worker, requesting every listed variant, what the
//! driver's pipeline-cache blob holds once the worker has drained, and the
//! [`ShaderLibrary`]'s modules — created on first use, once, and ahead of the
//! warm-up by `warm_up_shaders`' pool.
//! `#[ignore]`d — run manually on hardware with a GPU:
//!
//! ```text
//! cargo test -p frust-gpu --test warm_up -- --ignored --test-threads=1
//! ```
//!
//! On a multi-adapter host, pin the adapter the same way any other headless
//! run does (`WGPU_BACKEND=vulkan WGPU_ADAPTER_NAME=T400`). The blob test needs
//! a Vulkan adapter (the only backend wgpu implements `PIPELINE_CACHE` on) and
//! reports itself skipped elsewhere.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use frust_gpu::RenderContext;
use frust_gpu::pipeline::{PipelineCache, RenderPipelineDesc};
use frust_gpu::shader::{ShaderId, ShaderLibrary};

/// One device at a time, for the driver-teardown reason every GPU suite in
/// this workspace serializes on.
static GPU_LOCK: Mutex<()> = Mutex::new(());

fn gpu_lock() -> MutexGuard<'static, ()> {
    GPU_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Far longer than any warm-up of a handful of variants takes, so a timeout
/// here is a hang, not a slow driver.
const WAIT: Duration = Duration::from_secs(60);

const WGSL: &str = r#"
@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}
@fragment
fn fs_green() -> @location(0) vec4<f32> {
    return vec4<f32>(0.0, 1.0, 0.0, 1.0);
}
@fragment
fn fs_half() -> @location(0) vec4<f32> {
    return vec4<f32>(0.5, 0.25, 0.0, 0.5);
}
"#;

/// Eight distinct variants of one program pair: two fragment entry points,
/// two target formats, blended or not.
fn variants(library: &mut ShaderLibrary, device: &wgpu::Device) -> Vec<RenderPipelineDesc> {
    let shader = library.insert_wgsl(device, "warm-up test", WGSL);
    let mut descs = Vec::new();
    for fs in ["fs_green", "fs_half"] {
        for format in [
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureFormat::Bgra8Unorm,
        ] {
            for blend in [None, Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING)] {
                descs.push(RenderPipelineDesc {
                    blend,
                    ..RenderPipelineDesc::new(shader, "vs_main", fs, format)
                });
            }
        }
    }
    descs
}

#[test]
#[ignore = "needs a real GPU adapter; run with `cargo test -p frust-gpu --test warm_up -- --ignored \
            --test-threads=1` (pin the adapter with WGPU_BACKEND / WGPU_ADAPTER_NAME)"]
fn wait_warm_up_returns_once_every_listed_variant_exists() {
    let _serialized = gpu_lock();
    pollster::block_on(async {
        let mut context = RenderContext::new();
        let handle = context.device().await.expect("device creation");
        let device = &handle.device;
        let mut library = ShaderLibrary::new();
        let descs = variants(&mut library, device);
        let mut cache = PipelineCache::new(Arc::new(library), None);

        cache.warm_up(device, &descs);
        assert!(cache.wait_warm_up(WAIT), "the warm-up never finished");
        assert!(cache.warm_up_done());
        assert_eq!(cache.queued_variants(), 0);
        // The waiter built nothing itself: the worker compiled every variant.
        assert_eq!(cache.compiled_variants(), descs.len() as u64);
        assert_eq!(cache.warm_up_finished(), descs.len() as u64);

        // Every listed variant is now a hit, not a compile.
        for desc in &descs {
            let _ = cache.get_or_create(device, desc);
        }
        assert_eq!(cache.compiled_variants(), descs.len() as u64);
        cache.shutdown();
    });
}

#[test]
#[ignore = "needs a real GPU adapter; run with `cargo test -p frust-gpu --test warm_up -- --ignored \
            --test-threads=1` (pin the adapter with WGPU_BACKEND / WGPU_ADAPTER_NAME)"]
fn requesting_every_listed_variant_finishes_the_warm_up_and_is_idempotent() {
    let _serialized = gpu_lock();
    pollster::block_on(async {
        let mut context = RenderContext::new();
        let handle = context.device().await.expect("device creation");
        let device = &handle.device;
        let mut library = ShaderLibrary::new();
        let descs = variants(&mut library, device);
        let mut cache = PipelineCache::new(Arc::new(library), None);

        // The calling thread steals what the worker has not reached and
        // waits for what it is building: nothing compiles twice.
        cache.warm_up(device, &descs);
        for _ in 0..3 {
            for desc in &descs {
                let _ = cache.get_or_create(device, desc);
            }
            assert_eq!(cache.compiled_variants(), descs.len() as u64);
        }
        assert!(cache.warm_up_done());
        assert!(cache.wait_warm_up(Duration::ZERO));
        assert_eq!(cache.queued_variants(), 0);
        cache.shutdown();
    });
}

/// Needs a Vulkan adapter: the driver cache blob is only readable where wgpu
/// implements `PIPELINE_CACHE`. On any other adapter (Metal on a Mac, for
/// one) the test logs one skip line and passes without asserting anything.
#[test]
#[ignore = "needs a Vulkan adapter with PIPELINE_CACHE (WGPU_BACKEND=vulkan); run with \
            `cargo test -p frust-gpu --test warm_up -- --ignored --test-threads=1`"]
fn the_driver_cache_blob_holds_the_pipelines_once_the_warm_up_has_drained() {
    let _serialized = gpu_lock();
    pollster::block_on(async {
        let mut context = RenderContext::new();
        context.device().await.expect("device creation");
        if !context.pipeline_cache_supported() {
            println!(
                "skipped: the {} adapter has no PIPELINE_CACHE (this test needs Vulkan)",
                context.device_handle().adapter.get_info().backend
            );
            return;
        }
        // SAFETY: no seed — a cold cache carries no foreign data, which is
        // the only thing `create_pipeline_cache`'s contract constrains.
        let driver_cache =
            unsafe { context.create_pipeline_cache(None) }.expect("Vulkan offers a cache");
        let empty = driver_cache.get_data().map_or(0, |data| data.len());

        let device = &context.device_handle().device;
        let mut library = ShaderLibrary::new();
        let descs = variants(&mut library, device);
        let library = Arc::new(library);
        let mut cache = PipelineCache::new(Arc::clone(&library), Some(driver_cache.clone()));
        cache.warm_up(device, &descs);
        assert!(cache.wait_warm_up(WAIT), "the warm-up never finished");
        let warmed = driver_cache
            .get_data()
            .expect("a warmed Vulkan cache has data");
        println!(
            "driver cache blob: {empty} B empty, {} B after warming {} variants",
            warmed.len(),
            descs.len()
        );
        assert!(
            warmed.len() > empty,
            "the blob read after the warm-up must hold the compiled pipelines"
        );
        cache.shutdown();

        // The framed blob seeds a second cache, which warms up against it.
        let framed = frust_gpu::pipeline_cache::frame(&context.adapter_cache_key(), &warmed);
        // SAFETY: `framed` is this adapter's own `get_data()` output, framed
        // under this adapter's fingerprint a moment ago — exactly the
        // provenance `create_pipeline_cache` requires.
        let seeded =
            unsafe { context.create_pipeline_cache(Some(&framed)) }.expect("Vulkan offers a cache");
        let device = &context.device_handle().device;
        let mut cache = PipelineCache::new(library, Some(seeded));
        cache.warm_up(device, &descs);
        assert!(
            cache.wait_warm_up(WAIT),
            "the seeded warm-up never finished"
        );
        assert_eq!(cache.compiled_variants(), descs.len() as u64);
        cache.shutdown();
    });
}

/// `count` programs over the test source, registered under distinct names
/// and created by nobody yet.
fn lazy_programs(library: &mut ShaderLibrary, count: usize) -> Vec<ShaderId> {
    (0..count)
        .map(|index| library.register_wgsl(format!("lazy warm-up test {index}"), WGSL))
        .collect()
}

/// One unblended green variant over each of `shaders`.
fn green_variants(shaders: &[ShaderId]) -> Vec<RenderPipelineDesc> {
    shaders
        .iter()
        .map(|shader| {
            RenderPipelineDesc::new(
                *shader,
                "vs_main",
                "fs_green",
                wgpu::TextureFormat::Rgba8Unorm,
            )
        })
        .collect()
}

#[test]
#[ignore = "needs a real GPU adapter; run with `cargo test -p frust-gpu --test warm_up -- --ignored \
            --test-threads=1` (pin the adapter with WGPU_BACKEND / WGPU_ADAPTER_NAME)"]
fn a_registered_module_is_created_on_first_use_exactly_once() {
    let _serialized = gpu_lock();
    pollster::block_on(async {
        let mut context = RenderContext::new();
        let handle = context.device().await.expect("device creation");
        let device = &handle.device;
        let mut library = ShaderLibrary::new();
        let programs = lazy_programs(&mut library, 2);
        let (raced, built) = (programs[0], programs[1]);
        assert_eq!(library.compiled_len(), 0, "registration created a module");
        let library = Arc::new(library);

        // Eight threads ask for the same module at once: one creates it, the
        // rest wait for it, and every one of them gets that one module.
        let addresses: Vec<usize> = std::thread::scope(|scope| {
            let askers: Vec<_> = (0..8)
                .map(|_| {
                    let library = &library;
                    scope.spawn(move || {
                        let module = library.module(device, raced).expect("a minted id");
                        std::ptr::from_ref(module) as usize
                    })
                })
                .collect();
            askers
                .into_iter()
                .map(|asker| asker.join().expect("an asker panicked"))
                .collect()
        });
        assert!(addresses.windows(2).all(|pair| pair[0] == pair[1]));
        assert!(library.is_compiled(raced));
        assert!(
            !library.is_compiled(built),
            "an unrequested module was created"
        );
        assert_eq!(library.compiled_len(), 1);

        // A pipeline build is a first use: it creates its program's module.
        let mut cache = PipelineCache::new(Arc::clone(&library), None);
        let _pipeline = cache.get_or_create(device, &green_variants(&[built])[0]);
        assert!(library.is_compiled(built));
        assert_eq!(cache.compiled_variants(), 1);
        cache.shutdown();
    });
}

#[test]
#[ignore = "needs a real GPU adapter; run with `cargo test -p frust-gpu --test warm_up -- --ignored \
            --test-threads=1` (pin the adapter with WGPU_BACKEND / WGPU_ADAPTER_NAME)"]
fn the_shader_pool_creates_the_listed_modules_beside_the_warm_up() {
    let _serialized = gpu_lock();
    pollster::block_on(async {
        let mut context = RenderContext::new();
        let handle = context.device().await.expect("device creation");
        let device = &handle.device;
        let mut library = ShaderLibrary::new();
        let programs = lazy_programs(&mut library, 4);
        let late = library.register_wgsl("lazy warm-up test, after shutdown", WGSL);
        let library = Arc::new(library);
        let mut cache = PipelineCache::new(Arc::clone(&library), None);

        // The pool takes the first two (one listed twice); the worker's builds
        // wait for those and create the other two themselves.
        cache.warm_up_shaders(device, &[programs[0], programs[1], programs[0]]);
        let descs = green_variants(&programs);
        cache.warm_up(device, &descs);
        assert!(cache.wait_warm_up(WAIT), "the warm-up never finished");
        assert_eq!(cache.compiled_variants(), descs.len() as u64);
        for program in &programs {
            assert!(library.is_compiled(*program));
        }

        // Shutdown joins the pool; afterwards it starts nothing, so the
        // module stays uncreated until its first use.
        cache.shutdown();
        cache.warm_up_shaders(device, &[late]);
        assert!(
            !library.is_compiled(late),
            "a shut-down cache started a pool"
        );
        assert_eq!(library.compiled_len(), programs.len());
        assert!(library.module(device, late).is_some());
        assert_eq!(library.compiled_len(), programs.len() + 1);
    });
}
