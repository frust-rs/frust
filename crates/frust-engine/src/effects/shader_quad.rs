//! The pre-pass behind [`Command::ShaderQuad`]: a user-supplied WGSL fragment
//! program rendered into an offscreen texture the frame then draws as a scene
//! texture.
//!
//! # The split
//!
//! Every GPU resource the effect needs — the lazily compiled per-program
//! pipeline, the per-`(program, quantized size)` target pool, the
//! fullscreen-triangle pass, the age-based reaps and the churn detector —
//! lives one layer down in [`frust_gpu::effects`], which speaks `(id, wgsl,
//! size, time)` primitives and knows nothing about scenes. What lives here is
//! the half that reads a display list: which programs a frame draws, how big
//! a target each one may ask for, and the id its rendered result is
//! registered under.
//!
//! # How a quad reaches the screen
//!
//! [`ShaderQuadPass::prepare`] runs once per frame, before the frame's own
//! passes are recorded and into the *same* [`wgpu::CommandEncoder`], so the
//! effect and the frame that samples it can never be submitted apart:
//!
//! 1. walk the display list for its [`Command::ShaderQuad`]s
//!    ([`frame_demands`]), collapsing every quad of one program into a single
//!    demand and dropping one whose device rectangle does not intersect the
//!    frame's own target at all, when that extent is known (see
//!    [`ShaderQuadPass::set_target_extent`]);
//! 2. compile the program if it is new, size a pooled (quantized) target for
//!    it, and record its fullscreen-triangle pass into the encoder, confined
//!    to the sub-rect of that target matching the quad's own exact device
//!    size — see `frust_gpu::effects::ShaderEffects::encode_pass`;
//! 3. register that sub-rect with the [`EngineRenderer`] under
//!    [`SceneTextureId::for_shader_program`], the id the compiler's own
//!    lowering derives from the same program.
//!
//! The draw itself is then nothing special: the compiler lowers the quad to an
//! external-texture paint over its destination rectangle, exactly as it lowers
//! [`Command::SceneTexture`](frust_scene::Command::SceneTexture) — same
//! natural-pixels-onto-destination mapping, same blended pass — reading only
//! the registered sub-rect of what may be a larger, quantized texture (see
//! `crate::compile::external`/`crate::gpu::bindings`'s offset source-region
//! support).
//!
//! # One target per program per frame
//!
//! A program drawn twice in one frame renders once, into one target sized to
//! the largest device-space extent any of its quads asks for, and both quads
//! sample it. That is what lets the id a target is registered under be a pure
//! function of the program id: the compiler's walk holds only
//! `ShaderProgram::id`, so any keying that also involved a size would need the
//! two halves to agree on a floating-point extent computed twice. The cost is
//! that the smaller of two quads samples a larger target — a resample, not a
//! wrong pixel. The `time` the program renders at is the one its first quad in
//! painter order carries.
//!
//! # Target stability under resize
//!
//! A quad's `dest` changing by a fraction of a device pixel every frame — a
//! window resize drag, an animated scale, a spring layout — used to mint a
//! fresh GPU texture, view, uniform buffer and bind group on
//! [`frust_gpu::effects::ShaderEffects`] every single such frame, immediately
//! evicting the previous one. Two changes fix that, both living in
//! [`frust_gpu::effects`] and reused here rather than reinvented:
//!
//! - **Quantization**: a target's true size is rounded up to the next 256px
//!   quantum before it becomes a key
//!   (`frust_gpu::effects::quantized_target_key`, reusing
//!   `frust_gpu::pool`'s own quantum), so a resize drag mints a new texture
//!   only when it crosses a 256px boundary. The pass still renders — and the
//!   shader still sees `frust_u.resolution` as — the quad's own *exact*
//!   requested size, confined to that sub-rect of the (possibly larger)
//!   texture via a `wgpu` viewport; the target's true extent is never handed
//!   to the shader or to the frame's own paint mapping.
//! - **Age-based target reap**: a size a still-drawn program has resized away
//!   from is no longer reclaimed the instant it is not asked for (the old
//!   frame-scoped eviction, which actively fought quantization — two nearby
//!   requests sharing one quantized texture would otherwise evict each other
//!   every frame); it ages out over its own window, independent of the
//!   program id's own (wider) unseen-frame window.
//!
//! Registration itself ([`ShaderQuadPass::register`]) still only re-binds the
//! renderer's own external-texture slot — the strip pipeline's group-1 bind
//! group — when a program's *demanded* extent actually changes from the
//! previous frame, which is every frame for a quad genuinely resizing but
//! none for one holding steady; what quantization removes is the GPU-resource
//! churn one layer down, which used to happen on every such frame regardless.
//!
//! # Alpha
//!
//! A target is `Rgba8Unorm` and the pass writes the fragment shader's return
//! value into it unblended, so the target holds exactly what the shader
//! returned. It is then sampled as **premultiplied** colour, the convention
//! every paint in an engine frame travels in: a shader returning `vec4(rgb, a)`
//! must have already multiplied `rgb` by `a`, and the result composites over
//! what is behind it rather than replacing it. Opaque output (`a = 1.0`) is
//! unaffected by the convention, which is why shaders written against the
//! earlier opaque-only rule keep rendering identically.
//!
//! # Kill switch
//!
//! `FRUST_ENGINE_NO_SHADER_EFFECTS=1` ([`crate::config::shader_effects_disabled`])
//! turns the whole path off: nothing is compiled, no target is allocated, every
//! registration is dropped, and each quad draws nothing while the compiler
//! reports the skip once. The escape hatch for a driver that miscompiles a user
//! program, where the alternative is losing the application rather than one
//! effect.
//!
//! # No panics
//!
//! Every path here upholds the FFI no-panic invariant: a shader that fails to
//! compile is recorded and skipped by [`ShaderEffects`] with a rate-limited
//! warning, and a quad whose target could not be built simply draws nothing.

