//! The [`frust_render::ExternalPass`] seam: the registry's own bookkeeping,
//! device-free, and the whole path on real hardware.
//!
//! The device-free cases pin what a caller can observe without a GPU at all —
//! that a registration is a claim on an id rather than a silent takeover,
//! that unregistering an id nothing holds is an honest `false`, and that an
//! id inside the reserved shader-program namespace is refused outright. They
//! run everywhere.
//!
//! The `#[ignore]`d cases are the ones that matter, and they are the ones
//! only a real queue can make: a pass recording into the *frame's own*
//! encoder, ahead of the scene, binding a target it created itself, and the
//! engine compositing that target where the display list names its id — plus
//! a pass that panics with its own render pass still open, proving an unwind
//! through that open borrow leaves the frame's encoder just as usable for
//! whatever records after it. Nothing about any of that produces a wrong
//! value when it is wrong — it produces an untouched target, a validation
//! error, or the base colour where the texture should be — so it takes a
//! read-back to tell those apart:
//!
//! ```text
//! WGPU_BACKEND=vulkan WGPU_ADAPTER_NAME=T400 FRUST_GOLDEN_EXPECT_ADAPTER=T400 \
//!   cargo test -p frust-render --test external_pass -- --ignored --nocapture
//! ```
//!
//! The drain is driven directly rather than through `SurfaceRenderer::submit`
//! because `submit` needs a swapchain and so a window; what is driven is the
//! identical function that arm calls, in the identical position — before the
//! scene is encoded, into the encoder the scene is then encoded into.

#![cfg(not(target_arch = "wasm32"))]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Once, PoisonError};

use frust_engine::{EngineRenderer, EngineTarget, OutputAlpha};
use frust_gpu::{HeadlessTarget, SceneTextureId, TierCaps};
use frust_render::{
    ExternalFrame, ExternalPass, GOLDEN_EXPECT_ADAPTER_ENV_VAR, register_external_pass,
    unregister_external_pass,
};
use frust_scene::{Scene, SceneBuilder};
use kurbo::{Affine, Rect};
use peniko::Color;
use peniko::color::palette::css::BLACK;

/// Frame extent every GPU case renders at — wider than the texture under test
/// so there is somewhere outside the destination rectangle to prove nothing
/// was painted.
const SIZE: u32 = 128;

/// Side of the target a pass renders into, in texels.
const TEXTURE: u32 = 64;

/// Where a pass's texture is drawn: [`TEXTURE`]-square and offset from the
/// origin on both axes, so the mapping is one-for-one and a dropped
/// translation lands at `(0, 0)` rather than merely looking odd.
const DEST: Rect = Rect::new(16.0, 16.0, 80.0, 80.0);

/// The second pass's destination in the two-pass case — disjoint from
/// [`DEST`], so one pass's pixels showing up in the other's rectangle is a
/// failure rather than a coincidence.
const SECOND_DEST: Rect = Rect::new(88.0, 16.0, 120.0, 48.0);

/// `Rgba8Unorm` reads back R, G, B, A in that order, so a read-back needs no
/// swizzling — and a clear value of `1.0` lands as exactly `255`, which is
/// what lets a pass's colour be asserted as an exact byte triple.
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Serializes every case in this binary: the registry under test is
/// process-wide, and the GPU cases each build their own `wgpu::Device` (a
/// driver that serializes device teardown on a process-global mutex deadlocks
/// when two of them tear down at once). Poison is ignored deliberately — one
/// failing case must not cascade into every sibling.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Every record the process has logged since [`take_log`] last emptied it.
static LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// The capturing logger: the engine reports a display list naming an
/// unregistered id through `log`, and that report is part of what this file
/// pins — an invisible blank is exactly the failure a caller has no other
/// signal for.
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

/// Installs the capturing logger once per process and empties whatever it has
/// collected so far, so a case reads only its own records. Installation may
/// lose the race against a logger someone else set; every case that asserts
/// on records checks its own capture rather than assuming one.
fn take_log() -> Vec<String> {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        let _ = log::set_logger(&CAPTURE);
        log::set_max_level(log::LevelFilter::Debug);
    });
    std::mem::take(&mut *LOG.lock().unwrap_or_else(PoisonError::into_inner))
}

