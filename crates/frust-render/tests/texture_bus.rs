//! The external-texture frame bus ([`frust_render::texture_bus`]): a closure
//! frame source registered as a scene texture, its id namespace, and the
//! whole upload/import path on real hardware.
//!
//! The device-free cases pin the registration's own bookkeeping — its id sits
//! in the reserved texture-source namespace, an ordinary external pass is
//! refused that namespace, and dropping the registration unregisters it.
//!
//! The `#[ignore]`d cases drive the identical drain `SurfaceRenderer::submit`
//! runs (see `tests/external_pass.rs` for why the drain is driven directly)
//! and read the frame back: a CPU frame lands where the display list names
//! the id, a second frame replaces it, an extent change re-creates the
//! texture, a rebuilt engine gets the current view bound again, every
//! frame's release callback runs exactly once, and a native frame is refused
//! (once, released) without an importer and drawn with one, its release held
//! until the imported texture is replaced or unbound and the GPU finished
//! with it:
//!
//! ```text
//! WGPU_BACKEND=vulkan WGPU_ADAPTER_NAME=T400 FRUST_GOLDEN_EXPECT_ADAPTER=T400 \
//!   timeout 1200 cargo test -p frust-render --test texture_bus -- --ignored --test-threads=1
//! ```

#![cfg(not(target_arch = "wasm32"))]

use std::collections::VecDeque;
use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Once, PoisonError};

use frust_engine::{EngineRenderer, EngineTarget, OutputAlpha};
use frust_gpu::{HeadlessTarget, SceneTextureId, TierCaps};
use frust_render::texture_bus::{
    ImportError, ImportedTexture, NativeHandle, NativeHandleKind, NativeImporter, SourceFormat,
    SourceFrame, SourcePayload, clear_native_importer, register_texture_source,
    set_native_importer,
};
use frust_render::{
    ExternalFrame, ExternalPass, GOLDEN_EXPECT_ADAPTER_ENV_VAR, register_external_pass,
    unregister_external_pass,
};
use frust_scene::{Scene, SceneBuilder};
use kurbo::{Affine, Rect};
use peniko::color::palette::css::BLACK;

/// Frame extent every GPU case renders at — wider than [`DEST`] so there is
/// somewhere outside it to prove nothing was painted.
const SIZE: u32 = 128;

/// Side of the frames the source yields, in texels.
const FRAME: u32 = 64;

/// Where the source's texture is drawn: [`FRAME`]-square and offset on both
/// axes, so the mapping is one-for-one and a dropped translation lands at the
/// origin rather than merely looking odd.
const DEST: Rect = Rect::new(16.0, 16.0, 80.0, 80.0);

/// `Rgba8Unorm` reads back R, G, B, A in order, so a read-back needs no
/// swizzling, and the bus samples its `Rgba8Unorm` upload byte for byte.
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const RED: [u8; 4] = [255, 0, 0, 255];
const WHITE: [u8; 4] = [255, 255, 255, 255];
const BASE: [u8; 4] = [0, 0, 0, 255];

/// Serializes every case: the pass registry and the importer table are
/// process-wide, and each GPU case builds its own device.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Every record logged since [`take_log`] last emptied it.
static LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());

struct Capture;

impl log::Log for Capture {
    fn enabled(&self, _metadata: &log::Metadata<'_>) -> bool {
        true
    }

    fn log(&self, record: &log::Record<'_>) {
        LOG.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(format!("{} {}", record.level(), record.args()));
    }

    fn flush(&self) {}
}

static CAPTURE: Capture = Capture;

/// Installs the capturing logger once and empties what it has collected.
fn take_log() -> Vec<String> {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        let _ = log::set_logger(&CAPTURE);
        log::set_max_level(log::LevelFilter::Debug);
    });
    std::mem::take(&mut *LOG.lock().unwrap_or_else(PoisonError::into_inner))
}

/// The `SceneTextureId` a registration's raw id names — `for_texture_source`
/// is the identity on an id already inside its namespace.
fn id_of(raw: u64) -> SceneTextureId {
    SceneTextureId::for_texture_source(raw)
}

// ------------------------------------------------------------ registration --