use std::collections::{HashMap, HashSet};

use frust_gpu::effects::quantized_target_key;
use frust_gpu::{SceneTextureId, ShaderEffects};
use frust_scene::{Command, Scene, ShaderProgram};
use kurbo::{Affine, Rect};

use crate::config;
use crate::gpu::atlas::x_y_advances;
use crate::renderer::EngineRenderer;

/// The conservative upper bound on a shader-effect target's own dimensions,
/// applied on top of the adapter's `max_texture_dimension_2d` by
/// [`clamp_size`].
///
/// A policy cap rather than a device limit: a quad this large is a footgun on
/// any adapter — tens of megabytes of offscreen target re-rendered every frame
/// — long before the device refuses it. The device's own ceiling is enforced
/// independently one layer down (`ShaderEffects::ensure_target`), so an
/// oversized request is bounded twice rather than trusted once.
pub const MAX_TEXTURE_DIM: u32 = 8192;

/// One program's whole demand on a frame: which program, how large a target it
/// wants, and the time to render it at.
///
/// Collapsed across every quad the frame draws that program with — see the
/// module header's one-target-per-program rule.
#[derive(Debug, Clone, Copy)]
pub struct QuadDemand<'a> {
    /// The program to render, borrowed from the display list.
    pub program: &'a ShaderProgram,
    /// The clamped target extent, in texels.
    pub size: (u32, u32),
    /// The `time` uniform, from the program's first quad in painter order.
    pub time: f32,
}

/// Clamp a requested target size to the renderable range: at most
/// [`MAX_TEXTURE_DIM`] and the adapter's own `max_texture_dimension_2d`, and at
/// least 1 per axis (a zero-sized texture is invalid). Pure — no GPU state
/// touched, so the policy is testable without a device.
fn clamp_size(requested: (u32, u32), adapter_max: u32) -> (u32, u32) {
    let cap = MAX_TEXTURE_DIM.min(adapter_max);
    let clamp = |v: u32| v.clamp(1, cap.max(1));
    (clamp(requested.0), clamp(requested.1))
}

