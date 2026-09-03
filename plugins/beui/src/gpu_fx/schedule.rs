//! The per-frame hook: the `ExternalPass` a 3D component's work is recorded
//! through, and the frame-cadence vocabulary a component asks for its next
//! repaint with.
//!
//! # Where a 3D pass sits in a frame
//!
//! The engine tier drains the process-wide external-pass registry once per
//! frame, *before* it records anything of the scene, into the very
//! `wgpu::CommandEncoder` the scene is then recorded into and the renderer
//! submits once. That single-encoder ordering is what makes this work with no
//! second submit and no fence: the texture a pass binds during `record` is
//! already registered when the display list naming it is compiled, and the
//! pass that wrote it is already recorded ahead of the pass that samples it.
//!
//! [`FxPass`] is this crate's implementation of that seam, and it holds the
//! whole per-component GPU state: the compiled [`Quad3dRenderer`], the
//! [`TargetPool`], the [`Binding`] tracking what is registered under the
//! pass's own `SceneTextureId`, and the scene the component's last paint
//! handed over.
//!
//! # The paint/record split
//!
//! A component's `paint` runs on the UI thread; `record` runs on the render
//! thread, and — with two engine-tier surfaces live at once — can run on two
//! threads at the same moment (`docs/LIMITATIONS.md`'s
//! `external-pass-registry-process-wide`). So paint never touches the GPU: it
//! calls [`FxPass::submit`] with plain data, and `record` picks that data up
//! under a mutex. The last submitted scene is *kept*, not consumed, because a
//! frame can be drained without a repaint having happened — a component that
//! painted once and went still keeps rendering the same faces rather than
//! blinking out.
//!
//! # Rules this pass is under
//!
//! - **It renders into its own target, never the frame's.** The engine clears
//!   the frame's colour attachment when it records the scene, so anything a
//!   pass wrote there would be gone.
//! - **It never shares the frame's depth attachment.** `ExternalFrame` hands
//!   out no depth view; a scene wanting occlusion gets a depth attachment on
//!   its *own* pooled target.
//! - **It never submits**, and it leaves the shared encoder finishable: the
//!   render pass is opened and closed inside [`Quad3dRenderer::record`], and
//!   no debug group is pushed.
//! - **It binds only what is genuinely rendered.** `bind_texture` writes the
//!   engine's registry immediately and survives a frame the engine later
//!   refuses (`docs/LIMITATIONS.md`'s
//!   `external-pass-bind-not-transactional-with-frame`), so the bind happens
//!   after the draw is recorded, never before there is anything to sample.
//! - **It must not panic.** Panic isolation only exists in a build that
//!   unwinds (`docs/LIMITATIONS.md`'s
//!   `external-pass-panic-isolation-dev-only`), so every path here is total.

use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use frust::authoring::PaintCtx;
use frust::gpu::{ExternalFrame, ExternalPass, SceneTextureId};

use super::pool::{Binding, FxComponentId, TargetKey, TargetPool, clamp_requested};
use super::quad3d::{Quad3dRenderer, Quad3dScene, Quad3dTarget};

/// How often a purely decorative 3D loop repaints under
/// [`FxCadence::Decorative`] — 20fps, the same paced cadence this catalog's
/// other perpetual effects run at, since a background flourish has no reason
/// to hold the display at vsync.
pub const DECORATIVE_FRAME_INTERVAL: Duration = Duration::from_millis(50);

/// What a component wants the frame loop to do after this paint.
///
/// Registering a pass does not by itself keep frames coming — a pass is
/// called once per frame the app actually renders, and nothing more. A
/// component whose 3D content is changing has to ask, exactly as any other
/// animating widget does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FxCadence {
    /// The content is at rest: ask for nothing, and let the surface go idle.
    Still,
    /// The content is mid-interaction or mid-animation: ask for the next
    /// vsync.
    Animating,
    /// The content is a perpetual decorative loop: ask for a *paced* frame at
    /// [`DECORATIVE_FRAME_INTERVAL`] rather than every vsync.
    Decorative,
}

/// Asks the frame loop for whatever `cadence` describes.
///
/// The one place a 3D component's repaint request is expressed, so the
/// still/animating/decorative distinction is made once rather than
/// re-decided per component.
pub fn request_frame(ctx: &mut PaintCtx<'_>, cadence: FxCadence) {
    match cadence {
        FxCadence::Still => {}
        FxCadence::Animating => ctx.request_frame(),
        FxCadence::Decorative => ctx.request_frame_paced_at(DECORATIVE_FRAME_INTERVAL),
    }
}

