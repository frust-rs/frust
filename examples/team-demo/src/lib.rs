//! `team-demo` — a full ForgeKit Android+desktop app exercising both
//! `clean-signals` framework crates end to end.
//!
//! A single `team` feature slice (a small team-roster feature), laid out per
//! `templates/AGENTS.md`'s feature-slice convention — `domain/ ← data/`,
//! `domain/ ← presentation/`, nothing crossing the other direction. The domain,
//! data, and controller layers are ported nearly verbatim from the Leptos twin
//! (`clean-signals-rs/examples/team-demo`); only the presentation view
//! ([`features::team::presentation::screen`]) is a ForgeKit rewrite over the
//! `clean-signals-forgekit` glue.
//!
//! [`TeamApp`] is the root [`Component`] (`Default`, as [`forgekit::app!`]
//! requires); it hosts the injected-repo [`TeamScreen`] so the screen's `init`
//! runs under a real component reactive `Owner` (controller-disposal-on-teardown
//! and the failure-listener subscription both scope to it). The module tree and
//! `failure`/`features` items are `pub` so the headless team tests
//! (`tests/team.rs`) can drive the screen and its controller directly.

pub mod failure;
pub mod features;

use forgekit::{AnyView, Component};

pub use features::team::presentation::{
    TeamController, TeamHandles, TeamScreen, TeamSpy, TeamState,
};

/// The app root: hosts [`TeamScreen`] with the default in-memory backend so its
/// `init` runs under a real component [`Owner`]. `Default`-constructed and
/// stateless (`State = ()`), the shape [`forgekit::app!`] binds.
#[derive(Default)]
pub struct TeamApp;

impl Component for TeamApp {
    type State = ();

    fn init(&self) {}

    fn build(&self, _state: &mut ()) -> AnyView<()> {
        features::team::presentation::team_screen_with_default_repo()
    }
}

// The generated app's sole entry point (spec §5.5/§10): one line binds
// `TeamApp` to all three platforms — the Android JNI exports
// (`target_os = "android"` only), the iOS C-ABI exports (unconditional;
// self-gated to `target_os = "ios"`), and (on desktop) the hidden
// `__forgekit_main` that `main.rs` calls.
forgekit::app!(TeamApp);