/// The target extent a quad over `dest` under `transform` asks for: `dest`'s
/// own width/height scaled by the transform's linear magnitudes — the
/// lengths of its transformed unit axes ([`x_y_advances`]) — rounded up so a
/// fractional edge is covered rather than cropped, and clamped by
/// [`clamp_size`].
///
/// Sized from `dest`'s own dimensions rather than the device-space
/// axis-aligned bounding box a plain `transform_rect_bbox` would give: a
/// rotated or skewed quad's bbox is both larger than and a different aspect
/// from the quad itself, so a target sized from it would squash the rendered
/// content when the frame later resamples it back onto `dest` (a 100x50
/// rectangle rotated 45 degrees has a ~106x106 bbox, mapped non-uniformly —
/// (0.943, 0.472) per axis — onto the 100x50 destination it is actually drawn
/// into). The transform's own linear magnitudes are exactly the per-axis
/// scale `dest`'s W/H is stretched by on the way to device space, aspect-true
/// regardless of rotation or skew — a pure rotation leaves both magnitudes at
/// 1.0, so the target keeps `dest`'s own aspect exactly.
///
/// Rounded up rather than to nearest because the target is *resampled* onto
/// `dest`: a target a fraction of a pixel too small is a visibly softer edge,
/// while one a fraction too large costs a row of texels nothing reads.
///
/// `None` for geometry no target can be sized from — a non-finite transform or
/// rectangle — which the frame's own geometry check refuses anyway; answering
/// `None` here keeps this function honest on its own rather than relying on
/// that ordering.
fn requested_size(dest: Rect, transform: Affine, adapter_max: u32) -> Option<(u32, u32)> {
    if !dest.is_finite() || !transform.as_coeffs().iter().all(|c| c.is_finite()) {
        return None;
    }
    let (x_advance, y_advance) = x_y_advances(transform);
    let width = dest.width() * x_advance.hypot();
    let height = dest.height() * y_advance.hypot();
    if !width.is_finite() || !height.is_finite() {
        return None;
    }
    // Saturating on both ends: a huge-but-finite rectangle becomes the cap
    // rather than wrapping, and a negative or sub-texel one becomes 1.
    let axis = |extent: f64| -> u32 {
        let ceiled = extent.ceil();
        if ceiled <= 1.0 {
            1
        } else if ceiled >= f64::from(u32::MAX) {
            u32::MAX
        } else {
            ceiled as u32
        }
    };
    Some(clamp_size((axis(width), axis(height)), adapter_max))
}

/// Every fragment program `scene` draws, in painter order of first appearance,
/// each with the largest extent its quads ask for and the time its first quad
/// carries.
///
/// Pure over the display list and `root` — no GPU state, no device — so the
/// frame's whole demand can be asserted without a queue. `root` is the frame
/// transform the renderer applies ahead of each command's own, so a quad's
/// device extent here is the one the frame will actually draw it at.
///
/// A snapshot bracket's presentation scale is deliberately not folded in: it is
/// applied to a body's *rendered* pixels, so a quad inside one is rendered at
/// its own device size and resampled by the bracket, exactly like every other
/// command in that body.
///
/// `target_extent`, when given, culls a quad whose device-space rectangle
/// does not overlap `(0, 0)..target_extent` at all — an off-screen or fully
/// clipped quad then demands no target and is neither rendered nor
/// re-rendered every frame purely to go unseen. The check is conservative
/// (the quad's axis-aligned bounding box, not its exact rotated/skewed
/// footprint, and — via [`kurbo::Rect::overlaps`] — inclusive of a shared
/// edge), so it never culls a quad that is even partly visible. Clip-stack
/// awareness (culling a quad hidden entirely behind an unrelated clip) is
/// deliberately out of scope: the frame's own clip stack is a compile-time
/// concept this walk runs ahead of, so only whole-target intersection is
/// checked. `None` applies no culling at all — see
/// [`ShaderQuadPass::set_target_extent`] for why a caller may have nothing to
/// pass here today.
fn frame_demands(
    scene: &Scene,
    root: Affine,
    adapter_max: u32,
    target_extent: Option<(u32, u32)>,
) -> Vec<QuadDemand<'_>> {
    let mut order: Vec<u64> = Vec::new();
    let mut demands: HashMap<u64, QuadDemand<'_>> = HashMap::new();
    let target_rect = target_extent.map(|(w, h)| Rect::new(0.0, 0.0, f64::from(w), f64::from(h)));

    for command in scene.commands() {
        let Command::ShaderQuad {
            program,
            dest,
            transform,
            time,
        } = command
        else {
            continue;
        };
        let device_transform = root * *transform;
        if let Some(target_rect) = target_rect {
            let device_bbox = device_transform.transform_rect_bbox(*dest);
            if device_bbox.is_finite() && !device_bbox.overlaps(target_rect) {
                // Entirely outside the frame's own target: no target needed.
                // A non-finite bbox falls through to `requested_size` below,
                // which drops it on its own terms instead.
                continue;
            }
        }
        let Some(size) = requested_size(*dest, device_transform, adapter_max) else {
            continue;
        };
        match demands.get_mut(&program.id()) {
            // A program already demanded this frame keeps its first quad's
            // time and grows to cover the largest quad drawing it.
            Some(demand) => {
                demand.size = (demand.size.0.max(size.0), demand.size.1.max(size.1));
            }
            None => {
                order.push(program.id());
                demands.insert(
                    program.id(),
                    QuadDemand {
                        program,
                        size,
                        time: *time,
                    },
                );
            }
        }
    }

    order
        .into_iter()
        .filter_map(|id| demands.remove(&id))
        .collect()
}

