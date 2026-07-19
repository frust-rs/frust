//! Layer 3: vector scene / display list, renderer-agnostic scene builder (spec §4).
//!
//! `Scene`/`SceneBuilder`/`Command` are the stable seam between widgets (layer
//! 1+2) and the GPU backend (layer 4, `frust-render`). Only `kurbo`
//! (geometry) and `peniko` (brushes/fonts) appear in this crate's public API —
//! `vello`/`wgpu` types are forbidden here so the render backend can be swapped
//! later (spec §7).

mod arc;
mod builder;
mod glyph;
mod scene;

pub use arc::arc_path;
pub use builder::SceneBuilder;
pub use glyph::{FontHandle, Glyph, GlyphRun};
pub use scene::{Command, PathStyle, Scene};
