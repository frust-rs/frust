//! Huddle's full route table — a **hub file finalized in the skeleton (task
//! 10)**. Phase C screen tasks fill their own `screens/<name>.rs`, never this
//! file (see `src/README-phase-c.md`).
//!
//! Twelve routes covering every destination in the plan:
//! `/` (Home tab), `/search`, `/activity`, `/you`, `/channel/:id`,
//! `/thread/:id`, `/user/:id`, `/you/settings`, `/you/settings/notifications`,
//! `/you/settings/appearance`, `/you/settings/about`, and
//! `/workspace-switcher`. Each page builder is a plain
//! `Fn(&RouteParams) -> AnyView<HuddleState>` captured once; the navigator
//! [`controller`](frust::NavigatorController) is cloned into every builder
//! that pushes a nested page.

use frust::{AnyView, NavigatorController, Route, RouteParams};

use crate::HuddleState;
use crate::features::channels::presentation::pages::{
    home as channels_home, workspace_drawer as channels_workspace_drawer,
};
use crate::features::messages::presentation::pages::{
    channel_feed as messages_channel_feed, thread as messages_thread,
};
use crate::screens;

/// The `/` route (and the navigator's initial page): the Home tab (now a
/// channels-feature page — huddle clean-architecture refactor, task 02).
pub fn home_page(controller: NavigatorController<HuddleState>) -> AnyView<HuddleState> {
    channels_home::home_screen(controller)
}

/// Build the full route table. `controller` is created *before* this table (so
/// there is no circularity with the [`Router`](frust::Router) it attaches
/// to) and cloned into every builder that pushes a nested page.
pub fn build_routes(controller: NavigatorController<HuddleState>) -> Vec<Route<HuddleState>> {
    vec![
        Route::new("/", {
            let controller = controller.clone();
            move |_params: &RouteParams| home_page(controller.clone())
        }),
        Route::new("/search", |_params: &RouteParams| {
            screens::search::search_screen()
        }),
        Route::new("/activity", |_params: &RouteParams| {
            screens::activity::activity_screen()
        }),
        Route::new("/you", {
            let controller = controller.clone();
            move |_params: &RouteParams| screens::you::you_screen(controller.clone())
        }),
        Route::new("/channel/:id", {
            let controller = controller.clone();
            move |params: &RouteParams| {
                let id = params.get("id").cloned().unwrap_or_default();
                messages_channel_feed::channel_feed(controller.clone(), id)
            }
        }),
        Route::new("/thread/:id", |params: &RouteParams| {
            let id = params.get("id").cloned().unwrap_or_default();
            messages_thread::thread_screen(id)
        }),
        Route::new("/user/:id", |params: &RouteParams| {
            let id = params.get("id").cloned().unwrap_or_default();
            screens::profile::profile_screen(id)
        }),
        Route::new("/you/settings", {
            let controller = controller.clone();
            move |_params: &RouteParams| screens::settings::settings_screen(controller.clone())
        }),
        Route::new("/you/settings/notifications", |_params: &RouteParams| {
            screens::settings_notifications::notifications_screen()
        }),
        Route::new("/you/settings/appearance", |_params: &RouteParams| {
            screens::settings_appearance::appearance_screen()
        }),
        Route::new("/you/settings/about", |_params: &RouteParams| {
            screens::settings_about::about_screen()
        }),
        // Transparent push (top drawer) — see `workspace_drawer`'s transition
        // note; the skeleton routes it through the normal navigator transition.
        Route::new("/workspace-switcher", |_params: &RouteParams| {
            channels_workspace_drawer::workspace_drawer_screen()
        }),
    ]
}
