//! The showcase's full route table (finalized here in task 02; wave-2 tasks
//! fill their own screen files, never this one).
//!
//! Thirteen routes covering every destination in the plan:
//! `/`, `/member/:id`, `/profile`, `/widgets/{controls,cards,modals,appbars}`,
//! `/showcase`, `/theme`, `/motion`, `/notes`, `/nav`, and `/settings`. Each
//! route's page builder is a plain `Fn(&RouteParams) -> AnyView<ShellState>`
//! captured once (see `forgekit_widgets::navigator`'s module docs) — the
//! controller is cloned into every builder that needs to pop/push, exactly as
//! `examples/navdemo`'s `build_routes` wires it.

use std::sync::Arc;

use forgekit::{AnyView, NavigatorController, Route, RouteParams, any, component};

use crate::ShellSignals;
use crate::ShellState;
use crate::features::team::domain::repositories::TeamRepository;
use crate::features::team::presentation::TeamScreen;
use crate::screens;

/// The `/` route (and the navigator's initial page): the async team roster,
/// hosted as a [`TeamScreen`] [`Component`](forgekit::Component) so its `init`
/// runs under a real component reactive `Owner` (controller-disposal-on-teardown
/// and the failure-listener subscription both scope to it). `repo` is the
/// injected backend the composition root built once.
pub fn team_page(repo: Arc<dyn TeamRepository + Send + Sync>) -> AnyView<ShellState> {
    any(component(TeamScreen::new(repo)))
}

/// Build the full route table. `controller` is created *before* this table (so
/// no circularity with the [`Router`](forgekit::Router) it will be attached
/// to); `signals` is the shell-wide `Copy` bundle every stateful page reads;
/// `repo` is cloned into the `/` route's `TeamScreen`.
pub fn build_routes(
    controller: NavigatorController<ShellState>,
    signals: ShellSignals,
    repo: Arc<dyn TeamRepository + Send + Sync>,
) -> Vec<Route<ShellState>> {
    vec![
        Route::new("/", {
            let repo = Arc::clone(&repo);
            move |_params: &RouteParams| team_page(Arc::clone(&repo))
        }),
        Route::new("/member/:id", {
            let controller = controller.clone();
            move |params: &RouteParams| {
                let id = params.get("id").cloned().unwrap_or_default();
                screens::member_detail::member_detail(id, controller.clone(), signals)
            }
        }),
        Route::new("/profile", |_params: &RouteParams| {
            screens::profile::profile_screen()
        }),
        Route::new("/widgets/controls", |_params: &RouteParams| {
            screens::widgets_controls::controls_screen()
        }),
        Route::new("/widgets/cards", |_params: &RouteParams| {
            screens::widgets_cards::cards_screen()
        }),
        Route::new("/widgets/modals", |_params: &RouteParams| {
            screens::widgets_modals::modals_screen()
        }),
        Route::new("/widgets/appbars", |_params: &RouteParams| {
            screens::widgets_appbars::appbars_screen()
        }),
        Route::new("/showcase", {
            let controller = controller.clone();
            move |_params: &RouteParams| {
                screens::showcase_menu::showcase_menu(controller.clone(), signals)
            }
        }),
        Route::new("/theme", |_params: &RouteParams| {
            screens::theme::theme_screen()
        }),
        Route::new("/motion", |_params: &RouteParams| {
            screens::motion::motion_screen()
        }),
        Route::new("/notes", |_params: &RouteParams| {
            screens::notes::notes_screen()
        }),
        Route::new("/nav", {
            let controller = controller.clone();
            move |_params: &RouteParams| {
                screens::nav_playground::nav_playground(controller.clone(), signals)
            }
        }),
        Route::new("/settings", |_params: &RouteParams| {
            screens::settings::settings_screen()
        }),
    ]
}
