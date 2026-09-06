//! Layer 4: GPU backend — wgpu 30 over the frust-owned `frust-engine` strip
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
//! shell must hand over a window). There is no renderer to select: the engine
//! is the only one this crate contains, so what was a tier probe is now a
//! plain capability gate ([`engine_support`] over [`TierCaps`], against
//! [`ENGINE_REQUIRED_DOWNLEVEL_FLAGS`]) that refuses an adapter which cannot
//! run it. The cargo feature that gated the engine, the env-var/CLI override
//! that picked a renderer, and the vello-classic/`vello_cpu` tiers they chose
//! between are all gone.
//!
//! [`HeadlessRenderer`] renders the same scenes with no surface at all — the
//! offscreen harness this crate's pixel-regression tests compare against,
//! resolving its adapter and limits exactly as a real device does and asking
//! the same capability gate.
//!
//! [`external_pass`] is the third `wgpu`-typed seam, and the one that points
//! outward: an app or a crate beside the facade registers an [`ExternalPass`],
//! and [`SurfaceRenderer`] hands it the frame's own device, queue and encoder
//! ahead of the scene pass so it can render into a target of its own and bind
//! the result for a `Command::SceneTexture` to composite. See that module's
//! docs for the contract; `HeadlessRenderer` does not drain that registry.
//!
//! Surface lifecycle is a first-class state machine: see [`SurfaceRenderer`]
//! and [`SurfacePhase`]/[`FrameOutcome`], whose pure transition tables live in
//! `frust_gpu::lifecycle` and are re-exported here.
//!
//! # Where the foundation lives
//!
//! The `wgpu` instance, the lazily created logical device, the surface factory
//! and its cross-thread [`DetachedSurface`] hand-off, the surface lifecycle
//! state machine, the persisted pipeline-cache framing and the offscreen
//! shader-effect pipelines are all `frust-gpu`'s
//! ([`frust_gpu::context::RenderContext`], [`frust_gpu::surface`],
//! [`frust_gpu::lifecycle`], [`frust_gpu::pipeline_cache`],
//! [`frust_gpu::effects`]) — one copy, shared with every other consumer of that
//! crate. The context/surface/lifecycle types a shell touches are re-exported
//! here under the names they have always had, so `frust_render::RenderContext`
//! keeps working; the pipeline-cache framing and the shader-effect pipelines
//! were never public under this crate and are reached as
//! `frust_gpu::pipeline_cache`/`frust_gpu::effects` directly. What is genuinely this crate's own is the
//! renderer: the render-path decision, the engine resources each arm owns, the
//! engine's own capability gate, and [`SurfaceRenderer`] itself.

mod context;
// The pre-scene GPU seam: caller-supplied passes recorded into the frame's own
// encoder ahead of the scene, binding their results into the engine's
// external-texture registry. Public as a module (rather than only through the
// flat re-export below) so its own docs — the whole contract a pass is written
// against — are reachable under one heading.
pub mod external_pass;
// Offscreen (no surface, no swapchain) engine rendering: the harness the
// pixel-regression tests render through. Reachable from outside the crate,
// unlike the effects module below, because those tests live outside it — and
// still leaking no `wgpu` type (see its module docs).
mod headless;
mod renderer;
mod tier;

// The device/surface foundation, re-exported name for name from `frust-gpu`:
// every one of these was defined in this crate before the two copies were
// merged, and a shell must not have to care that they moved.
pub use frust_gpu::{
    AcquireOutcome, DetachedSurface, EncodeOutcome, FrameOutcome, RenderContext,
    SurfaceAlphaRequest, SurfaceFactory, SurfacePhase,
};
// `DeviceHandle` (the cheap-to-clone device/queue/adapter pair a shell hands
// off to `frust-shell-common`'s process-wide GPU slot — see that crate's
// `gpu` module) stays behind this crate's own `gpu` feature rather than
// joining the unconditional re-export above: `frust-gpu` itself is not
// optional, so gating costs nothing but keeps a default build of this crate
// (the only one most apps ever produce) at zero new public symbols.
#[cfg(feature = "gpu")]
pub use frust_gpu::DeviceHandle;
// Flat, alongside every other name a caller reaches this crate by; the module
// above stays public for its docs.
pub use external_pass::{
    ExternalFrame, ExternalPass, register_external_pass, unregister_external_pass,
};
pub use headless::{
    GOLDEN_EXPECT_ADAPTER_ENV_VAR, GOLDEN_EXPECT_BACKEND_ENV_VAR, HeadlessImage, HeadlessMeta,
    HeadlessOptions, HeadlessRenderer, HeadlessSpec,
};
// `DeferredPresent` stays here: it wraps a frame this crate's renderer
// acquired, submitted and handed back un-presented, which is a renderer
// concern, not a foundation one.
pub use renderer::{DeferredPresent, SurfaceRenderer};
// The adapter-capability gate is all that is left of the tier seam: the
// renderer itself is no longer a choice, so the tier enum, its selection
// result types, the selection function, the env-var name and the override
// parsers are all gone from this surface along with the choice they
// described.
pub use tier::{ENGINE_REQUIRED_DOWNLEVEL_FLAGS, EngineUnsupported, TierCaps, engine_support};