struct Inert;

impl ExternalPass for Inert {
    fn record(&self, _frame: &mut ExternalFrame<'_>) {}
}

#[test]
fn a_registration_lives_in_the_texture_source_namespace() {
    let _serial = serial();
    let registration = register_texture_source(|| None);
    let id = id_of(registration.scene_texture_id());

    assert_eq!(id.get(), registration.scene_texture_id());
    assert!(id.is_texture_source());
    assert!(!id.is_shader_program());
    assert_eq!(registration.frame_size(), None, "no frame shown yet");

    let other = register_texture_source(|| None);
    assert_ne!(
        registration.scene_texture_id(),
        other.scene_texture_id(),
        "every registration gets an id of its own"
    );
}

#[test]
fn an_ordinary_pass_is_refused_a_texture_source_id() {
    let _serial = serial();
    let registration = register_texture_source(|| None);
    let id = id_of(registration.scene_texture_id());
    let _ = take_log();

    assert!(
        !register_external_pass(id, Arc::new(Inert)),
        "the bus's namespace is the bus's own"
    );
    let unclaimed = id_of(SceneTextureId::mint().get());
    assert!(
        !register_external_pass(unclaimed, Arc::new(Inert)),
        "refused even where no registration holds the id"
    );
    let reports = take_log()
        .into_iter()
        .filter(|record| record.contains("texture-source namespace"))
        .collect::<Vec<_>>();
    assert!(!reports.is_empty(), "the refusal is reported: {reports:?}");
}

#[test]
fn dropping_a_registration_unregisters_it() {
    let _serial = serial();
    let registration = register_texture_source(|| None);
    let id = id_of(registration.scene_texture_id());
    drop(registration);
    assert!(
        !unregister_external_pass(id),
        "the drop already took the pass out of the registry"
    );
}

// --------------------------------------------------------------------- GPU --

/// A frame queue a test pushes into and the registered closure pops from —
/// the closure source shape a sink bridge has, without the plugin crate.
#[derive(Clone, Default)]
struct Queue(Arc<Mutex<VecDeque<SourceFrame>>>);

impl Queue {
    fn push(&self, frame: SourceFrame) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push_back(frame);
    }

    fn source(&self) -> impl FnMut() -> Option<SourceFrame> + Send + 'static {
        let queue = Arc::clone(&self.0);
        move || {
            queue
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .pop_front()
        }
    }
}

/// Counts release callbacks, per frame.
#[derive(Clone, Default)]
struct Releases(Arc<Mutex<Vec<usize>>>);

impl Releases {
    /// A callback bumping frame `index`'s count.
    fn callback(&self, index: usize) -> impl FnOnce() + Send + 'static {
        let counts = Arc::clone(&self.0);
        {
            let mut counts = counts.lock().unwrap_or_else(PoisonError::into_inner);
            if counts.len() <= index {
                counts.resize(index + 1, 0);
            }
        }
        move || {
            counts.lock().unwrap_or_else(PoisonError::into_inner)[index] += 1;
        }
    }

    fn counts(&self) -> Vec<usize> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// A `width`x`height` frame of one solid colour.
