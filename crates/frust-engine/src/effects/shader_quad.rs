//! The pre-pass behind [`Command::ShaderQuad`]: a user-supplied WGSL fragment
//! program rendered into an offscreen texture the frame then draws as a scene
//! texture.
//!
//! # The split
//!
//! Every GPU resource the effect needs — the lazily compiled per-program
//! pipeline, the per-`(program, size)` target pool, the fullscreen-triangle
//! pass, the age-based reap and the churn detector — lives one layer down in
//! [`frust_gpu::effects`], which speaks `(id, wgsl, size, time)` primitives and
//! knows nothing about scenes. What lives here is the half that reads a display
//! list: which programs a frame draws, how big a target each one may ask for,
//! and the id its rendered result is registered under.
//!
//! # How a quad reaches the screen
//!
//! [`ShaderQuadPass::prepare`] runs once per frame, before the frame's own
//! passes are recorded and into the *same* [`wgpu::CommandEncoder`], so the
//! effect and the frame that samples it can never be submitted apart:
//!
//! 1. walk the display list for its [`Command::ShaderQuad`]s
//!    ([`frame_demands`]), collapsing every quad of one program into a single
//!    demand;
//! 2. compile the program if it is new, size a pooled target for it, and
//!    record its fullscreen-triangle pass into the encoder;
//! 3. register that target with the [`EngineRenderer`] under
//!    [`SceneTextureId::for_shader_program`], the id the compiler's own
//!    lowering derives from the same program.
//!
//! The draw itself is then nothing special: the compiler lowers the quad to an
//! external-texture paint over its destination rectangle, exactly as it lowers
//! [`Command::SceneTexture`](frust_scene::Command::SceneTexture) — same
//! natural-pixels-onto-destination mapping, same blended pass.
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

use frust_gpu::{SceneTextureId, ShaderEffects};
use frust_scene::{Command, Scene, ShaderProgram};
use kurbo::{Affine, Rect};

use crate::config;
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

/// The target extent a quad over `dest` under `transform` asks for: the device
/// -space bounding box of the destination rectangle, rounded up so a fractional
/// edge is covered rather than cropped, and clamped by [`clamp_size`].
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
    let device = transform.transform_rect_bbox(dest);
    if !device.is_finite() {
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
    Some(clamp_size(
        (axis(device.width()), axis(device.height())),
        adapter_max,
    ))
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
fn frame_demands(scene: &Scene, root: Affine, adapter_max: u32) -> Vec<QuadDemand<'_>> {
    let mut order: Vec<u64> = Vec::new();
    let mut demands: HashMap<u64, QuadDemand<'_>> = HashMap::new();

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
        let Some(size) = requested_size(*dest, root * *transform, adapter_max) else {
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
    /// What makes registration incremental: a program whose target is unchanged
    /// is left bound, so the frame's bind groups naming it survive instead of
    /// being dropped and rebuilt every frame. An entry here is always matched
    /// by a live registration on the renderer, which is what lets a reap unbind
    /// exactly what it reaped.
    registered: HashMap<u64, (u32, u32)>,
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
        }
    }

    /// How many programs currently have a target registered with a renderer.
    #[must_use]
    pub fn registered_len(&self) -> usize {
        self.registered.len()
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

        let demands = frame_demands(scene, root, device.limits().max_texture_dimension_2d);
        let mut live_ids: HashSet<u64> = HashSet::with_capacity(demands.len());
        let mut live_keys: HashSet<(u64, u32, u32)> = HashSet::with_capacity(demands.len());

        for demand in demands {
            let id = demand.program.id();
            let (w, h) = demand.size;
            live_ids.insert(id);
            live_keys.insert((id, w, h));

            self.effects
                .ensure_pipeline(device, id, demand.program.source());
            self.effects.ensure_target(device, id, w, h);
            self.effects
                .encode_pass(encoder, queue, id, (w, h), demand.time);
            self.register(engine, id, (w, h));
        }

        // Frame-scoped first — the sizes a still-drawn program resized away
        // from — then the age-based whole-program reap, which is the one that
        // has registrations to withdraw. `mark_seen` is called on every frame,
        // empty demand list included: that is the clock a vanished program ages
        // against.
        self.effects.evict_stale_targets(&live_keys);
        let stale = self.effects.mark_seen(&live_ids);
        self.effects.reap(&stale);
        for id in stale {
            self.unregister(engine, id);
        }
    }

    /// Registers program `id`'s `(w, h)` target with `engine`, or withdraws any
    /// earlier registration when there is no such target to register.
    ///
    /// A no-op when the program is already registered at the extent its target
    /// was built at, which is the common case on every frame after the first:
    /// re-registering would drop the frame's bind groups naming the view and
    /// rebuild them for an identical binding.
    fn register(&mut self, engine: &mut EngineRenderer, id: u64, size: (u32, u32)) {
        let (w, h) = size;
        let Some((extent, view)) = self
            .effects
            .target_extent(id, w, h)
            .zip(self.effects.target_view(id, w, h).cloned())
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
    fn a_scene_without_shader_quads_demands_nothing() {
        let scene = scene_of(|builder| {
            builder.fill_rect(
                Rect::new(0.0, 0.0, 8.0, 8.0),
                peniko::Brush::Solid(peniko::color::palette::css::RED),
            );
        });

        assert!(frame_demands(&scene, Affine::IDENTITY, 8192).is_empty());
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

        let demands = frame_demands(&scene, Affine::IDENTITY, 8192);

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

        let demands = frame_demands(&scene, Affine::IDENTITY, 8192);

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

        let demands = frame_demands(&scene, Affine::IDENTITY, 8192);

        assert_eq!(demands[0].time, 1.5);
    }

    #[test]
    fn a_quad_whose_geometry_no_target_can_be_sized_from_is_dropped() {
        let program = ShaderProgram::new(SOURCE);
        let scene = scene_of(|builder| {
            builder.draw_shader(&program, Rect::new(0.0, 0.0, f64::NAN, 8.0), 0.0);
        });

        assert!(frame_demands(&scene, Affine::IDENTITY, 8192).is_empty());
    }

    #[test]
    fn the_frame_transform_scales_the_demand() {
        let program = ShaderProgram::new(SOURCE);
        let scene = scene_of(|builder| {
            builder.draw_shader(&program, Rect::new(0.0, 0.0, 100.0, 50.0), 0.0);
        });

        let demands = frame_demands(&scene, Affine::scale(3.0), 8192);

        assert_eq!(demands[0].size, (300, 150));
    }

    #[test]
    fn a_demand_is_clamped_by_the_adapter_ceiling() {
        let program = ShaderProgram::new(SOURCE);
        let scene = scene_of(|builder| {
            builder.draw_shader(&program, Rect::new(0.0, 0.0, 6000.0, 6000.0), 0.0);
        });

        let demands = frame_demands(&scene, Affine::IDENTITY, 4096);

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
}
