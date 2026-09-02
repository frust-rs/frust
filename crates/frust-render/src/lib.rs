//! Layer 4: GPU backend — wgpu 29 over the frust-owned `frust-engine` strip
//! pipeline.
//!
//! Consumes the renderer-agnostic [`frust_scene::Scene`] display list and
//! renders it into a window's swapchain. `frust-engine` draws through ordinary
//! render passes into the acquired swapchain texture — no compute pass, no
//! storage write, no intermediate and no blit — so the surface needs only
//! `RENDER_ATTACHMENT` in whatever format the platform reports, plus the depth
//! attachment the surface carries alongside it. The one variation is a
//! swapchain whose compositor genuinely reads STRAIGHT alpha: there the frame
//! lands in a surface-owned intermediate and one fragment pass un-premultiplies
//! it into the swapchain. Both presentation models live in [`RenderContext`] /
//! [`SurfaceRenderer`].
//!
//! `wgpu` types are kept out of the public API except at one deliberate seam:
//! [`SurfaceRenderer::on_surface_created`] takes a `wgpu::SurfaceTarget` (the
//! shell must hand over a window). Tier selection ([`RenderTier`],
//! [`select_render_tier`]) probes real adapter downlevel flags via [`TierCaps`]
//! and honours an explicit override (env var / CLI flag, see
//! [`RENDER_TIER_ENV_VAR`]) — `Engine` is the only tier this crate contains;
//! the experimental `vello_cpu`-backed `Cpu` fallback and its `cpu-tier`
//! feature were retired once the engine tier proved it needs no downlevel
//! capability that fallback existed to cover.
//!
//! [`HeadlessRenderer`] renders the same scenes with no surface at all — the
//! offscreen harness this crate's pixel-regression tests compare against,
//! resolving its adapter, limits and tier exactly as a real device does.
//!
//! Surface lifecycle is a first-class state machine: see
//! [`SurfaceRenderer`] and [`SurfacePhase`]/[`FrameOutcome`] in [`lifecycle`].

// The engine tier is no longer one renderer among several — it is the only
// one. Deleting vello classic left `engine-tier` carrying the whole surface
// pipeline (the render paths, the alpha routing, the headless harness), so a
// build without it has no renderer at all: it would fail deep inside those
// modules with a pile of unrelated type errors instead of saying so. The
// feature is kept (rather than folded away) because `frust-engine`/`frust-gpu`
// stay optional dependencies and later cards in this plan still key off it.
#[cfg(not(feature = "engine-tier"))]
compile_error!(
    "frust-render requires the `engine-tier` feature (its default): the vello-classic renderer \
     was deleted, so it is the only renderer this crate contains. Do not build with \
     `--no-default-features`."
);

mod context;
// Offscreen (no surface, no swapchain) engine rendering: the harness the
// pixel-regression tests render through. Reachable from outside the crate,
// unlike the effects module below, because those tests live outside it — and
// still leaking no `wgpu` type (see its module docs).
mod headless;
mod lifecycle;
mod pipeline_cache;
mod renderer;
// Offscreen WGSL fragment-shader effects (shader-showcase feature). Currently
// UNWIRED: its only caller was the vello-classic tier's encode-time pre-pass,
// which registered each rendered quad with `vello::Renderer` as an image
// override — a seam that died with vello, and one the engine tier has no
// counterpart for yet. Retained (rather than deleted) because the engine plan
// moves its compile half into `frust-gpu`, where the engine can drive it; the
// `allow` is what keeps the module intact without an unused-code failure
// meanwhile.
#[allow(dead_code)]
mod shader_effects;
mod tier;

pub use context::{DetachedSurface, RenderContext, SurfaceFactory};
pub use headless::{GOLDEN_EXPECT_ADAPTER_ENV_VAR, GOLDEN_EXPECT_BACKEND_ENV_VAR};
#[cfg(feature = "engine-tier")]
pub use headless::{HeadlessImage, HeadlessMeta, HeadlessOptions, HeadlessRenderer, HeadlessSpec};
pub use lifecycle::{AcquireOutcome, EncodeOutcome, FrameOutcome, SurfacePhase};
pub use renderer::{DeferredPresent, SurfaceAlphaRequest, SurfaceRenderer};
pub use tier::{
    ENGINE_REQUIRED_DOWNLEVEL_FLAGS, RENDER_TIER_ENV_VAR, RenderTier, TierCaps, TierOutcome,
    TierSelection, parse_render_tier_override, render_tier_override_from_env, select_render_tier,
};