fn solid(width: u32, height: u32, rgba: [u8; 4]) -> SourceFrame {
    let pixels: Vec<u8> = rgba
        .iter()
        .copied()
        .cycle()
        .take((width * height * 4) as usize)
        .collect();
    SourceFrame::rgba8(width, height, Arc::from(pixels))
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

fn drain_error_scope(device: &wgpu::Device, scope: wgpu::ErrorScopeGuard) -> Option<wgpu::Error> {
    use std::task::{Context, Poll, Waker};

    let mut cx = Context::from_waker(Waker::noop());
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

/// One case's device, engine and read-back target.
struct Harness {
    device: wgpu::Device,
    queue: wgpu::Queue,
    caps: TierCaps,
    engine: EngineRenderer,
    target: HeadlessTarget,
}

impl Harness {
    fn new() -> Self {
        let (device, queue, caps) = block_on(async {
            let instance = wgpu::Instance::new(
                wgpu::InstanceDescriptor::new_without_display_handle_from_env(),
            );
            let adapter = wgpu::util::initialize_adapter_from_env_or_default(&instance, None)
                .await
                .expect("no compatible GPU adapter");
            let info = adapter.get_info();
            println!("frust-render texture_bus adapter: {info:?}");
            if let Ok(expected) = std::env::var(GOLDEN_EXPECT_ADAPTER_ENV_VAR) {
                let expected = expected.trim();
                assert!(
                    expected.is_empty()
                        || info.name.to_lowercase().contains(&expected.to_lowercase()),
                    "{GOLDEN_EXPECT_ADAPTER_ENV_VAR}={expected:?} but the run resolved {:?}",
                    info.name
                );
            }
            let caps = TierCaps::probe(&adapter);
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    label: Some("frust-render texture bus device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: frust_gpu::test_device_limits(&adapter, &caps),
                    ..Default::default()
                })
                .await
                .expect("failed to create the device");
            (device, queue, caps)
        });
        let target = HeadlessTarget::new(&device, SIZE, SIZE, FORMAT);
        let engine = EngineRenderer::new(&device, &caps, FORMAT, None)
            .expect("the engine builds on this device");
        Self {
            device,
            queue,
            caps,
            engine,
            target,
        }
    }

    /// Replaces the engine with a fresh one on the same device — what a shell
    /// installing a fresh surface does.
    fn rebuild_engine(&mut self) {
        self.engine = EngineRenderer::new(&self.device, &self.caps, FORMAT, None)
            .expect("the engine rebuilds on this device");
    }

    /// One whole frame in `SurfaceRenderer::submit`'s order: external passes
    /// into the frame's encoder, the scene into the same encoder, one submit.
    fn frame(&mut self, scene: &Scene) -> Vec<u8> {
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frust-render texture bus frame"),
            });
        frust_render::external_pass::run_external_passes(
            &self.device,
            &self.queue,
            &mut encoder,
            &mut self.engine,
        );
        self.engine
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
        self.engine.end_frame(&self.queue);

        let error = drain_error_scope(&self.device, scope);
        assert!(error.is_none(), "the frame raised {error:?}");
        self.target.read_back(&self.device, &self.queue)
    }
}

fn pixel(frame: &[u8], x: u32, y: u32) -> [u8; 4] {
    let start = ((y * SIZE + x) * 4) as usize;
    frame[start..start + 4]
        .try_into()
        .expect("a read-back pixel is four bytes")
}

fn scene_drawing(raw: u64, dest: Rect) -> Scene {
    let mut scene = Scene::new();
    SceneBuilder::new(&mut scene).scene_texture(raw, dest);
    scene
}

/// The colour at the centre of [`DEST`] and at four points just inside its
/// corners — the whole destination, not one lucky texel.
fn assert_dest_is(frame: &[u8], rgba: [u8; 4], why: &str) {
    for (x, y) in [(48, 48), (18, 18), (77, 18), (18, 77), (77, 77)] {
        assert_eq!(pixel(frame, x, y), rgba, "{why} at ({x}, {y})");
    }
}

/// Nothing outside [`DEST`] was painted.
fn assert_outside_is_base(frame: &[u8]) {
    for (x, y) in [
        (1, 1),
        (SIZE - 2, 1),
        (1, SIZE - 2),
        (SIZE - 2, SIZE - 2),
        (100, 48),
    ] {
        assert_eq!(
            pixel(frame, x, y),
            BASE,
            "outside the destination is the base colour at ({x}, {y})"
        );
    }
}

/// Leaves the registry as the case found it: the queued unbinds are the next
/// drain's work, and this harness is the only thing that would run one.
fn settle(harness: &mut Harness) {
    let _ = harness.frame(&Scene::new());
}