/// Everything one component's pass owns, behind the mutex that serialises the
/// concurrent `record` two live surfaces can produce.
#[derive(Default)]
struct PassState {
    /// The last scene a paint handed over, and the extent it wants rendered.
    pending: Option<(Quad3dScene, (u32, u32))>,
    /// Built on the first frame that has a device, then reused.
    renderer: Option<Quad3dRenderer>,
    pool: TargetPool,
    binding: Binding,
}

/// One component's registered pass.
///
/// Created and registered by [`crate::gpu_fx::GpuFx::try_acquire`], driven by
/// the engine every frame, and withdrawn by
/// [`crate::gpu_fx::GpuFxHandle::release`].
pub struct FxPass {
    id: SceneTextureId,
    component: FxComponentId,
    label: String,
    state: Mutex<PassState>,
}

impl FxPass {
    /// A pass with nothing to draw yet.
    #[must_use]
    pub fn new(id: SceneTextureId, component: FxComponentId, label: &str) -> Self {
        Self {
            id,
            component,
            label: label.to_owned(),
            state: Mutex::new(PassState::default()),
        }
    }

    /// The `SceneTextureId` this pass binds under — the id a display list
    /// names to composite its output.
    #[must_use]
    pub const fn id(&self) -> SceneTextureId {
        self.id
    }

    /// The component identity its pooled targets are keyed by.
    #[must_use]
    pub const fn component(&self) -> FxComponentId {
        self.component
    }

    /// Hands this frame's content over from paint.
    ///
    /// `extent` is the destination's size in device texels — the size the
    /// target is rendered at and the extent the engine maps the destination
    /// rectangle onto. Called on the UI thread; touches no GPU resource.
    pub fn submit(&self, extent: (u32, u32), scene: Quad3dScene) {
        let extent = (clamp_requested(extent.0), clamp_requested(extent.1));
        self.lock().pending = Some((scene, extent));
    }

    /// Withdraws the content, so the next frame renders and binds nothing.
    ///
    /// What a component calls when it falls back to its 2D path mid-life
    /// (reduced motion turning on, a tilt settling to rest) without giving up
    /// its registration.
    pub fn clear(&self) {
        self.lock().pending = None;
    }

    /// Whether a scene is waiting to be rendered.
    #[must_use]
    pub fn has_content(&self) -> bool {
        self.lock().pending.is_some()
    }

    /// How many pooled targets this pass is holding — the reaping window made
    /// observable.
    #[must_use]
    pub fn resident_targets(&self) -> usize {
        self.lock().pool.len()
    }

    /// The state, with poison ignored: one frame's panic must not turn every
    /// later frame into a panic of its own, which is the opposite of what the
    /// engine's `catch_unwind` around `record` is for.
    fn lock(&self) -> std::sync::MutexGuard<'_, PassState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl ExternalPass for FxPass {
    fn record(&self, frame: &mut ExternalFrame<'_>) {
        let mut guard = self.lock();
        // Split into disjoint field borrows up front: the scene is read while
        // the pool and the renderer are mutated, which one whole-struct
        // borrow would not allow and a clone of the scene would pay for every
        // frame.
        let PassState {
            pending,
            renderer,
            pool,
            binding,
        } = &mut *guard;
        pool.begin_frame(frame.frame_index());

        let Some((scene, extent)) = pending.as_ref().filter(|(scene, _)| !scene.is_empty()) else {
            withdraw(binding, pool, frame);
            return;
        };
        let extent = *extent;

        // Cloned rather than borrowed: `device()`/`queue()` borrow the frame
        // immutably and `encoder()` borrows it mutably, so the three cannot
        // be held at once. Both are `Arc`-backed handles; these clones live
        // and die inside this call, which is what the "never stash the
        // frame's device" rule is about.
        let device = frame.device().clone();
        let queue = frame.queue().clone();

        let key = TargetKey::new(self.component, extent.0, extent.1);
        let Some((color, depth, generation)) = pool
            .acquire(&device, key, scene.depth, &self.label)
            .map(|target| {
                (
                    target.color().clone(),
                    target.depth().cloned(),
                    target.generation(),
                )
            })
        else {
            withdraw(binding, pool, frame);
            return;
        };

        renderer
            .get_or_insert_with(|| Quad3dRenderer::new(&device, &self.label))
            .record(
                &device,
                &queue,
                frame.encoder(),
                Quad3dTarget {
                    color: &color,
                    depth: depth.as_ref(),
                    requested: extent,
                },
                scene,
            );

        if binding.needs_rebind(extent, generation) {
            frame.bind_texture(extent, color);
            binding.record_bind(extent, generation);
        }

        // Reaping runs after the frame's own key has been marked seen, so
        // this can only reclaim sizes the component has moved away from.
        // Losing the bound key to a reap would leave the engine sampling a
        // texture nothing writes, so that case unbinds too.
        if pool.reap().contains(&key) && binding.take() {
            frame.unbind_texture();
        }
    }
}

