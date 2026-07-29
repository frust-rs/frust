//! Layer 3: vector scene / display list, renderer-agnostic scene builder.
//!
//! `Scene`/`SceneBuilder`/`Command` are the stable seam between widgets (layer
//! 1+2) and the GPU backend (layer 4, `frust-render`). Only `kurbo`
//! (geometry) and `peniko` (brushes/fonts) appear in this crate's public API —
//! `vello`/`wgpu` types are forbidden here so the render backend can be swapped
//! later.

mod arc;
mod builder;
mod glyph;
mod scene;
mod shader;

pub use arc::arc_path;
pub use builder::SceneBuilder;
pub use glyph::{FontHandle, Glyph, GlyphRun};
pub use scene::{Command, PathStyle, Scene};
pub use shader::ShaderProgram;

/// Compile-time assertion that [`Scene`] is [`Send`].
///
/// This tripwire is a refactor guard: any future field added to [`Scene`]
/// that is `!Send` will fail this assertion with a helpful compiler message
/// pointing to this site.
///
/// The assertion is necessary to keep the render-thread-split path viable —
/// splitting render onto a dedicated thread requires the scene to move
/// safely across thread boundaries.
const _: () = {
    /// Zero-cost static assertion that `T: Send`.
    const fn assert_send<T: Send>() {}

    const fn check_scene_is_send() {
        assert_send::<Scene>();
    }

    // Invoke the assertion at compile time
    let () = check_scene_is_send();
};