// ---------------------------------------------------------------- registry --

/// A pass that records nothing: enough for every claim about the registry's
/// own bookkeeping, which never reaches a device.
struct Inert;

impl ExternalPass for Inert {
    fn record(&self, _frame: &mut ExternalFrame<'_>) {}
}

#[test]
fn a_second_registration_of_one_id_is_refused_rather_than_taking_it_over() {
    let _serial = serial();
    let id = SceneTextureId::mint();

    assert!(
        register_external_pass(id, Arc::new(Inert)),
        "a fresh id registers"
    );
    assert!(
        !register_external_pass(id, Arc::new(Inert)),
        "a claimed id is refused, leaving the first pass registered"
    );

    assert!(unregister_external_pass(id));
}

#[test]
fn unregistering_reports_whether_there_was_anything_to_remove() {
    let _serial = serial();
    let id = SceneTextureId::mint();

    assert!(
        !unregister_external_pass(id),
        "an id nothing registered has nothing to remove"
    );

    assert!(register_external_pass(id, Arc::new(Inert)));
    assert!(unregister_external_pass(id));
    assert!(
        !unregister_external_pass(id),
        "the second removal of the same id finds it already gone"
    );
}

#[test]
fn an_id_freed_by_unregistering_can_be_claimed_again() {
    let _serial = serial();
    let id = SceneTextureId::mint();

    assert!(register_external_pass(id, Arc::new(Inert)));
    assert!(unregister_external_pass(id));
    assert!(
        register_external_pass(id, Arc::new(Inert)),
        "handing an id over is unregister-then-register, and it works"
    );

    assert!(unregister_external_pass(id));
}

#[test]
fn a_reserved_shader_program_namespace_id_is_refused_registration() {
    let _serial = serial();
    // Derived from a minted id's own ordinal rather than a literal: any
    // ordinal works, `for_shader_program` is what puts it in the reserved
    // range, and this keeps the case from depending on a hardcoded number.
    let id = SceneTextureId::for_shader_program(SceneTextureId::mint().get());
    let _ = take_log();

    assert!(
        !register_external_pass(id, Arc::new(Inert)),
        "an id inside the reserved shader-program namespace is refused"
    );
    assert!(
        !unregister_external_pass(id),
        "nothing was ever registered, so there is nothing to unregister"
    );

    // Once a reserved-namespace refusal is warned, all further attempts
    // report at debug level — the refusal reason is identical for every id,
    // so there's no need to warn per-id.
    assert!(!register_external_pass(id, Arc::new(Inert)));
    let id2 = SceneTextureId::for_shader_program(SceneTextureId::mint().get());
    assert!(!register_external_pass(id2, Arc::new(Inert)));
    let reports = take_log()
        .into_iter()
        .filter(|record| record.contains("reserved shader-program namespace"))
        .collect::<Vec<_>>();
    assert!(
        !reports.is_empty(),
        "reserved-namespace refusals are reported: {reports:?}"
    );
    assert!(
        reports[0].starts_with("WARN"),
        "the first reserved-namespace refusal is a warning: {reports:?}"
    );
    if reports.len() > 1 {
        assert!(
            reports[1].starts_with("DEBUG"),
            "further reserved-namespace refusals are logged at debug: {reports:?}"
        );
    }
}

// --------------------------------------------------------------------- GPU --

/// A pass that clears a target of its own to `color` and binds it — the
/// smallest piece of real GPU work that still exercises every part of the
/// seam: it allocates from the frame's device, records into the frame's
/// encoder, and hands the engine a view it created itself.
struct SolidPass {
    id: SceneTextureId,
    color: wgpu::Color,
    /// Set the first time [`ExternalPass::record`] runs, so a case can tell
    /// "the pass drew the wrong thing" from "the pass was never called".
    recorded: AtomicBool,
}

impl SolidPass {
    fn new(id: SceneTextureId, color: wgpu::Color) -> Self {
        Self {
            id,
            color,
            recorded: AtomicBool::new(false),
        }
    }
}

impl ExternalPass for SolidPass {
    fn record(&self, frame: &mut ExternalFrame<'_>) {
        assert_eq!(
            frame.id(),
            self.id,
            "the frame handed to a pass is scoped to that pass's own registered id"
        );
        let texture = frame.device().create_texture(&wgpu::TextureDescriptor {
            label: Some("external pass target"),
            size: wgpu::Extent3d {
                width: TEXTURE,
                height: TEXTURE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        // A clear is the whole pass: no pipeline, no draw, and an exact known
        // value in every texel of the target.
        frame
            .encoder()
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("external pass clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(self.color),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        frame.bind_texture((TEXTURE, TEXTURE), view);
        self.recorded.store(true, Ordering::Relaxed);
    }
}

/// A pass that panics instead of recording anything.
struct PanickingPass;

impl ExternalPass for PanickingPass {
    fn record(&self, _frame: &mut ExternalFrame<'_>) {
        panic!("this pass panics on purpose");
    }
}

/// A pass that opens a render pass on its own target and panics with it still
/// open — never reaching the point where the `wgpu::RenderPass` borrow of the
/// frame's encoder would end on its own. What this exercises is the unwind
/// itself dropping that open borrow: `catch_unwind` only recovers *after*
/// unwinding has run every local's `Drop`, so by the time it returns the
/// render pass this struct opened has already been ended by its own
/// destructor, and the frame's encoder is left exactly as usable for the
/// pass recorded after it as it would be following an ordinary return.
struct PanicMidRenderPass;

impl ExternalPass for PanicMidRenderPass {
    fn record(&self, frame: &mut ExternalFrame<'_>) {
        let texture = frame.device().create_texture(&wgpu::TextureDescriptor {
            label: Some("panic mid render pass target"),
            size: wgpu::Extent3d {
                width: TEXTURE,
                height: TEXTURE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let _open = frame
            .encoder()
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("panic mid render pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(GREEN),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        // `_open` is still alive (not dropped) at this point — the panic
        // unwinds straight through it.
        panic!("this pass panics with its own render pass still open");
    }
}

/// Blocks on `future` by polling it to completion — wgpu's native adapter and
/// device requests resolve without an executor driving them, and this crate's
/// tests have no async runtime.
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

/// One case's device, engine and read-back target.
struct Harness {
    device: wgpu::Device,
    queue: wgpu::Queue,
    engine: EngineRenderer,
    target: HeadlessTarget,
}

impl Harness {
    fn new() -> Self {
        let (device, queue, caps) = block_on(async {
            let instance = wgpu::Instance::new(
                wgpu::InstanceDescriptor::new_without_display_handle_from_env(),
            );
            // The environment-aware initializer, so `WGPU_ADAPTER_NAME` picks
            // the GPU on a multi-adapter host instead of the run silently
            // landing on whichever one enumerates first.
            let adapter = wgpu::util::initialize_adapter_from_env_or_default(&instance, None)
                .await
                .expect("no compatible GPU adapter");
            let info = adapter.get_info();
            println!("frust-render external_pass adapter: {info:?}");
            // The provenance rule every GPU run records (`docs/TESTING.md` §
            // GPU Run Metadata): naming the expected adapter turns "which GPU
            // did this run on?" into a refusal rather than a footnote.
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
                    label: Some("frust-render external pass device"),
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
            engine,
            target,
        }
    }

    /// One whole frame, in the order `SurfaceRenderer::submit` records it:
    /// the external passes into the frame's own encoder first, then the scene
    /// into that same encoder, then one submit and one `end_frame`.
    fn frame(&mut self, scene: &Scene) -> Vec<u8> {
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frust-render external pass frame"),
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

/// A scene drawing the texture `id` names over `dest`.
fn scene_drawing(id: SceneTextureId, dest: Rect) -> Scene {
    let mut scene = Scene::new();
    SceneBuilder::new(&mut scene).scene_texture(id.get(), dest);
    scene
}

/// The centre of `rect`, as device pixels.
fn centre(rect: Rect) -> (u32, u32) {
    (rect.center().x as u32, rect.center().y as u32)
}

/// Asserts nothing outside `rect` was painted: the four corners of the frame,
/// which every case's destination rectangle leaves alone.
fn assert_corners_are_base_colour(frame: &[u8]) {
    for probe in [(1, 1), (SIZE - 2, 1), (1, SIZE - 2), (SIZE - 2, SIZE - 2)] {
        assert_eq!(
            pixel(frame, probe.0, probe.1),
            rgba(BLACK),
            "the frame outside every destination rectangle is the base colour at {probe:?}"
        );
    }
}

/// Opaque pure green: `1.0` clears to exactly `255`, and a fully opaque colour
/// reads back the same premultiplied or straight, so the assertion is about
/// which texels were sampled rather than about how they were blended.
const GREEN: wgpu::Color = wgpu::Color {
    r: 0.0,
    g: 1.0,
    b: 0.0,
    a: 1.0,
};

/// Opaque pure blue, for telling a second pass's pixels from the first's.
const BLUE: wgpu::Color = wgpu::Color {
    r: 0.0,
    g: 0.0,
    b: 1.0,
    a: 1.0,
};

/// The byte quad a [`wgpu::Color`] cleared into an `Rgba8Unorm` target reads
/// back as.
fn cleared(color: wgpu::Color) -> [u8; 4] {
    [
        (color.r * 255.0) as u8,
        (color.g * 255.0) as u8,
        (color.b * 255.0) as u8,
        (color.a * 255.0) as u8,
    ]
}

#[test]
#[ignore = "requires a GPU; run with `cargo test -p frust-render --test external_pass -- --ignored`"]
fn a_registered_pass_renders_a_texture_the_scene_composites_and_unregistering_stops_it() {
    let _serial = serial();
    let mut harness = Harness::new();
    let id = SceneTextureId::mint();
    let pass = Arc::new(SolidPass::new(id, GREEN));
    assert!(register_external_pass(
        id,
        Arc::clone(&pass) as Arc<dyn ExternalPass>
    ));

    let scene = scene_drawing(id, DEST);
    let drawn = harness.frame(&scene);

    assert!(
        pass.recorded.load(Ordering::Relaxed),
        "the registered pass was called for the frame"
    );
    let (x, y) = centre(DEST);
    assert_eq!(
        pixel(&drawn, x, y),
        cleared(GREEN),
        "the destination rectangle holds what the pass rendered"
    );
    assert_corners_are_base_colour(&drawn);
    assert_eq!(
        harness.engine.bound_texture_count(),
        1,
        "the pass's binding is live for as long as the pass is registered"
    );

    // Unregistering queues the unbind; the next frame's drain performs it
    // before any pass runs, and the same scene then names an id nothing is
    // bound under — the display list's documented draw-nothing case.
    assert!(unregister_external_pass(id));
    let _ = take_log();
    let after = harness.frame(&scene);
    let unregistered_reports = take_log()
        .into_iter()
        .filter(|record| {
            record.contains("no texture is registered under that id")
                && record.contains(&id.get().to_string())
        })
        .collect::<Vec<_>>();

    assert_eq!(
        unregistered_reports.len(),
        1,
        "the engine reports the now-unregistered id exactly once, at warn: {unregistered_reports:?}"
    );
    assert!(
        unregistered_reports[0].starts_with("WARN"),
        "the first sighting of an unregistered id is a warning: {unregistered_reports:?}"
    );
    assert_eq!(
        harness.engine.bound_texture_count(),
        0,
        "the next frame cleared the engine binding of an id whose pass is gone"
    );
    assert_eq!(
        pixel(&after, x, y),
        rgba(BLACK),
        "an unregistered id draws nothing, not a retained view"
    );
    assert!(
        after.as_chunks::<4>().0.iter().all(|texel| *texel == rgba(BLACK)),
        "the whole frame is the base colour once the pass is unregistered"
    );
}

#[test]
#[ignore = "requires a GPU; run with `cargo test -p frust-render --test external_pass -- --ignored`"]
fn a_panicking_pass_is_retired_without_taking_the_frame_or_its_siblings_down() {
    let _serial = serial();
    let mut harness = Harness::new();
    // Minted in registration order, and the registry records passes in id
    // order, so the panicking one genuinely runs first.
    let panicking = SceneTextureId::mint();
    let solid = SceneTextureId::mint();
    let survivor = Arc::new(SolidPass::new(solid, BLUE));
    assert!(register_external_pass(panicking, Arc::new(PanickingPass)));
    assert!(register_external_pass(
        solid,
        Arc::clone(&survivor) as Arc<dyn ExternalPass>
    ));

    let mut scene = Scene::new();
    {
        let mut builder = SceneBuilder::new(&mut scene);
        builder.scene_texture(panicking.get(), DEST);
        builder.scene_texture(solid.get(), SECOND_DEST);
    }

    // The panic is expected, so the default hook's backtrace would be noise
    // in a passing run; every case in this binary is serialized, so nothing
    // else is panicking while the hook is swapped out.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let drawn = harness.frame(&scene);
    std::panic::set_hook(hook);

    assert!(
        survivor.recorded.load(Ordering::Relaxed),
        "a pass after the panicking one still ran"
    );
    let (x, y) = centre(SECOND_DEST);
    assert_eq!(
        pixel(&drawn, x, y),
        cleared(BLUE),
        "the surviving pass's texture was composited"
    );
    let (px, py) = centre(DEST);
    assert_eq!(
        pixel(&drawn, px, py),
        rgba(BLACK),
        "the panicking pass bound nothing, so its id draws nothing"
    );
    assert_corners_are_base_colour(&drawn);
    assert!(
        !unregister_external_pass(panicking),
        "the panicking pass was retired by the drain itself"
    );

    assert!(unregister_external_pass(solid));
    // Leave the registry as this case found it: the queued unbinds are the
    // next drain's work, and this harness is the only thing that would run
    // one.
    let _ = harness.frame(&Scene::new());
}

#[test]
#[ignore = "requires a GPU; run with `cargo test -p frust-render --test external_pass -- --ignored`"]
fn a_pass_panicking_with_its_own_render_pass_open_still_lets_the_frame_submit() {
    let _serial = serial();
    let mut harness = Harness::new();
    let panicking = SceneTextureId::mint();
    let solid = SceneTextureId::mint();
    let survivor = Arc::new(SolidPass::new(solid, BLUE));
    assert!(register_external_pass(
        panicking,
        Arc::new(PanicMidRenderPass)
    ));
    assert!(register_external_pass(
        solid,
        Arc::clone(&survivor) as Arc<dyn ExternalPass>
    ));

    let mut scene = Scene::new();
    {
        let mut builder = SceneBuilder::new(&mut scene);
        builder.scene_texture(panicking.get(), DEST);
        builder.scene_texture(solid.get(), SECOND_DEST);
    }

    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    // `Harness::frame` itself asserts the frame's validation error scope is
    // empty — the whole point of this case is that an unwind through an open
    // `wgpu::RenderPass` borrow of the encoder raises nothing there and
    // still lets the frame submit.
    let drawn = harness.frame(&scene);
    std::panic::set_hook(hook);

    assert!(
        survivor.recorded.load(Ordering::Relaxed),
        "a sibling pass after the mid-render-pass panic still ran"
    );
    let (x, y) = centre(SECOND_DEST);
    assert_eq!(
        pixel(&drawn, x, y),
        cleared(BLUE),
        "the surviving pass's texture was composited — the frame submitted at all"
    );
    let (px, py) = centre(DEST);
    assert_eq!(
        pixel(&drawn, px, py),
        rgba(BLACK),
        "the panicking pass's own render pass never reached bind_texture, so its id draws nothing"
    );
    assert_corners_are_base_colour(&drawn);
    assert!(
        !unregister_external_pass(panicking),
        "the panicking pass was retired by the drain itself"
    );

    assert!(unregister_external_pass(solid));
    let _ = harness.frame(&Scene::new());
}
