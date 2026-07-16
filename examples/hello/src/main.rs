//! The Phase 0 exit-criterion example: render "Hello from ForgeKit" in the
//! desktop preview window (spec §5, §14).
//!
//! Phase 5.5: migrated to the [`Component`](forgekit::Component) model — the
//! smallest possible app, a single `HelloApp` component whose retained state
//! is just the greeting string, driven through [`forgekit::run`] (the
//! desktop preview `runApp` equivalent). `hello` has no library/mobile story
//! today, so — per the migration plan — this stays a single `main.rs`
//! calling `forgekit::run` directly rather than inventing a `lib.rs`/`app!`
//! split it doesn't need.

use forgekit::{AnyView, Component, any, text};

/// The whole app: a [`Component`](forgekit::Component) whose retained state
/// is just the greeting string to display.
struct HelloApp;

impl Component for HelloApp {
    type State = String;

    /// Seed the initial greeting once, when the app starts.
    fn init(&self) -> String {
        "Hello from ForgeKit".into()
    }

    /// Map the greeting to a view tree. Re-run each frame (spec §5).
    fn build(&self, state: &mut String) -> AnyView<String> {
        any(text(state.clone()).size(32.0))
    }
}

fn main() {
    forgekit::run(HelloApp).unwrap();
}
