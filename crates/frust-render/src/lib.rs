//! Layer 4: GPU backend — wgpu 29 + Vello 0.9.
//!
//! Consumes the renderer-agnostic [`frust_scene::Scene`] display list and
//! renders it into a window's swapchain via Vello. Vello 0.9 has no
//! `render_to_surface`: it renders (by compute) into an `Rgba8Unorm` storage
//! texture. Where the surface itself supports `Rgba8Unorm` + `STORAGE_BINDING`
//! (probed per surface), Frust renders **direct-to-surface** — vello targets the
//! acquired swapchain texture and there is no blit; otherwise it
//! falls back to rendering into an intermediate `Rgba8Unorm` texture blitted to
//! the swapchain each frame. Both presentation models live in [`RenderContext`]
//! / [`SurfaceRenderer`].
//!
//! `vello`/`wgpu` types are kept out of the public API except at two
//! deliberate seams: [`SurfaceRenderer::on_surface_created`] takes a
//! `wgpu::SurfaceTarget` (the shell must hand over a window), and
//! [`encode_scene`] exposes `vello::Scene` for shells that drive their own
//! renderer. Tier selection ([`RenderTier`], [`select_render_tier`])
//! probes real adapter downlevel flags via [`TierCaps`]
//! and honours an explicit override (env var / CLI flag, see
//! [`RENDER_TIER_ENV_VAR`]); the experimental `Cpu` tier (vello_cpu, behind the
//! non-default `cpu-tier` feature) is wired into device creation and
//! presentation only when that feature is compiled in — its rasterized
//! `Pixmap` is uploaded into the same intermediate target the GPU path blits
//! from (see the `cpu_tier` module).
//!
//! Two Gpu-tier pre-passes run inside `encode` before the main scene is built.
//! The fragment-shader pre-pass (`shader_effects`) renders each
//! `Command::ShaderQuad` into an offscreen texture registered with vello as an
//! image override. The snapshot-layer cache (`snapshot`) rasterizes a stable
//! subtree once into its own texture and hands back a frame plan; those
//! textures never enter vello at all — `compositor` draws each of them as one
//! alpha-blended quad in a wgpu render pass AFTER vello, on every render arm,
//! while the bracket's `alpha`/`scale` animate for free.
//!
//! [`HeadlessRenderer`] renders the same scenes with no surface at all — the
//! offscreen oracle harness golden and pixel-regression tests compare against,
//! resolving its adapter, limits and tier exactly as a real device does.
//!
//! Surface lifecycle is a first-class state machine: see
//! [`SurfaceRenderer`] and [`SurfacePhase`]/[`FrameOutcome`] in [`lifecycle`].

// The wgpu quad pass that draws `snapshot`'s cached page textures onto a
// frame. Crate-private; runs after vello on every render arm (see
// `SurfaceRenderer::encode`/`submit`), because a full-page image quad inside
// vello costs ~100 ms/frame on a low-end mobile GPU and one blended quad
// costs ~1-2 ms.
mod compositor;
mod context;
mod convert;
#[cfg(feature = "cpu-tier")]
mod cpu_tier;
// Offscreen (no surface, no swapchain) vello-classic rendering: the harness
// the oracle/golden and pixel-regression tests render through. Reachable from
// outside the crate, unlike the pre-pass modules, because those tests live
// outside it — and still leaking no `vello`/`wgpu` type (see its module docs).
mod headless;
// Experimental vello_hybrid render tier (non-default `hybrid-tier` feature):
// a third `SceneSink` over the same command walk, so the sparse-strip core can
// be measured against vello on real scenes. Crate-private — its entry points
// take bare `wgpu` types, which this crate's API confines (see
// `docs/RENDER_ARCHITECTURE.md`).
#[cfg(feature = "hybrid-tier")]
mod hybrid_tier;
mod lifecycle;
mod pipeline_cache;
mod renderer;
// Offscreen WGSL fragment-shader effects (shader-showcase feature). Crate-
// private; wired into `SurfaceRenderer::encode`'s Gpu-tier shader pre-pass,
// which compiles/renders each `Command::ShaderQuad` program into an offscreen
// texture and registers it as a vello image override before vello encoding.
mod shader_effects;
// Cached rasterizations of `Command::PushSnapshot` bodies. Crate-private;
// wired into `SurfaceRenderer::encode`'s Gpu-tier snapshot pre-pass, which
// rasterizes each outermost bracket's body into its own texture and returns
// the frame plan the encode walk skips those bodies by and `compositor` draws
// them from.
mod snapshot;
mod tier;

pub use context::{DetachedSurface, RenderContext, SurfaceFactory};
pub use convert::encode_scene;
pub use headless::{
    GOLDEN_EXPECT_ADAPTER_ENV_VAR, GOLDEN_EXPECT_BACKEND_ENV_VAR, HeadlessAa, HeadlessImage,
    HeadlessMeta, HeadlessOptions, HeadlessRenderer, HeadlessSpec, ShaderOverrideSpec,
};
pub use lifecycle::{AcquireOutcome, EncodeOutcome, FrameOutcome, SurfacePhase};
pub use renderer::{DeferredPresent, SurfaceAlphaRequest, SurfaceRenderer};
pub use tier::{
    GPU_REQUIRED_DOWNLEVEL_FLAGS, RENDER_TIER_ENV_VAR, RenderTier, TierCaps, TierOutcome,
    TierSelection, parse_render_tier_override, render_tier_override_from_env, select_render_tier,
};