/// Renders a frame's [`Command::ShaderQuad`]s into pooled offscreen targets and
/// registers each with the renderer that will draw it.
///
/// Owned by whoever owns the frame's [`EngineRenderer`] and the encoder it
/// records into — the two have to be the same frame's — and driven once per
/// frame through [`Self::prepare`], including frames drawing no quad at all:
/// that is the call on which a program the scene has stopped drawing ages
/// towards its reap.
pub struct ShaderQuadPass {
    /// Every GPU resource the effect owns: pipelines, targets, the reap clock.
    effects: ShaderEffects,
    /// The extent each program's target is currently registered with the
    /// renderer at, keyed by program id.
    ///
    /// What makes registration incremental: a program whose *demanded* extent
    /// is unchanged frame to frame is left bound, so the frame's bind groups
    /// naming it survive instead of being dropped and rebuilt every frame —
    /// the steady-state (by far most common) case for a quad that is not
    /// actively resizing. An entry here is always matched by a live
    /// registration on the renderer, which is what lets a reap unbind exactly
    /// what it reaped.
    registered: HashMap<u64, (u32, u32)>,
    /// The frame's own render-target extent, in device pixels — see
    /// [`Self::set_target_extent`].
    target_extent: Option<(u32, u32)>,
}

impl ShaderQuadPass {
    /// An empty pass seeded with an optional clone of the surface's
    /// [`wgpu::PipelineCache`], so a persisted cache speeds a user program's
    /// first compilation exactly as it speeds the engine's own pipelines.
    #[must_use]
    pub fn new(pipeline_cache: Option<wgpu::PipelineCache>) -> Self {
        Self {
            effects: ShaderEffects::new(pipeline_cache),
            registered: HashMap::new(),
            target_extent: None,
        }
    }

    /// How many programs currently have a target registered with a renderer.
    #[must_use]
    pub fn registered_len(&self) -> usize {
        self.registered.len()
    }

    /// Sets the frame's own render-target extent, in device pixels, so the
    /// next [`Self::prepare`] call can cull a quad whose device-space
    /// rectangle does not intersect it at all (see [`frame_demands`]'s doc
    /// comment for the exact rule). `None` (the default a fresh
    /// [`Self::new`] starts with) applies no culling.
    ///
    /// Opt-in rather than inferred: `prepare`'s own signature is fixed by its
    /// external caller (`frust-render`'s `SurfaceRenderer`, which records the
    /// pre-pass ahead of `EngineRenderer::encode` — see the module header),
    /// and nothing reachable from `prepare`'s existing parameters names the
    /// frame's target extent today (`EngineRenderer` learns it only when
    /// `encode` itself is called, afterward). Adding a required parameter to
    /// `prepare` to carry it through would break that caller — out of scope
    /// here — so a caller that knows its target extent ahead of time calls
    /// this setter first instead. Wiring `frust-render`'s own call site to do
    /// so is a follow-up outside this module.
    pub fn set_target_extent(&mut self, extent: Option<(u32, u32)>) {
        self.target_extent = extent;
    }