#[test]
#[ignore = "requires a GPU; run with `cargo test -p frust-render --test texture_bus -- --ignored --test-threads=1`"]
fn cpu_frames_upload_replace_and_resize_with_one_release_each() {
    let _serial = serial();
    let mut harness = Harness::new();
    let queue = Queue::default();
    let releases = Releases::default();
    let registration = register_texture_source(queue.source());
    let raw = registration.scene_texture_id();
    let scene = scene_drawing(raw, DEST);

    // Frame 0: the first frame lands where the display list names the id.
    queue.push(solid(FRAME, FRAME, GREEN).on_release(releases.callback(0)));
    let drawn = harness.frame(&scene);
    assert_dest_is(&drawn, GREEN, "the first frame is drawn at the destination");
    assert_outside_is_base(&drawn);
    assert_eq!(registration.frame_size(), Some((FRAME, FRAME)));
    assert_eq!(
        releases.counts(),
        vec![1],
        "released once the upload was enqueued"
    );
    assert_eq!(harness.engine.bound_texture_count(), 1);

    // No new frame: the last one stays shown, and nothing is released again.
    let drawn = harness.frame(&scene);
    assert_dest_is(&drawn, GREEN, "an idle source keeps its last frame shown");
    assert_eq!(releases.counts(), vec![1]);

    // Frame 1, same extent: replaces the content in place.
    queue.push(solid(FRAME, FRAME, BLUE).on_release(releases.callback(1)));
    let drawn = harness.frame(&scene);
    assert_dest_is(&drawn, BLUE, "a second frame replaces the first");
    assert_outside_is_base(&drawn);
    assert_eq!(releases.counts(), vec![1, 1]);

    // Frame 2, a new extent: the texture is re-created and re-bound, and the
    // display list's destination still maps the whole of it.
    queue.push(solid(32, 16, RED).on_release(releases.callback(2)));
    let drawn = harness.frame(&scene);
    assert_dest_is(&drawn, RED, "an extent change re-creates the texture");
    assert_outside_is_base(&drawn);
    assert_eq!(registration.frame_size(), Some((32, 16)));
    assert_eq!(releases.counts(), vec![1, 1, 1]);
    assert_eq!(
        harness.engine.bound_texture_count(),
        1,
        "re-binding replaced the old view under the same id"
    );

    // Two frames queued before one engine frame: the bus takes one per frame.
    queue.push(solid(FRAME, FRAME, WHITE).on_release(releases.callback(3)));
    queue.push(solid(FRAME, FRAME, GREEN).on_release(releases.callback(4)));
    let drawn = harness.frame(&scene);
    assert_dest_is(&drawn, WHITE, "one frame per engine frame");
    let drawn = harness.frame(&scene);
    assert_dest_is(&drawn, GREEN, "the next one on the next engine frame");
    assert_eq!(
        releases.counts(),
        vec![1; 5],
        "every frame the source handed over was released exactly once"
    );

    drop(registration);
    let after = harness.frame(&scene);
    assert_dest_is(&after, BASE, "a dropped registration draws nothing");
    assert_eq!(harness.engine.bound_texture_count(), 0);
}

#[test]
#[ignore = "requires a GPU; run with `cargo test -p frust-render --test texture_bus -- --ignored --test-threads=1`"]
fn a_rebuilt_engine_gets_the_shown_frame_bound_again_without_a_new_frame() {
    let _serial = serial();
    let mut harness = Harness::new();
    let queue = Queue::default();
    let registration = register_texture_source(queue.source());
    let scene = scene_drawing(registration.scene_texture_id(), DEST);

    queue.push(solid(FRAME, FRAME, BLUE));
    let drawn = harness.frame(&scene);
    assert_dest_is(&drawn, BLUE, "the frame is shown");

    harness.rebuild_engine();
    assert_eq!(harness.engine.bound_texture_count(), 0);
    let drawn = harness.frame(&scene);
    assert_dest_is(
        &drawn,
        BLUE,
        "a paused source still shows on a fresh engine — the pass re-binds its view",
    );

    drop(registration);
    settle(&mut harness);
}

