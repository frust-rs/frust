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
//! deliberate seams: [`RenderContext::create_surface`] takes a
//! `wgpu::SurfaceTarget` (the shell must hand over a window), and
//! [`encode_scene`] exposes `vello::Scene` for shells that drive their own
//! renderer. Tier selection ([`RenderTier`], [`select_render_tier`]) is stubbed
//! at `Gpu` until spec Phase 6.

mod context;
mod convert;
mod renderer;
mod tier;

pub use context::RenderContext;
pub use convert::encode_scene;
pub use renderer::SurfaceRenderer;
pub use tier::{RenderTier, select_render_tier};