    /// Renders every fragment program `scene` draws into its own pooled target,
    /// recording each pass into `encoder`, and registers the results with
    /// `engine` so the frame compiled after this call draws them.
    ///
    /// Call once per frame, before `engine`'s own encode and with that frame's
    /// encoder, `scene` and `root`. Nothing is submitted: the passes recorded
    /// here precede the frame's in the same command buffer, which is the whole
    /// ordering guarantee the effect needs.
    ///
    /// A program that fails to compile, or whose target could not be built, is
    /// unregistered rather than left pointing at a stale texture — its quads
    /// draw nothing, and the compiler says so once. When the kill switch is set
    /// the same unregistration happens for every program and no GPU work is
    /// done at all.
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        scene: &Scene,
        root: Affine,
        engine: &mut EngineRenderer,
    ) {
        if config::shader_effects_disabled() {
            self.unregister_all(engine);
            return;
        }

        let adapter_max = device.limits().max_texture_dimension_2d;
        let demands = frame_demands(scene, root, adapter_max, self.target_extent);
        let mut live_ids: HashSet<u64> = HashSet::with_capacity(demands.len());
        let mut live_keys: HashSet<(u64, u32, u32)> = HashSet::with_capacity(demands.len());

        for demand in demands {
            let id = demand.program.id();
            let (w, h) = demand.size;
            let quantized = quantized_target_key(w, h, adapter_max);
            live_ids.insert(id);
            live_keys.insert((id, quantized.0, quantized.1));

            self.effects
                .ensure_pipeline(device, id, demand.program.source());
            self.effects.ensure_target(device, id, w, h);
            self.effects
                .encode_pass(encoder, queue, device, id, (w, h), demand.time);
            self.register(engine, device, id, (w, h));
        }

        // The one clock tick per frame — including a frame drawing no quad at
        // all — that ages both a program id absent from `live_ids` towards
        // its whole-program reap and a target key absent from `live_keys`
        // towards its own, shorter, target-level reap (see
        // `frust_gpu::effects::ShaderEffects::mark_seen`). Only the
        // whole-program reap has registrations to withdraw: a target-level
        // reap frees GPU memory for a size this id has resized away from
        // without touching what is currently registered.
        let stale = self.effects.mark_seen(&live_ids, &live_keys);
        self.effects.reap(&stale);
        for id in stale {
            self.unregister(engine, id);
        }
    }

    /// Registers program `id`'s `(w, h)`-requested target with `engine`, or
    /// withdraws any earlier registration when there is no such target to
    /// register.
    ///
    /// A no-op when the program is already registered at the extent its
    /// demand resolved to this call, which is the common (steady-state) case
    /// on every frame a quad is not actively resizing: re-registering would
    /// drop the frame's bind groups naming the view and rebuild them for an
    /// identical binding. The registered extent is the *requested* sub-rect
    /// (see `frust_gpu::effects::ShaderEffects::target_extent`), never the
    /// quantized target's own larger size, so the compiler still maps `dest`
    /// onto exactly the quad's own device pixels — a quad resizing pixel by
    /// pixel still re-registers every such frame (its demand genuinely
    /// changes), but the expensive part — a fresh GPU texture, view, uniform
    /// buffer and bind group — no longer does, since quantization keeps the
    /// underlying target the same across an entire 256px band.
    fn register(
        &mut self,
        engine: &mut EngineRenderer,
        device: &wgpu::Device,
        id: u64,
        size: (u32, u32),
    ) {
        let (w, h) = size;
        let Some((extent, view)) = self
            .effects
            .target_extent(device, id, w, h)
            .zip(self.effects.target_view(device, id, w, h).cloned())
        else {
            // No target: the program failed to compile, or its size was
            // refused. Either way it must stop drawing whatever it drew last.
            self.unregister(engine, id);
            return;
        };

        if self.registered.get(&id) == Some(&extent) {
            return;
        }
        engine.bind_texture(SceneTextureId::for_shader_program(id), extent, view);
        self.registered.insert(id, extent);
    }

    /// Withdraws program `id`'s registration, if it has one.
    fn unregister(&mut self, engine: &mut EngineRenderer, id: u64) {
        if self.registered.remove(&id).is_some() {
            engine.unbind_texture(SceneTextureId::for_shader_program(id));
        }
    }

    /// Withdraws every registration this pass made — the kill switch's path,
    /// and the one that makes turning the switch on mid-process take effect on
    /// the next frame rather than leaving stale targets bound.
    fn unregister_all(&mut self, engine: &mut EngineRenderer) {
        for id in self.registered.drain().map(|(id, _)| id) {
            engine.unbind_texture(SceneTextureId::for_shader_program(id));
        }
    }
}

