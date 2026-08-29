//! Layer 4: Frust Engine — GPU render pipeline abstraction for Phase 3.
//!
//! `frust-engine` provides the skeleton of the rendering engine, with
//! configuration kill switches, error handling, and the public seam types
//! used by shells and integrations.
//!
//! The engine accepts [`EngineTarget`] descriptors (texture views, formats, dimensions)
//! and fills them via [`EngineRenderer`], returning engine-specific errors on failure.
//! All rendering paths return errors rather than panicking (E17).
//!
//! Configuration is global and cached per-process, selected via environment variables
//! parsed at compile-time (via `option_env!`) or runtime (via `std::env::var`),
//! supporting kill switches for layers, atlas, pooling, depth, and resource limits.

pub mod cache;
pub mod compile;
pub mod config;
pub mod error;
pub mod gpu;

pub use cache::{CachedRamp, GradientCache, GradientTextureLayout};
pub use compile::paint::{BrushEncoding, LutRequest, encode_brush};
pub use compile::{CompiledFrame, DepthCounter, EngineDraw, SceneCompiler};
pub use error::EngineError;
pub use gpu::{
    EnginePipeline, EngineShaderModule, EngineShaders, GpuConfig, GpuEncodedPaint, GpuStrip,
    StripDraw,
};

/// Alpha output mode for the render target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputAlpha {
    /// Premultiplied alpha (color already multiplied by alpha).
    Premultiplied,
    /// Straight (unpremultiplied) alpha.
    Straight,
}

/// Render target descriptor passed to [`EngineRenderer`].
///
/// Specifies the texture view, format, dimensions, and optional depth texture
/// for the engine to render into.
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

/// Engine renderer — handles scene rendering and GPU resource management.
///
/// Filled in by Phase 3, step p3-07. Placeholder type for public API seam.
pub struct EngineRenderer {
    // Implementation details to be added in p3-07
}

/// Engine-wide render settings and configuration.
///
/// Collects render options such as quality, feature toggles, and resource constraints.
pub struct EngineSettings {
    // Settings to be defined as phases progress
}