/// Drops whatever a pass had registered and ages its pool by one frame — the
/// path for a frame with nothing to draw.
fn withdraw(binding: &mut Binding, pool: &mut TargetPool, frame: &mut ExternalFrame<'_>) {
    if binding.take() {
        frame.unbind_texture();
    }
    pool.reap();
}

impl std::fmt::Debug for FxPass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FxPass")
            .field("id", &self.id.get())
            .field("component", &self.component.get())
            .field("label", &self.label)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use frust::FrameTime;
    use frust::authoring::{PaintCtx, Point, Rect, Size, TickClass};
    use frust::gpu::SceneTextureId;
    use peniko::Color;

    use super::{DECORATIVE_FRAME_INTERVAL, FxCadence, FxPass, request_frame};
    use crate::gpu_fx::pool::FxComponentId;
    use crate::gpu_fx::quad3d::{Quad3d, Quad3dScene, QuadFace};

    fn pass() -> FxPass {
        FxPass::new(SceneTextureId::mint(), FxComponentId::mint(), "test")
    }

    fn scene() -> Quad3dScene {
        Quad3dScene::new().with(Quad3d::new(
            Rect::new(0.0, 0.0, 32.0, 32.0),
            QuadFace::Solid(Color::WHITE),
        ))
    }

    fn ctx() -> PaintCtx<'static> {
        PaintCtx::for_test(Point::ZERO, Size::new(64.0, 64.0), FrameTime::ZERO)
    }

    #[test]
    fn a_fresh_pass_holds_nothing() {
        let pass = pass();
        assert!(!pass.has_content());
        assert_eq!(pass.resident_targets(), 0);
    }

    #[test]
    fn a_submitted_scene_is_held_until_it_is_cleared() {
        let pass = pass();
        pass.submit((32, 32), scene());
        assert!(pass.has_content());
        pass.clear();
        assert!(!pass.has_content());
    }

    /// The last scene is kept, not consumed: a frame can drain without a
    /// repaint having happened, and the component must not blink out.
    #[test]
    fn a_submitted_scene_survives_repeated_reads() {
        let pass = pass();
        pass.submit((32, 32), scene());
        assert!(pass.has_content());
        assert!(pass.has_content());
    }

    #[test]
    fn a_pass_reports_its_own_identity() {
        let id = SceneTextureId::mint();
        let component = FxComponentId::mint();
        let pass = FxPass::new(id, component, "identity");
        assert_eq!(pass.id(), id);
        assert_eq!(pass.component(), component);
        assert!(format!("{pass:?}").contains("identity"));
    }

    #[test]
    fn a_still_component_asks_for_no_frame() {
        let mut ctx = ctx();
        request_frame(&mut ctx, FxCadence::Still);
        assert!(!ctx.needs_frame());
    }

    #[test]
    fn an_animating_component_asks_for_the_next_vsync() {
        let mut ctx = ctx();
        request_frame(&mut ctx, FxCadence::Animating);
        assert!(ctx.needs_frame());
        assert!(!ctx.needs_frame_paced_only());
    }

    #[test]
    fn a_decorative_component_asks_for_a_paced_frame() {
        let mut ctx = ctx();
        request_frame(&mut ctx, FxCadence::Decorative);
        assert!(ctx.needs_frame());
        assert!(ctx.needs_frame_paced_only());
        assert_eq!(ctx.paced_interval(), Some(DECORATIVE_FRAME_INTERVAL));
        assert_eq!(ctx.frame_class(), Some(TickClass::CosmeticLoop));
    }
}
