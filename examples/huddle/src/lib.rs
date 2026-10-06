//! `huddle` — Frust's single showcase app (a Slack-style mock team chat).
//!
//! A 4-tab, [`Router`]-driven app: Home (channels + DMs) · Search · Activity ·
//! You (settings). Channel → message feed → thread flow, a settings stack, and
//! a workspace switcher, all over the shared mock dataset in
//! [`data::store`]. Each feature is a `{domain, data, presentation}` slice
//! under [`features`]; [`lib`](self), [`shell`], [`routes`], and [`ui`]
//! remain the shared hub files.
//!
//! # Overlay slot
//!
//! The root is a [`Stack`] whose top layer is the [`ui::toast`] overlay: the
//! toast/snackbar service mounts above every screen, below nothing. The
//! [`ToastController`](ui::toast::ToastController) handle is
//! [`provide_context`]-ed under the root owner so any screen can raise a toast
//! (the undo affordance the swipe actions use).
//!
//! # Theme delivery
//!
//! The shell reads the active [`Theme`] via `use_context::<Theme>()` and
//! branches its Material/Cupertino chrome on `theme.design_language`. The
//! single-call-site `set_app_theme` mechanism lives in
//! [`features::settings`] and is driven by the appearance settings screen.
//!
//! # Deep links
//!
//! The router is wired to the process-wide deep-link source via
//! [`router_with_deep_links`]; a link lands on any route, including
//! `/channel/:id` and `/user/:id`. Deep-link scheme stays `teamdemo`
//! (the platform manifest scheme isn't wired to it yet).

pub mod data;
pub mod failure;
pub mod features;
pub mod ui;

mod routes;
mod shell;

use std::rc::Rc;
use std::sync::Arc;

use frust::{
    Axis, BackHandler, Component, CrossAxisAlignment, DesignLanguage, FlexView,
    NavigatorController, PageTransition, Router, RouterDeepLinks, RwSignal, Stack, Theme,
    TransitionSpec, View, any, attach_back_handler, flexible, inflexible, navigator,
    provide_context, router_with_deep_links, use_context,
};

use features::activity::data::repositories::StoreActivityRepository;
use features::activity::domain::repositories::ActivityRepository;
use features::channels::data::repositories::StoreChannelRepository;
use features::channels::domain::repositories::ChannelRepository;
use features::messages::data::repositories::StoreMessageRepository;
use features::messages::domain::repositories::MessageRepository;
use features::profile::data::repositories::StoreProfileRepository;
use features::profile::domain::repositories::ProfileRepository;
use features::search::data::repositories::StoreSearchRepository;
use features::search::domain::repositories::SearchRepository;
use shell::Tab;
use ui::toast::{ToastController, toast_overlay};

/// The Huddle app's retained state (the framework's `Component::State`): the
/// deep-link-wired router, the selected-tab signal (the bottom bar highlight),
/// and the app-wide toast handle.
pub struct HuddleState {
    /// The router plus its deep-link auto-wiring (resolves `/` at startup unless
    /// a cold-start link already arrived). `Rc` so the bottom bar's closures —
    /// rebuilt fresh every `build` — can cheaply clone a handle into each tap.
    pub nav: Rc<RouterDeepLinks<HuddleState>>,
    /// The Android/gesture back-press ⇄ navigator glue:
    /// wired to the *same* [`NavigatorController`] as [`nav`](Self::nav) (a
    /// clone sharing the same `Rc`-backed depth — see [`BackHandler`]'s docs),
    /// so a platform back press pops whatever page the router pushed and
    /// `frust::handles_back` mirrors the live stack depth. [`track`](BackHandler::track)
    /// is called every `build`, alongside `nav.track()`.
    pub back: BackHandler<HuddleState>,
    /// The bottom navigation's selected destination (see [`shell::Tab`]).
    pub tab: RwSignal<Tab>,
    /// The app-wide toast/snackbar handle, mounted in the shell's overlay slot
    /// and provided to screens via context (see the crate docs).
    pub toasts: ToastController,
}

/// The Huddle app's root [`Component`] (`Default`, as [`frust::app!`]
/// requires): stateless configuration wiring the router + tab shell + overlay
/// together.
#[derive(Default)]
pub struct HuddleApp;

impl Component for HuddleApp {
    type State = HuddleState;

