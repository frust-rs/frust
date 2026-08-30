//! Layer 4: Frust Engine — the sparse-strip GPU render pipeline.
//!
//! `frust-engine` turns a `frust_scene::Scene` into recorded GPU work. One
//! [`EngineRenderer`] drives one surface: it compiles the display list into
//! sparse strips ([`compile`]), packs them into the layouts the WGSL reads
//! ([`gpu`]), and records the frame's passes into a [`EngineTarget`] through a
//! command encoder the *caller* owns and submits. Every rendering path returns
//! an [`EngineError`] rather than panicking (E17).
//!
//! The crate splits along one line throughout: pure decisions over plain
//! values on one side (sizing, packing, addressing, pool keying — all
//! host-testable with no GPU), and a small number of named entry points that
//! touch a live `wgpu::Device` on the other. [`renderer`] is where the two
//! meet.
//!
//! Configuration is process-global and cached, selected from environment
//! variables read at compile time (`option_env!`) or run time
//! (`std::env::var`); see [`config`] for the kill switches over layers, atlas,
//! pooling, depth and resource limits.

pub mod cache;
pub mod compile;
pub mod config;
pub mod error;
pub mod gpu;
pub mod renderer;
pub mod schedule;

pub use cache::{CachedRamp, GradientCache, GradientTextureLayout};
pub use compile::paint::{BrushEncoding, LutRequest, encode_brush};
pub use compile::{CompiledFrame, DepthCounter, EngineDraw, SceneCompiler};
pub use error::EngineError;
pub use gpu::{
    DepthAttachment, DepthTexture, EnginePipeline, EngineShaderModule, EngineShaders, GpuConfig,
    GpuEncodedPaint, GpuStrip, IntermediateTargets, IntermediateTexture, StripDraw,
};
pub use renderer::EngineRenderer;
pub use schedule::{Composite, PageParity, PageTarget, Round, RoundOp, RoundTarget, Schedule};

/// Alpha output mode for the render target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputAlpha {
    /// Premultiplied alpha (color already multiplied by alpha).
    Premultiplied,
    /// Straight (unpremultiplied) alpha.
    Straight,
}

/// Render target descriptor passed to [`EngineRenderer::encode`].
///
/// Describes one frame's destination: the view to draw into, the format and
/// extent that view was created with, and how the result's alpha is to be
/// interpreted.
///
/// `depth` is the caller's own depth attachment, for a host that already ran a
/// 3D pass into the same colour target and wants the 2D pass to test against
/// the depth that pass established. Leaving it `None` lets the engine own a
/// depth attachment of its own; either way, pair it with
/// [`EngineRenderer::set_depth_pre_cleared`] so the frame loads a populated
/// buffer instead of clearing it.
#[derive(Debug)]
pub struct EngineTarget<'a> {
    /// The destination texture view to render into.
    pub view: &'a wgpu::TextureView,
    /// The texture format of the target (must match the view).
    pub format: wgpu::TextureFormat,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Optional depth texture view for depth operations.
    pub depth: Option<&'a wgpu::TextureView>,
    /// Alpha output mode.
    pub output: OutputAlpha,
}

/// Engine-wide render settings and configuration.
///
/// Collects render options such as quality, feature toggles, and resource constraints.
pub struct EngineSettings {
    // Settings to be defined as phases progress
}
