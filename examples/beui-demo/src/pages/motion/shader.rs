//! Stub: Motion · Shader — this scaffold's placeholder (see `crate::pages`
//! for the convention every stub leaf follows).

use frust::AnyView;

use crate::AppState;

/// b-11b fills this page in with the WGSL shader-background variants, gated
/// on the engine's `ShaderQuad` (p9-03, premultiplied alpha).
pub fn page() -> AnyView<AppState> {
    crate::pages::stub_page(
        "Motion \u{b7} Shader",
        "b-11b fills this page in with the WGSL shader-background variants, gated on the engine's ShaderQuad (p9-03).",
    )
}
