//! The feature-gated GPU-effect substrate every true-3D variant in this
//! catalog is built on.
//!
//! Behind the non-default `gpu-effects` cargo feature. A default build of
//! this crate contains none of it — no `wgpu` dependency, no module, no
//! symbol — and every component that can use it renders its ordinary 2D path
//! when it is absent *or* when it is present and the GPU is not reachable.
//! There is no third behaviour: the 3D path is an enhancement.
//!
//! # What it is
//!
//! Real perspective. A `SceneTexture` composites under an `Affine`, so a
//! widget subtree can be translated, scaled, skewed and rotated, but never
//! *perspective*-transformed — a 2D "3D card" is a shear, and it stays one no
//! matter how it is tuned. This substrate renders textured quads through an
//! actual perspective projection into an offscreen target, and hands that
//! target to the engine to composite. What tilts is what this substrate drew,
//! not the widget tree.
//!
//! # The four pieces
//!
//! - [`context`] — acquisition and the degrade contract. [`GpuFx::try_acquire`]
//!   answers `Option<`[`GpuFxHandle`]`>`; `None` means paint 2D.
//! - [`quad3d`] — the renderer. A [`Quad3dScene`] of [`Quad3d`] faces, each
//!   with a [`QuadFace`] fill, rendered under a perspective camera tuned for
//!   card-scale tilt, optionally depth-tested for occlusion.
//! - [`pool`] — offscreen targets, keyed per component and quantized extent,
//!   reaped by age, plus the bookkeeping keeping the engine's binding in step
//!   with them.
//! - [`schedule`] — the `ExternalPass` the engine calls once per frame ahead
//!   of the scene, and [`FxCadence`], the vocabulary a component asks for its
//!   next repaint with.
//!
//! # The shape a component uses it in
//!
//! ```text
//! // once, on first paint — and again on a later paint if it answered None
//! self.fx = GpuFx::try_acquire("tilt-card");
//!
//! // every paint with 3D content
//! if let Some(fx) = &self.fx {
//!     fx.submit((width, height), Quad3dScene::new().with(face));
//!     schedule::request_frame(ctx, FxCadence::Animating);
//!     // ...and record the id over the destination rectangle (see below)
//! } else {
//!     self.paint_flat(ctx, scene);
//! }
//!
//! // in View::teardown
//! if let Some(fx) = self.fx.take() {
//!     fx.release();
//! }
//! ```
//!
//! # What it still cannot do
//!
//! - **Arbitrary child subtrees are never perspective-transformed.** This
//!   substrate tilts *faces it renders itself*. A component wanting its own
//!   children on a tilted face has to draw their content into a face, and the
//!   only face content reachable from this crate is a colour, a gradient, or
//!   a texture the caller produced with `wgpu` itself.
//! - **Widget-painted content cannot become a face today.** The engine can
//!   render a display list into an offscreen texture, but the widget-facing
//!   paint trait exposes no route to one — see the *Gap* note below. Until
//!   that exists, a 3D face carries no text and no child widgets.
//! - **The composite is always blended, never opaque**
//!   (`docs/LIMITATIONS.md`'s `engine-scene-texture-always-blended`), so a
//!   fully opaque face still pays the blend and never occludes 2D content
//!   behind it through depth.
//! - **Only the desktop shell installs a device**
//!   (`docs/LIMITATIONS.md`'s `facade-gpu-context-desktop-only`), so
//!   acquisition answers `None` on Android and iOS today and every 3D
//!   component renders flat there.
//!
//! # Gap: recording the rendered texture from a widget
//!
//! The seam has two reachable halves and one missing one. A plugin can reach
//! the device (`frust::gpu::with_context`) and can render and register a
//! texture ahead of the frame's scene encode (`ExternalPass` +
//! `ExternalFrame::bind_texture`) — both are what this module is built on.
//! What a widget cannot do is *name* that texture in its own paint: a widget
//! paints through `&mut dyn PaintScene`, and that trait has a `draw_shader`
//! command but no scene-texture command, while `SceneBuilder::scene_texture`
//! (the function the facade's own documentation points at) is reachable only
//! from code holding a `SceneBuilder` — which a widget never does.
//!
//! Everything in this module is therefore complete and exercised on its own
//! terms, and the rendered target is bound under a real id, but nothing in
//! this catalog can draw that id until the paint trait carries the command.
//! Closing it is a framework change of one defaulted trait method plus its
//! forwarding implementation, not a redesign; it is recorded here so the
//! variants built on this substrate inherit the constraint explicitly rather
//! than discovering it.

pub mod context;
pub mod pool;
pub mod quad3d;
pub mod schedule;

// Later 3D variant modules attach here as their own `pub mod` lines
// (card/cylinder/fan geometry, each owning its component-facing view). This
// module deliberately declares none of them ahead of time: a `pub mod` line
// without a file behind it does not compile, and an empty file declared here
// would be an unused module rather than a reservation.

