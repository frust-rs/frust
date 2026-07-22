//! Shadertoy — a Frust shader showcase: a fragment-shader-driven `Component`
//! adapter over Shadertoy-style GLSL/WGSL shaders, painted through a
//! hand-rolled `frust-core`/`frust-scene` widget (see `Cargo.toml`'s
//! escape-hatch comment).
//!
//! This is the scaffold-only placeholder: a minimal screen that compiles and
//! runs on all three platforms. Task 06 replaces [`app_logic`] with the
//! actual `ShaderProgram` widget and shader picker; never edit
//! [`frust::app!`]'s invocation below or `main.rs` (spec §5.5's canonical
//! app shape).

use frust::{AnyView, Component, View, any, text};

/// Application state: empty for now — task 06 adds the selected shader and
/// any live-tunable uniforms.
pub struct AppState;

/// Pure view function: renders `AppState` into the placeholder screen (spec
/// §5). Task 06 replaces this body with the `ShaderProgram` canvas.
///
/// `+ use<>` opts the return type out of capturing the `&mut AppState`
/// lifetime — required in edition 2024, where `-> impl Trait` otherwise
/// captures all in-scope lifetimes. The view borrows nothing from `state`
/// (views are `'static`), so capturing nothing is correct.
pub fn app_logic(_state: &mut AppState) -> impl View<AppState> + use<> {
    text("shadertoy").size(32.0)
}

/// The generated app's root [`Component`] (spec §5.5): stateless config
/// (`Default`-constructed) that wires [`AppState`] as its local state.
#[derive(Default)]
pub struct ShadertoyApp;

impl Component for ShadertoyApp {
    type State = AppState;

    fn init(&self) -> AppState {
        AppState
    }

    fn build(&self, state: &mut AppState) -> AnyView<AppState> {
        any(app_logic(state))
    }
}

// The generated app's sole entry point (spec §5.5/§10): one line binds
// `ShadertoyApp` to all three platforms — the
// Android JNI exports (`target_os = "android"` only), the iOS C-ABI exports
// (unconditional; self-gated to `target_os = "ios"`), and (on desktop) the
// hidden `__frust_main` that `main.rs` calls. Never edit this line or
// `main.rs` — add screens/state to `app_logic`/`AppState` above instead.
frust::app!(ShadertoyApp);
