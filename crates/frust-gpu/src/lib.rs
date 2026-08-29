//! The `wgpu` adapter/device/surface substrate the render engine builds on.
//!
//! `frust-gpu` owns nothing rendering-specific — no scene display list, no
//! shader pipeline, no `vello`/`glifo` dependency. Its whole job is the layer
//! directly above `wgpu` itself: probing what an adapter can actually do
//! ([`caps::TierCaps`]) and, in later work, turning that into instances,
//! devices, surfaces and pooled GPU resources a renderer built on top of it
//! can consume without re-deriving adapter capabilities itself.
//!
//! [`caps::TierCaps::probe`] is the only place a real `wgpu::Adapter` is
//! consulted; every decision built on top of it takes the plain
//! [`caps::TierCaps`] value instead, so it stays testable with
//! [`caps::TierCaps::fake`] and no GPU in the loop. [`surface`] and
//! [`lifecycle`] follow the same split: every policy decision they make is a
//! pure function over plain values, and only a handful of named entry points
//! touch a live `wgpu::Device` or a real window handle.

pub mod arena;
pub mod caps;
pub mod context;
pub mod encoder;
pub mod headless;
pub mod lifecycle;
pub mod lint;
pub mod pipeline;
pub mod pipeline_cache;
pub mod pool;
pub mod shader;
pub mod surface;
pub mod texture;

pub use arena::{BufferSlice, BufferUploader, HostBuffer};
pub use caps::{DownlevelProfile, TierCaps};
pub use context::{Context, ContextOptions, DeviceHandle};
pub use encoder::CommandBuffer;
pub use headless::HeadlessTarget;
pub use lifecycle::{
    AcquireAction, AcquireOutcome, AcquireStatus, EncodeOutcome, FrameOutcome, SurfaceEvent,
    SurfacePhase,
};
pub use lint::{check_limits_against_webgl2, lint_pipeline_layout, lint_wgsl_dir};
pub use pipeline::{PipelineCache, RenderPipelineDesc, VertexLayout};
pub use pool::{PoolKey, PoolStats, PooledTexture, TextureAllocator, TexturePool};
pub use shader::{ShaderId, ShaderLibrary};
pub use surface::{
    ConfiguredSurface, DetachedSurface, SURFACE_FORMATS, SurfaceAlphaRequest, SurfaceFactory,
};
pub use texture::{
    Attachment, ColorAttachment, DepthAttachment, RenderTarget, SceneTextureId, Texture,
    TextureDesc, TextureId, TextureRegistry,
};