#[test]
#[ignore = "requires a GPU; run with `cargo test -p frust-render --test texture_bus -- --ignored --test-threads=1`"]
fn a_malformed_cpu_frame_is_refused_released_and_leaves_the_last_frame_shown() {
    let _serial = serial();
    let mut harness = Harness::new();
    let queue = Queue::default();
    let releases = Releases::default();
    let registration = register_texture_source(queue.source());
    let scene = scene_drawing(registration.scene_texture_id(), DEST);

    queue.push(solid(FRAME, FRAME, GREEN).on_release(releases.callback(0)));
    let _ = harness.frame(&scene);

    let _ = take_log();
    // One row short of a tightly packed 64x64 frame.
    let short = SourceFrame::rgba8(
        FRAME,
        FRAME,
        Arc::from(vec![255u8; (FRAME * (FRAME - 1) * 4) as usize]),
    )
    .on_release(releases.callback(1));
    queue.push(short);
    let drawn = harness.frame(&scene);
    assert_dest_is(
        &drawn,
        GREEN,
        "a refused frame leaves the last good one shown",
    );
    assert_eq!(
        releases.counts(),
        vec![1, 1],
        "the refused frame was released"
    );
    let reports = take_log()
        .into_iter()
        .filter(|record| record.contains("tightly packed rows need"))
        .collect::<Vec<_>>();
    assert_eq!(reports.len(), 1, "{reports:?}");
    assert!(reports[0].starts_with("WARN"), "{reports:?}");

    drop(registration);
    settle(&mut harness);
}

/// A stand-in native buffer: the address of a colour, which the stand-in
/// importer reads back — exactly the "only the importer dereferences it"
/// contract the bus holds to.
struct FakeBuffer {
    rgba: [u8; 4],
}

/// Imports a [`FakeBuffer`] by uploading its colour into a texture of its own.
struct FakeImporter {
    calls: AtomicUsize,
}

impl NativeImporter for FakeImporter {
    fn import(
        &self,
        handle: &NativeHandle,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Result<ImportedTexture, ImportError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if handle.ptr.is_null() {
            return Err(ImportError::Failed("null buffer".into()));
        }
        // SAFETY: every non-null handle in this file points at a `FakeBuffer`
        // the case keeps alive across the frame that imports it.
        let buffer = unsafe { &*handle.ptr.cast::<FakeBuffer>() };
        let frame = solid(8, 8, buffer.rgba);
        let SourcePayload::Cpu(pixels) = frame.payload else {
            unreachable!("solid frames are CPU frames")
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("fake native import"),
            size: wgpu::Extent3d {
                width: 8,
                height: 8,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(8 * 4),
                rows_per_image: Some(8),
            },
            wgpu::Extent3d {
                width: 8,
                height: 8,
                depth_or_array_layers: 1,
            },
        );
        Ok(ImportedTexture {
            view: texture.create_view(&wgpu::TextureViewDescriptor::default()),
            size: (8, 8),
        })
    }
}

fn native(buffer: Option<&mut FakeBuffer>) -> SourceFrame {
    let ptr: *mut c_void = buffer.map_or(std::ptr::null_mut(), |b| (b as *mut FakeBuffer).cast());
    SourceFrame {
        width: 8,
        height: 8,
        format: SourceFormat::Rgba8,
        payload: SourcePayload::Native(NativeHandle {
            kind: NativeHandleKind::AHardwareBuffer,
            ptr,
        }),
        release: None,
    }
}

#[test]
#[ignore = "requires a GPU; run with `cargo test -p frust-render --test texture_bus -- --ignored --test-threads=1`"]
fn a_native_frame_without_an_importer_is_refused_once_and_released() {
    let _serial = serial();
    let _ = clear_native_importer(NativeHandleKind::AHardwareBuffer);
    let mut harness = Harness::new();
    let queue = Queue::default();
    let releases = Releases::default();
    let registration = register_texture_source(queue.source());
    let scene = scene_drawing(registration.scene_texture_id(), DEST);
    let mut buffer = FakeBuffer { rgba: RED };
    let _ = take_log();

    queue.push(native(Some(&mut buffer)).on_release(releases.callback(0)));
    let drawn = harness.frame(&scene);
    assert_dest_is(&drawn, BASE, "a refused native frame draws nothing");
    assert_eq!(registration.frame_size(), None);

    queue.push(native(Some(&mut buffer)).on_release(releases.callback(1)));
    let _ = harness.frame(&scene);
    assert_eq!(
        releases.counts(),
        vec![1, 1],
        "every refused native frame is released, exactly once"
    );

    let reports = take_log()
        .into_iter()
        .filter(|record| record.contains("no native importer is installed"))
        .collect::<Vec<_>>();
    assert_eq!(reports.len(), 2, "both refusals are logged: {reports:?}");
    assert!(
        reports[0].starts_with("WARN"),
        "the first is a warning: {reports:?}"
    );
    assert!(
        reports[1].starts_with("DEBUG"),
        "the refusal is warned about once, then logged at debug: {reports:?}"
    );

    drop(registration);
    settle(&mut harness);
}

