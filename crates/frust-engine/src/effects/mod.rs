//! Effects the engine renders *before* a frame's own passes, into textures the
//! frame then samples.
//!
//! Everything here shares one shape: a pre-pass produces an offscreen texture,
//! registers it with the renderer as a scene texture, and the display-list
//! command that asked for it lowers to an ordinary external-texture draw. That
//! keeps the frame path itself unaware of the effect — it sees a texture, the
//! same as one a host bound — and keeps the effect unaware of strips, paints
//! and depth.
//!
//! [`shader_quad`] is the one such effect today: user-supplied WGSL fragment
//! programs rendered for [`frust_scene::Command::ShaderQuad`].

pub mod shader_quad;

pub use shader_quad::{MAX_TEXTURE_DIM, ShaderQuadPass};
