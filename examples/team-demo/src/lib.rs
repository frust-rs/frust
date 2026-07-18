//! `team-demo` — ForgeKit's single showcase app (examples-convergence PLAN.md).
//!
//! A 4-tab, [`Router`]-driven showcase built on the `clean-signals`
//! clean-architecture spine. The original team-roster feature slice
//! (`domain/ ← data/`, `domain/ ← presentation/`, per `templates/AGENTS.md`)
//! keeps working exactly as-is at the `/` route; around it sits a persistent
//! shell (top app bar + bottom navigation, branched on the active
//! [`DesignLanguage`]) hosting every other screen.
//!
//! # Wave layout (the placeholder contract)
//!
//! This module (task 02, "team-demo showcase shell") finalizes everything
//! **shared**: [`lib.rs`](self), [`shell`], [`routes`], [`screens`]'s module
//! declarations, [`controllers`], and the three `*_domain` placeholder
//! modules. The `screens/*` files are pre-registered so wave-2 tasks fill only
//! their own disjoint screen file and never touch a shared file:
//!
//! - real, shipped here: [`screens::showcase_menu`], [`screens::nav_playground`]
//!   (ported from `examples/navdemo`), [`screens::member_detail`] (`/member/:id`).
//! - placeholders (each names its owning wave-2 task): the widgets, theme,
//!   motion, notes, settings, and profile screens.
//!
//! # Theme delivery
//!
//! The shell reads the active [`Theme`] via `use_context::<Theme>()` (the
//! `examples/catalog` read pattern) and branches its Material/Cupertino chrome
//! on `theme.design_language`. The settings-driven `set_app_theme` mechanism
//! lands in task 06; until then the app runs on the platform default theme
//! (no language is hardcoded — the `use_context` fallback is `m3_baseline`
//! only when no theme is threaded at all, e.g. a bare test).
//!
//! # Deep links
//!
//! The router is wired to the process-wide deep-link source via
//! [`router_with_deep_links`]; a link lands on any route, including
//! `/member/:id`. The nav playground's "simulate deep link" button drives the
//! desktop dev seam ([`forgekit::push_deep_link`]) straight to `/member/7`.
//! `forgekit.toml` carries no `[deeplink]` scheme yet — the platform manifest
//! scheme is task 08's concern (the desktop demo needs none).
//!
//! The `features`/`failure` items stay `pub` so the headless team tests
//! (`tests/team.rs`) can drive [`TeamScreen`] and its controller directly,
//! independent of this shell.

pub mod failure;
pub mod features;

mod controllers;
mod notes_domain;
mod profile_domain;
mod routes;
mod screens;
mod settings_domain;
mod shell;

use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use forgekit::{
    AnyView, Axis, Component, CrossAxisAlignment, DesignLanguage, FlexView, Get,
    NavigatorController, Router, RouterDeepLinks, RwSignal, Theme, any, flexible, inflexible,
    navigator, router_with_deep_links, use_context,
};

use features::team::data::InMemoryTeamRepo;
use features::team::domain::repositories::TeamRepository;
pub use features::team::presentation::{
    TeamController, TeamHandles, TeamScreen, TeamSpy, TeamState,
};

use shell::{Tab, TransitionChoice};

/// Every piece of shell-wide mutable UI state a captured page-builder closure
/// (a navigator/router page is `Fn() -> AnyView`, re-invoked each rebuild with
/// no `&State` argument — see `forgekit_widgets::navigator`'s module docs)
/// must reach through a signal rather than a borrowed field. `Copy` (each
/// field is an [`RwSignal`]) so it captures cheaply into nested closures — the
/// same bundle pattern `examples/catalog`'s `CatalogSignals` establishes.
#[derive(Clone, Copy)]
pub struct ShellSignals {
    /// The bottom navigation's selected destination (see [`Tab`]).
    pub tab: RwSignal<Tab>,
    /// The most recent [`forgekit::PopResult`] payload surfaced by a
    /// push-for-result round trip (the member-detail rename, the nav
    /// playground's result demo), rendered as a "last result" banner.
    pub last_result: RwSignal<Option<String>>,
    /// The member-detail rename field's controlled draft text (the notes/team
    /// draft pattern): a `TextInput`'s canonical value lives here so a
    /// per-frame reconcile does not fight in-progress typing.
    pub member_draft: RwSignal<String>,
    /// The navigator's page-transition preset, driven live by the nav
    /// playground's switcher and applied in [`ShellApp::build`].
    pub transition: RwSignal<TransitionChoice>,
}