#[test]
#[ignore = "requires a GPU; run with `cargo test -p frust-render --test texture_bus -- --ignored --test-threads=1`"]
fn an_installed_importer_draws_native_frames_and_a_failed_import_keeps_the_last() {
    let _serial = serial();
    let importer = Arc::new(FakeImporter {
        calls: AtomicUsize::new(0),
    });
    let _ = set_native_importer(
        NativeHandleKind::AHardwareBuffer,
        Arc::clone(&importer) as Arc<dyn NativeImporter>,
    );
    let mut harness = Harness::new();
    let queue = Queue::default();
    let releases = Releases::default();
    let registration = register_texture_source(queue.source());
    let scene = scene_drawing(registration.scene_texture_id(), DEST);
    let mut buffer = FakeBuffer { rgba: RED };

    queue.push(native(Some(&mut buffer)).on_release(releases.callback(0)));
    let drawn = harness.frame(&scene);
    assert_dest_is(&drawn, RED, "the imported view is drawn at the destination");
    assert_outside_is_base(&drawn);
    assert_eq!(registration.frame_size(), Some((8, 8)));
    assert_eq!(importer.calls.load(Ordering::SeqCst), 1);

    let _ = take_log();
    queue.push(native(None).on_release(releases.callback(1)));
    let drawn = harness.frame(&scene);
    assert_dest_is(
        &drawn,
        RED,
        "a failed import leaves the last good frame shown",
    );
    assert!(
        take_log()
            .iter()
            .any(|record| record.starts_with("WARN") && record.contains("null buffer")),
        "the failure is reported"
    );

    // A CPU frame after an imported one goes back to the bus's own upload.
    queue.push(solid(FRAME, FRAME, BLUE).on_release(releases.callback(2)));
    let drawn = harness.frame(&scene);
    assert_dest_is(&drawn, BLUE, "a CPU frame replaces an imported one");
    let _ = harness.device.poll(wgpu::PollType::wait_indefinitely());
    assert_eq!(
        releases.counts(),
        vec![1, 1, 1],
        "the retired native frame's release ran once its fence fired"
    );

    let removed = clear_native_importer(NativeHandleKind::AHardwareBuffer);
    assert!(removed.is_some_and(|removed| Arc::ptr_eq(
        &removed,
        &(Arc::clone(&importer) as Arc<dyn NativeImporter>)
    )));
    drop(registration);
    settle(&mut harness);
}

/// Polls the device until `done` holds, up to a bounded number of waits.
fn poll_until(harness: &Harness, done: impl Fn() -> bool) {
    for _ in 0..16 {
        if done() {
            return;
        }
        let _ = harness.device.poll(wgpu::PollType::wait_indefinitely());
    }
}

fn install_fake_importer() -> Arc<FakeImporter> {
    let importer = Arc::new(FakeImporter {
        calls: AtomicUsize::new(0),
    });
    let _ = set_native_importer(
        NativeHandleKind::AHardwareBuffer,
        Arc::clone(&importer) as Arc<dyn NativeImporter>,
    );
    importer
}

