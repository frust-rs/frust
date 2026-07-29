//! Huddle's full route table — a hub file: each page builder lives in its
//! owning feature's `presentation/pages/` module (see `src/README-phase-c.md`
//! for the feature-slice convention this crate follows).
//!
//! Twelve routes covering every destination:
//! `/` (Home tab), `/search`, `/activity`, `/you`, `/channel/:id`,
//! `/thread/:id`, `/user/:id`, `/you/settings`, `/you/settings/notifications`,
//! `/you/settings/appearance`, `/you/settings/about`, and
//! `/workspace-switcher`. Each page builder is a plain
//! `Fn(&RouteParams) -> AnyView<HuddleState>` captured once; the navigator
//! [`controller`](frust::NavigatorController) is cloned into every builder
//! that pushes a nested page.

use frust::{AnyView, NavigatorController, Route, RouteParams};

use crate::HuddleState;
use crate::features::activity::presentation::pages::activity as activity_activity;
use crate::features::channels::presentation::pages::{
    home as channels_home, workspace_drawer as channels_workspace_drawer,
};
use crate::features::messages::presentation::pages::{
    channel_feed as messages_channel_feed, thread as messages_thread,
};
use crate::features::profile::presentation::pages::{
    profile as profile_profile, you as profile_you,
};
use crate::features::search::presentation::pages::search as search_search;
use crate::features::settings::presentation::pages::{
    settings as settings_settings, settings_about as settings_settings_about,
    settings_appearance as settings_settings_appearance,
    settings_notifications as settings_settings_notifications,
};

/// The `/` route (and the navigator's initial page): the Home tab (now a
/// channels-feature page).
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
            search_search::search_screen()
        }),
        Route::new("/activity", |_params: &RouteParams| {
            activity_activity::activity_screen()
        }),
        Route::new("/you", {
            let controller = controller.clone();
            move |_params: &RouteParams| profile_you::you_screen(controller.clone())
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
            profile_profile::profile_screen(id)
        }),
        Route::new("/you/settings", {
            let controller = controller.clone();
            move |_params: &RouteParams| settings_settings::settings_screen(controller.clone())
        }),
        Route::new("/you/settings/notifications", |_params: &RouteParams| {
            settings_settings_notifications::notifications_screen()
        }),
        Route::new("/you/settings/appearance", |_params: &RouteParams| {
            settings_settings_appearance::appearance_screen()
        }),
        Route::new("/you/settings/about", |_params: &RouteParams| {
            settings_settings_about::about_screen()
        }),
        // Transparent push (top drawer) — see `workspace_drawer`'s transition
        // note; the skeleton routes it through the normal navigator transition.
        Route::new("/workspace-switcher", |_params: &RouteParams| {
            channels_workspace_drawer::workspace_drawer_screen()
        }),
    ]
}