impl std::fmt::Debug for ShaderQuadPass {
    /// `ShaderEffects` owns `wgpu` handles that do not print usefully, so the
    /// pass reports the only state a reader can act on: how many programs are
    /// registered.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShaderQuadPass")
            .field("registered", &self.registered.len())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_scene::SceneBuilder;

    /// A trivial but well-formed fragment source: the demand walk never
    /// compiles it, so only its identity matters.
    const SOURCE: &str = "@fragment fn fs_main(in: FrustVsOut) -> @location(0) vec4<f32> \
                          { return vec4<f32>(1.0, 0.0, 1.0, 1.0); }";

    fn scene_of(record: impl FnOnce(&mut SceneBuilder<'_>)) -> Scene {
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        record(&mut builder);
        scene
    }

    #[test]
    fn clamp_size_caps_at_the_policy_bound() {
        assert_eq!(clamp_size((10_000, 10_000), u32::MAX), (8192, 8192));
    }

    #[test]
    fn clamp_size_respects_an_adapter_max_below_the_policy_bound() {
        assert_eq!(clamp_size((6000, 6000), 4096), (4096, 4096));
    }

    #[test]
    fn clamp_size_floors_zero_to_one() {
        assert_eq!(clamp_size((0, 0), 8192), (1, 1));
        assert_eq!(clamp_size((0, 512), 8192), (1, 512));
    }

    #[test]
    fn clamp_size_passes_an_in_range_request_through() {
        assert_eq!(clamp_size((1290, 2796), 16384), (1290, 2796));
    }

    #[test]
    fn clamp_size_survives_a_degenerate_adapter_max() {
        // A zero ceiling must still never produce a zero dimension.
        assert_eq!(clamp_size((100, 100), 0), (1, 1));
    }

    #[test]
    fn a_requested_size_is_the_destination_in_device_space() {
        assert_eq!(
            requested_size(Rect::new(0.0, 0.0, 40.0, 20.0), Affine::IDENTITY, 8192),
            Some((40, 20))
        );
        // The frame's own scale is part of the device extent.
        assert_eq!(
            requested_size(Rect::new(0.0, 0.0, 40.0, 20.0), Affine::scale(2.0), 8192),
            Some((80, 40))
        );
    }

    #[test]
    fn a_requested_size_rounds_a_fractional_extent_up() {
        assert_eq!(
            requested_size(Rect::new(0.0, 0.0, 40.5, 20.25), Affine::IDENTITY, 8192),
            Some((41, 21))
        );
    }

    #[test]
    fn a_requested_size_is_never_zero() {
        assert_eq!(
            requested_size(Rect::new(4.0, 4.0, 4.0, 4.0), Affine::IDENTITY, 8192),
            Some((1, 1))
        );
    }

    #[test]
    fn a_requested_size_is_clamped_rather_than_wrapped() {
        assert_eq!(
            requested_size(Rect::new(0.0, 0.0, 1e12, 1e12), Affine::IDENTITY, 8192),
            Some((8192, 8192))
        );
    }

    #[test]
    fn non_finite_geometry_asks_for_no_target() {
        assert_eq!(
            requested_size(Rect::new(0.0, 0.0, f64::NAN, 8.0), Affine::IDENTITY, 8192),
            None
        );
        assert_eq!(
            requested_size(
                Rect::new(0.0, 0.0, 8.0, 8.0),
                Affine::translate((f64::INFINITY, 0.0)),
                8192
            ),
            None
        );
    }

    #[test]
    fn a_rotated_quad_sizes_from_dest_dimensions_not_the_axis_aligned_bbox() {
        // A pure rotation has unit-magnitude axes, so the aspect-true target
        // keeps dest's own 100x50 exactly — the axis-aligned bbox of a 100x50
        // rectangle rotated 45 degrees is instead ~106x106, which would
        // squash the rendered content non-uniformly when resampled back onto
        // the (still 100x50) destination.
        let dest = Rect::new(0.0, 0.0, 100.0, 50.0);
        let transform = Affine::rotate(std::f64::consts::FRAC_PI_4);

        assert_eq!(requested_size(dest, transform, 8192), Some((100, 50)));
    }

    #[test]
    fn a_scaled_and_rotated_quad_sizes_aspect_true_too() {
        // The transform's linear magnitude folds in uniform scale the same
        // way plain `Affine::scale` already did before this fix; rotation on
        // top of it changes nothing about the sizing, only the bbox.
        let dest = Rect::new(0.0, 0.0, 100.0, 50.0);
        let transform = Affine::rotate(std::f64::consts::FRAC_PI_4) * Affine::scale(2.0);

        assert_eq!(requested_size(dest, transform, 8192), Some((200, 100)));
    }

    #[test]
    fn a_scene_without_shader_quads_demands_nothing() {
        let scene = scene_of(|builder| {
            builder.fill_rect(
                Rect::new(0.0, 0.0, 8.0, 8.0),
                peniko::Brush::Solid(peniko::color::palette::css::RED),
            );
        });

        assert!(frame_demands(&scene, Affine::IDENTITY, 8192, None).is_empty());
    }

    #[test]
    fn each_program_is_demanded_once_in_painter_order() {
        let first = ShaderProgram::new(SOURCE);
        let second = ShaderProgram::new(SOURCE);
        let scene = scene_of(|builder| {
            builder.draw_shader(&first, Rect::new(0.0, 0.0, 8.0, 8.0), 0.0);
            builder.draw_shader(&second, Rect::new(0.0, 0.0, 4.0, 4.0), 0.0);
            builder.draw_shader(&first, Rect::new(0.0, 0.0, 8.0, 8.0), 0.0);
        });

        let demands = frame_demands(&scene, Affine::IDENTITY, 8192, None);

        assert_eq!(demands.len(), 2);
        assert_eq!(demands[0].program.id(), first.id());
        assert_eq!(demands[1].program.id(), second.id());
    }

    #[test]
    fn a_program_drawn_at_two_sizes_demands_the_larger_on_each_axis() {
        let program = ShaderProgram::new(SOURCE);
        let scene = scene_of(|builder| {
            builder.draw_shader(&program, Rect::new(0.0, 0.0, 40.0, 10.0), 0.0);
            builder.draw_shader(&program, Rect::new(0.0, 0.0, 10.0, 30.0), 0.0);
        });

        let demands = frame_demands(&scene, Affine::IDENTITY, 8192, None);

        assert_eq!(demands.len(), 1, "one target serves both quads");
        assert_eq!(demands[0].size, (40, 30));
    }

    #[test]
    fn a_program_renders_at_its_first_quads_time() {
        let program = ShaderProgram::new(SOURCE);
        let scene = scene_of(|builder| {
            builder.draw_shader(&program, Rect::new(0.0, 0.0, 8.0, 8.0), 1.5);
            builder.draw_shader(&program, Rect::new(0.0, 0.0, 8.0, 8.0), 9.0);
        });

        let demands = frame_demands(&scene, Affine::IDENTITY, 8192, None);

        assert_eq!(demands[0].time, 1.5);
    }

    #[test]
    fn a_quad_whose_geometry_no_target_can_be_sized_from_is_dropped() {
        let program = ShaderProgram::new(SOURCE);
        let scene = scene_of(|builder| {
            builder.draw_shader(&program, Rect::new(0.0, 0.0, f64::NAN, 8.0), 0.0);
        });

        assert!(frame_demands(&scene, Affine::IDENTITY, 8192, None).is_empty());
    }

    #[test]
    fn the_frame_transform_scales_the_demand() {
        let program = ShaderProgram::new(SOURCE);
        let scene = scene_of(|builder| {
            builder.draw_shader(&program, Rect::new(0.0, 0.0, 100.0, 50.0), 0.0);
        });

        let demands = frame_demands(&scene, Affine::scale(3.0), 8192, None);

        assert_eq!(demands[0].size, (300, 150));
    }

    #[test]
    fn a_demand_is_clamped_by_the_adapter_ceiling() {
        let program = ShaderProgram::new(SOURCE);
        let scene = scene_of(|builder| {
            builder.draw_shader(&program, Rect::new(0.0, 0.0, 6000.0, 6000.0), 0.0);
        });

        let demands = frame_demands(&scene, Affine::IDENTITY, 4096, None);

        assert_eq!(demands[0].size, (4096, 4096));
    }

    #[test]
    fn a_programs_target_id_matches_the_one_the_compiler_derives() {
        // The two halves of the seam never exchange a table — each computes
        // the id from the program alone, so they must agree by construction.
        let program = ShaderProgram::new(SOURCE);

        assert_eq!(
            SceneTextureId::for_shader_program(program.id()).get(),
            crate::compile::shader_quad_texture_id(program.id())
        );
    }

    #[test]
    fn with_no_target_extent_a_far_off_screen_quad_still_demands() {
        // `None` (a fresh `ShaderQuadPass`'s default) applies no culling at
        // all — see `ShaderQuadPass::set_target_extent`'s doc comment for why.
        let program = ShaderProgram::new(SOURCE);
        let scene = scene_of(|builder| {
            builder.draw_shader(
                &program,
                Rect::new(10_000.0, 10_000.0, 10_008.0, 10_008.0),
                0.0,
            );
        });

        assert_eq!(frame_demands(&scene, Affine::IDENTITY, 8192, None).len(), 1);
    }

    #[test]
    fn a_quad_entirely_outside_the_target_extent_demands_nothing() {
        let program = ShaderProgram::new(SOURCE);
        let scene = scene_of(|builder| {
            builder.draw_shader(
                &program,
                Rect::new(10_000.0, 10_000.0, 10_008.0, 10_008.0),
                0.0,
            );
        });

        let demands = frame_demands(&scene, Affine::IDENTITY, 8192, Some((64, 64)));

        assert!(demands.is_empty());
    }

    #[test]
    fn a_quad_straddling_the_target_edge_still_demands() {
        let program = ShaderProgram::new(SOURCE);
        let scene = scene_of(|builder| {
            builder.draw_shader(&program, Rect::new(-8.0, -8.0, 8.0, 8.0), 0.0);
        });

        let demands = frame_demands(&scene, Affine::IDENTITY, 8192, Some((64, 64)));

        assert_eq!(
            demands.len(),
            1,
            "a quad straddling the target boundary is still partly visible"
        );
    }

    #[test]
    fn a_quad_touching_the_target_edge_still_demands() {
        // `Rect::overlaps` treats a shared edge as overlapping — deliberately
        // conservative, so a quad exactly abutting the target boundary is
        // never wrongly culled.
        let program = ShaderProgram::new(SOURCE);
        let scene = scene_of(|builder| {
            builder.draw_shader(&program, Rect::new(64.0, 0.0, 80.0, 16.0), 0.0);
        });

        let demands = frame_demands(&scene, Affine::IDENTITY, 8192, Some((64, 64)));

        assert_eq!(demands.len(), 1);
    }

    #[test]
    fn a_quad_fully_inside_the_target_extent_still_demands() {
        let program = ShaderProgram::new(SOURCE);
        let scene = scene_of(|builder| {
            builder.draw_shader(&program, Rect::new(4.0, 4.0, 12.0, 12.0), 0.0);
        });

        let demands = frame_demands(&scene, Affine::IDENTITY, 8192, Some((64, 64)));

        assert_eq!(demands.len(), 1);
    }

    #[test]
    fn a_quad_with_non_finite_geometry_is_dropped_by_requested_size_not_by_culling() {
        // A non-finite device bbox falls through the target-extent check
        // (which only culls a *finite* bbox proven not to overlap) rather
        // than being treated as "outside", so `requested_size`'s own
        // non-finite handling is what actually drops it — proven here by
        // still getting an empty result, not a panic or a false demand.
        let program = ShaderProgram::new(SOURCE);
        let scene = scene_of(|builder| {
            builder.draw_shader(&program, Rect::new(0.0, 0.0, f64::NAN, 8.0), 0.0);
        });

        assert!(frame_demands(&scene, Affine::IDENTITY, 8192, Some((64, 64))).is_empty());
    }

    #[test]
    fn set_target_extent_defaults_to_none() {
        let pass = ShaderQuadPass::new(None);
        assert_eq!(pass.target_extent, None);
    }
}