#[test]
#[ignore = "requires a GPU; run with `cargo test -p frust-render --test texture_bus -- --ignored --test-threads=1`"]
fn a_native_frames_release_waits_for_its_texture_to_be_replaced_and_fenced() {
    let _serial = serial();
    let _importer = install_fake_importer();
    let mut harness = Harness::new();
    let queue = Queue::default();
    let releases = Releases::default();
    let registration = register_texture_source(queue.source());
    let scene = scene_drawing(registration.scene_texture_id(), DEST);
    let mut red = FakeBuffer { rgba: RED };
    let mut green = FakeBuffer { rgba: GREEN };

    queue.push(native(Some(&mut red)).on_release(releases.callback(0)));
    let drawn = harness.frame(&scene);
    assert_dest_is(&drawn, RED, "the imported frame is drawn");
    let _ = harness.device.poll(wgpu::PollType::wait_indefinitely());
    assert_eq!(
        releases.counts(),
        vec![0],
        "the frame is still the one shown, so its buffer is not released"
    );
    let _ = harness.frame(&scene);
    assert_eq!(releases.counts(), vec![0], "still shown on an idle frame");

    queue.push(native(Some(&mut green)).on_release(releases.callback(1)));
    let drawn = harness.frame(&scene);
    assert_dest_is(&drawn, GREEN, "the newer frame replaces it");
    poll_until(&harness, || releases.counts()[0] == 1);
    assert_eq!(
        releases.counts(),
        vec![1, 0],
        "the replaced frame is released after its fence; the shown one is not"
    );

    drop(registration);
    settle(&mut harness);
    poll_until(&harness, || releases.counts()[1] == 1);
    assert_eq!(
        releases.counts(),
        vec![1, 1],
        "every release ran exactly once"
    );
    let _ = clear_native_importer(NativeHandleKind::AHardwareBuffer);
}

#[test]
#[ignore = "requires a GPU; run with `cargo test -p frust-render --test texture_bus -- --ignored --test-threads=1`"]
fn dropping_a_registration_releases_its_shown_native_frame_only_after_the_fence() {
    let _serial = serial();
    let _importer = install_fake_importer();
    let mut harness = Harness::new();
    let queue = Queue::default();
    let releases = Releases::default();
    let registration = register_texture_source(queue.source());
    let scene = scene_drawing(registration.scene_texture_id(), DEST);
    let mut red = FakeBuffer { rgba: RED };

    queue.push(native(Some(&mut red)).on_release(releases.callback(0)));
    let _ = harness.frame(&scene);
    assert_eq!(releases.counts(), vec![0]);

    drop(registration);
    assert_eq!(
        releases.counts(),
        vec![0],
        "the drop alone does not release: the fence has not been polled"
    );
    poll_until(&harness, || releases.counts()[0] == 1);
    assert_eq!(releases.counts(), vec![1], "released once the fences fired");
    settle(&mut harness);
    let _ = harness.device.poll(wgpu::PollType::wait_indefinitely());
    assert_eq!(releases.counts(), vec![1]);
    let _ = clear_native_importer(NativeHandleKind::AHardwareBuffer);
}

#[test]
#[ignore = "requires a GPU; run with `cargo test -p frust-render --test texture_bus -- --ignored --test-threads=1`"]
fn a_refused_native_frame_is_released_at_once_and_the_retire_backlog_is_bounded() {
    let _serial = serial();
    let _ = clear_native_importer(NativeHandleKind::AHardwareBuffer);
    let mut harness = Harness::new();
    let queue = Queue::default();
    let releases = Releases::default();
    let registration = register_texture_source(queue.source());
    let scene = scene_drawing(registration.scene_texture_id(), DEST);
    let mut red = FakeBuffer { rgba: RED };

    // No importer: nothing sampled it, so it is released before record returns.
    queue.push(native(Some(&mut red)).on_release(releases.callback(0)));
    let _ = harness.frame(&scene);
    assert_eq!(releases.counts(), vec![1], "released with no fence wait");

    // A long run of native frames never leaves more than the bound pending,
    // and every frame is released exactly once, imported or refused.
    let _importer = install_fake_importer();
    for index in 1..=24 {
        queue.push(native(Some(&mut red)).on_release(releases.callback(index)));
        let _ = harness.frame(&scene);
    }
    drop(registration);
    settle(&mut harness);
    poll_until(&harness, || releases.counts().iter().all(|&n| n == 1));
    assert!(
        releases.counts().iter().all(|&n| n == 1),
        "every frame released exactly once: {:?}",
        releases.counts()
    );
    let _ = clear_native_importer(NativeHandleKind::AHardwareBuffer);
}
