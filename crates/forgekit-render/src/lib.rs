//! Layer 4: GPU backend — wgpu 29 + Vello 0.9 (spec §4, §8).
//!
//! Consumes the renderer-agnostic [`forgekit_scene::Scene`] display list and
//! renders it into a window's swapchain via Vello. Vello 0.9 has no
//! `render_to_surface`: it renders (by compute) into an intermediate
//! `Rgba8Unorm` texture, which is then blitted to the acquired swapchain
//! texture — that presentation model lives in [`RenderContext`] /
//! [`SurfaceRenderer`].
//!
//! `vello`/`wgpu` types are kept out of the public API except at two
//! deliberate seams: [`SurfaceRenderer::on_surface_created`] takes a
//! `wgpu::SurfaceTarget` (the shell must hand over a window), and
//! [`encode_scene`] exposes `vello::Scene` for shells that drive their own
//! renderer. Tier selection ([`RenderTier`], [`select_render_tier`], spec
//! Phase 6 / PLAN.md D4) probes real adapter downlevel flags via [`TierCaps`]
//! and honours an explicit override (env var / CLI flag, see
//! [`RENDER_TIER_ENV_VAR`]); the experimental `Cpu` tier is not yet wired
//! into device creation (only selectable behind the non-default `cpu-tier`
//! feature).
//!
//! Surface lifecycle is a first-class state machine (spec §8.1): see
//! [`SurfaceRenderer`] and [`SurfacePhase`]/[`FrameOutcome`] in [`lifecycle`].

mod context;
mod convert;
mod lifecycle;
mod renderer;
mod tier;

pub use context::RenderContext;
pub use convert::encode_scene;
pub use lifecycle::{FrameOutcome, SurfacePhase};
pub use renderer::SurfaceRenderer;
pub use tier::{
    GPU_REQUIRED_DOWNLEVEL_FLAGS, RENDER_TIER_ENV_VAR, RenderTier, TierCaps, TierOutcome,
    TierSelection, parse_render_tier_override, render_tier_override_from_env, select_render_tier,
};
