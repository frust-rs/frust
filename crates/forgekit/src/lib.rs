//! Facade crate: the public `forgekit` framework API (spec §5).
//!
//! App authors depend on this single crate. It exposes the [`App`] entry point
//! and curates the view/widget vocabulary from the underlying framework crates
//! so the declarative call shape reads exactly as the spec promises:
//!
//! ```no_run
//! struct AppState {
//!     greeting: String,
//! }
//!
//! // `+ use<>`: opt out of edition-2024's implicit lifetime capture; views
//! // are `'static` and borrow nothing from `state`.
//! fn app_logic(state: &mut AppState) -> impl forgekit::View<AppState> + use<> {
//!     forgekit::text(state.greeting.clone()).size(32.0)
//! }
//!
//! forgekit::App::new(AppState { greeting: "Hello from ForgeKit".into() }, app_logic)
//!     .run()
//!     .unwrap();
//! ```

pub use forgekit_core::view::View;
pub use forgekit_widgets::{TextView, text};

/// A ForgeKit application: the app state plus the `app_logic` function that maps
/// it to a view tree (spec §5).
///
/// Construct with [`App::new`] and start the event loop with [`App::run`].
///
/// The view type is intentionally *not* a parameter of this struct: capturing a
/// free `fn app_logic(&mut State) -> impl View<State>`'s opaque return type into
/// a stored type parameter defeats method resolution (the opaque type's trait
/// bounds can't be re-proven on the already-typed value). Instead [`App::run`]
/// infers the view type freshly at the call site, so the exact spec §5 shape —
/// `App::new(state, app_logic).run()` — compiles for both `impl View` and
/// concrete-typed `app_logic`.
pub struct App<State, Logic> {
    state: State,
    logic: Logic,
}

impl<State, Logic> App<State, Logic> {
    /// Create an app from an initial `state` and its `app_logic`.
    ///
    /// `logic` is a `FnMut(&mut State) -> impl View<State>` re-run each frame to
    /// produce the current view tree.
    pub fn new(state: State, logic: Logic) -> Self {
        Self { state, logic }
    }
}

impl<State: 'static, Logic> App<State, Logic> {
    /// Run the app in the desktop preview window until it is closed (spec §12.9).
    ///
    /// Blocks the calling thread on the platform event loop. Returns once the
    /// window closes, or an error if the window/GPU surface could not be
    /// created. The concrete view type `V` is inferred from `logic`.
    pub fn run<V>(self) -> anyhow::Result<()>
    where
        V: View<State>,
        Logic: FnMut(&mut State) -> V + 'static,
    {
        forgekit_shell_desktop::run_desktop(self.state, self.logic)
    }
}