pub use context::{FxAvailability, GpuFx, GpuFxHandle, KILL_SWITCH_ENV_VAR};
pub use pool::{
    Binding, FxComponentId, FxTarget, MAX_TARGET_SIDE, MAX_UNSEEN_FRAMES, TARGET_QUANTUM,
    TargetKey, TargetPool,
};
pub use quad3d::{
    CAMERA_DISTANCE, COLOR_FORMAT, DEPTH_FORMAT, FAR, FOV_Y_RADIANS, NEAR, Quad3d, Quad3dRenderer,
    Quad3dScene, Quad3dTarget, QuadFace,
};
pub use schedule::{DECORATIVE_FRAME_INTERVAL, FxCadence, FxPass, request_frame};

/// The shared harness the `#[ignore]`d GPU cases in this module tree run on.
///
/// They are ignored by default because they need a real adapter, and they read
/// pixels back because nothing they check produces a *wrong* value when it is
/// wrong — it produces an untouched target or a validation error, which only a
/// read-back tells apart from success:
///
/// ```text
/// WGPU_BACKEND=vulkan WGPU_ADAPTER_NAME=T400 FRUST_GOLDEN_EXPECT_ADAPTER=T400 \
///   cargo test -p frust-beui --features gpu-effects -- --ignored --nocapture
/// ```
///
/// The device comes from the facade's own `frust::gpu::Context` — the
/// documented standalone/headless route — rather than from a second GPU
/// dependency of this crate's own, which keeps the production charter line
/// (`frust` + kurbo + peniko) intact for the test build too.
#[cfg(test)]
pub(crate) mod test_gpu {
    use std::sync::{Mutex, MutexGuard, PoisonError};

    /// The provenance variable every GPU run in this workspace records under
    /// (`docs/TESTING.md`'s GPU Run Metadata), named here as a literal because
    /// the facade does not re-export the renderer's own constant.
    const GOLDEN_EXPECT_ADAPTER_ENV_VAR: &str = "FRUST_GOLDEN_EXPECT_ADAPTER";

    /// Serialises every GPU case in this binary: each builds its own
    /// `wgpu::Device`, and a driver that serialises device teardown on a
    /// process-global mutex deadlocks when two of them tear down at once.
    /// Poison is ignored — one failing case must not cascade into its
    /// siblings.
    static SERIAL: Mutex<()> = Mutex::new(());

    fn serial() -> MutexGuard<'static, ()> {
        SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Polls `future` to completion — wgpu's native adapter and device
    /// requests resolve without an executor driving them, and this crate has
    /// no async runtime.
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

    /// Runs `case` against a real device, naming the adapter it resolved and
    /// refusing a run on an adapter other than the one the caller pinned.
    ///
    /// Asserts on exit that the device raised no uncaptured error, so a case
    /// whose assertions happen to pass over a validation failure still fails.
    pub(crate) fn with_device(case: impl FnOnce(&wgpu::Device, &wgpu::Queue)) {
        let _serial = serial();
        let mut context = frust::gpu::Context::new();
        let handle = block_on(context.device()).expect("a GPU adapter and device");
        let info = handle.adapter.get_info();
        println!("frust-beui gpu_fx adapter: {info:?}");
        if let Ok(expected) = std::env::var(GOLDEN_EXPECT_ADAPTER_ENV_VAR) {
            let expected = expected.trim();
            assert!(
                expected.is_empty() || info.name.to_lowercase().contains(&expected.to_lowercase()),
                "{GOLDEN_EXPECT_ADAPTER_ENV_VAR}={expected:?} but the run resolved {:?}",
                info.name
            );
        }
        case(&handle.device, &handle.queue);
        let _ = handle.device.poll(wgpu::PollType::wait_indefinitely());
        assert_eq!(
            handle.first_uncaptured_error(),
            None,
            "the device raised an uncaptured error during the case"
        );
    }

    /// Reads `texture` back as tightly packed RGBA8 rows.
    pub(crate) fn read_back(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        texture: &wgpu::Texture,
        width: u32,
        height: u32,
    ) -> Vec<u8> {
        let unpadded = width * 4;
        let bytes_per_row = unpadded.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frust-beui gpu_fx readback"),
            size: u64::from(bytes_per_row) * u64::from(height),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("frust-beui gpu_fx readback copy"),
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        queue.submit([encoder.finish()]);

        let slice = buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("the device poll driving the read-back map succeeds");
        rx.recv()
            .expect("the read-back map channel stays open")
            .expect("the read-back buffer maps");
        let mapped = slice
            .get_mapped_range()
            .expect("the read-back buffer is mapped");
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for row in 0..height {
            let start = (row * bytes_per_row) as usize;
            pixels.extend_from_slice(&mapped[start..start + unpadded as usize]);
        }
        drop(mapped);
        buffer.unmap();
        pixels
    }
}