/// The showcase app's retained state (spec §5.5's `Component::State`): the
/// deep-link-wired router, the shared [`ShellSignals`] bundle, and the injected
/// team-roster backend the `/` route hosts [`TeamScreen`] with.
pub struct ShellState {
    /// The router plus its deep-link auto-wiring (resolves `/` at startup
    /// unless a cold-start link already arrived). `Rc` so the shell chrome's
    /// button closures — rebuilt fresh every `build` — can cheaply clone a
    /// handle into each `on_press`.
    pub nav: Rc<RouterDeepLinks<ShellState>>,
    /// The shell-wide signal bundle (see [`ShellSignals`]).
    pub signals: ShellSignals,
    /// The team-roster backend, built once in `init` and cloned into the `/`
    /// route's `TeamScreen` (constructor injection — the composition root's
    /// job, exactly as the pre-shell `TeamApp` did it).
    pub repo: Arc<dyn TeamRepository + Send + Sync>,
}

/// The showcase app's root [`Component`] (`Default`, as [`forgekit::app!`]
/// requires): stateless configuration wiring the router + tab shell together.
#[derive(Default)]
pub struct ShellApp;

impl Component for ShellApp {
    type State = ShellState;

    fn init(&self) -> ShellState {
        // A latency + one seeded failure so the roster tab visibly exercises
        // the retry path (the first load attempt fails, the RetryPolicy
        // retries) — the same backend the pre-shell `TeamApp` composed.
        let repo: Arc<dyn TeamRepository + Send + Sync> =
            Arc::new(InMemoryTeamRepo::new(Duration::from_millis(400), 1));

        let signals = ShellSignals {
            tab: RwSignal::new(Tab::default()),
            last_result: RwSignal::new(None),
            member_draft: RwSignal::new(String::new()),
            transition: RwSignal::new(TransitionChoice::default()),
        };

        // The controller is created *before* the route table (so every route's
        // page can pop/push through the same controller the router drives),
        // exactly as `examples/navdemo`'s `build_routes` does.
        let controller = NavigatorController::new();
        let routes = routes::build_routes(controller.clone(), signals, Arc::clone(&repo));
        let router = Router::with_controller(&controller, routes);
        // Resolves the start location once: a cold-start deep link (if any)
        // wins over "/" — see `RouterDeepLinks::new`'s doc.
        let nav = router_with_deep_links(router, "/");

        ShellState {
            nav: Rc::new(nav),
            signals,
            repo,
        }
    }

    fn build(&self, state: &mut ShellState) -> AnyView<ShellState> {
        // Navigates on a new warm deep link; a no-op otherwise (dedup'd) —
        // see `RouterDeepLinks::track`'s doc.
        state.nav.track();

        let signals = state.signals;
        let controller = state.nav.router().controller().clone();
        // A separate clone for the navigator's `initial` closure (only ever
        // invoked on the very first build) so it doesn't fight the
        // `&controller` borrow `navigator()` itself takes.
        let initial_repo = Arc::clone(&state.repo);

        let nav_view = navigator(&controller, move || {
            routes::team_page(Arc::clone(&initial_repo))
        })
        .transition(signals.transition.get().to_spec());

        // The persistent chrome branches on the live design language (task 06
        // wires the settings toggle that swaps it; until then it is the
        // platform default). `use_context` resolves here because `build` runs
        // under the shell's `runtime.with_owner(|| scope.track(..))` — see
        // `docs/ARCHITECTURE.md`'s Frame pipeline and Theme delivery.
        let theme = use_context::<Theme>().unwrap_or_else(Theme::m3_baseline);
        let design: DesignLanguage = theme.design_language;

        let top = shell::top_bar(design, Rc::clone(&state.nav));
        let bottom = shell::bottom_bar(design, Rc::clone(&state.nav), signals.tab);

        any(FlexView::new(
            Axis::Vertical,
            vec![
                inflexible(top),
                flexible(1, any(nav_view)),
                inflexible(bottom),
            ],
        )
        .cross_axis(CrossAxisAlignment::Stretch))
    }
}

// The showcase's sole entry point (spec §5.5/§10): one line binds `ShellApp`
// to all three platforms — the Android JNI exports (`target_os = "android"`
// only), the iOS C-ABI exports (unconditional; self-gated to
// `target_os = "ios"`), and (on desktop) the hidden `__forgekit_main` that
// `main.rs` calls.
forgekit::app!(ShellApp);