    fn init(&self) -> HuddleState {
        let toasts = ToastController::new();
        // Provide the toast handle under the root owner so every screen can
        // raise a toast via `use_context::<ToastController>()`.
        provide_context(toasts.clone());

        // Composition root: construct each feature's repository ONCE and
        // publish it under the root Owner via context — the same mechanism the
        // `ToastController` above (and `Theme`) use. Pages/controllers recover
        // it with `use_context` inside `use_controller`, so construction stays
        // single-site while reaching every construction site of a page (the
        // route table AND in-screen `nav.push` closures).
        let channel_repo: Arc<dyn ChannelRepository + Send + Sync> =
            Arc::new(StoreChannelRepository::new());
        provide_context(channel_repo);

        let message_repo: Arc<dyn MessageRepository + Send + Sync> =
            Arc::new(StoreMessageRepository::new());
        provide_context(message_repo);

        let activity_repo: Arc<dyn ActivityRepository + Send + Sync> =
            Arc::new(StoreActivityRepository::new());
        provide_context(activity_repo);

        let search_repo: Arc<dyn SearchRepository + Send + Sync> =
            Arc::new(StoreSearchRepository::new());
        provide_context(search_repo);

        let profile_repo: Arc<dyn ProfileRepository + Send + Sync> =
            Arc::new(StoreProfileRepository::new());
        provide_context(profile_repo);

        // The controller is created *before* the route table (so every route's
        // page can push through the same controller the router drives).
        let controller = NavigatorController::new();
        let routes = routes::build_routes(controller.clone());
        let router = Router::with_controller(&controller, routes);
        // Resolves the start location once: a cold-start deep link (if any)
        // wins over "/" — see `RouterDeepLinks::new`'s doc.
        let nav = router_with_deep_links(router, "/");

        // Wired to the same controller (a clone sharing its `Rc`-backed depth
        // — see `HuddleState::back`'s doc) so a platform back press pops
        // whatever the router pushed.
        let back = attach_back_handler(controller);

        HuddleState {
            nav: Rc::new(nav),
            back,
            tab: RwSignal::new(Tab::default()),
            toasts,
        }
    }

    fn build(&self, state: &mut HuddleState) -> impl View<HuddleState> {
        // Consumes a new back press (pop if the stack can) and refreshes
        // `handles_back` from the current depth — see `BackHandler::track`'s
        // docs for the one-rebuild refresh lag. Drawer/sheet-first dismissal
        // on back is a known gap: `BackHandler` has no app-level intercept
        // hook today, so an open sheet/drawer does not yet close on a back
        // press — only the navigator pops.
        state.back.track();
        // Navigates on a new warm deep link; a no-op otherwise (dedup'd).
        state.nav.track();

        let controller = state.nav.router().controller().clone();
        // A separate clone for the navigator's `initial` closure (only ever
        // invoked on the very first build).
        let initial_controller = controller.clone();

        let nav_view = navigator(&controller, move || {
            routes::home_page(initial_controller.clone())
        })
        // Tab roots cross-fade (the shell's tab-switch transition — item 1).
        .transition(TransitionSpec::duration(PageTransition::M3FadeThrough));

        // The persistent chrome branches on the live design language (the
        // appearance settings screen swaps it via `set_app_theme`).
        let theme = use_context::<Theme>().unwrap_or_else(frust_material::baseline);
        let design: DesignLanguage = theme.design_language;

        let bottom = shell::bottom_bar(design, Rc::clone(&state.nav), state.tab);

        // The tab shell: the router-driven navigator above the persistent
        // bottom bar.
        let content = any(FlexView::new(
            Axis::Vertical,
            vec![flexible(1, any(nav_view)), inflexible(bottom)],
        )
        .cross_axis(CrossAxisAlignment::Stretch));

        // The root `Stack`: the tab shell, then the toast overlay on top (the
        // reserved overlay slot — above everything, below nothing).
        any(Stack(vec![content, toast_overlay(state.toasts.clone())]))
    }
}

// The showcase's sole entry point: one line binds `HuddleApp`
// to all three platforms. The `setup` block installs Glyph as the seeded
// default theme (`frust_glyph::install()`) before any shell reads the
// default-theme slot — a shell's own fallback is `Theme::neutral()` now, and
// the appearance settings screen's System/Material3/Cupertino/Glyph toggle
// (`features::settings`, driven by `set_app_theme`) still needs a Glyph
// first-launch default to demonstrate the design-language switch faithfully.
frust::app!(
    HuddleApp,
    setup = {
        frust_glyph::install();
    }
);
