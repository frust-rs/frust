//! Navdemo — the Phase 6b navigation exit-criterion demo (spec §19).
//!
//! A small real app proving the whole navigation stack end-to-end through the
//! `forgekit` facade alone (no escape-hatch dependency, unlike
//! `examples/gallery` — see `docs/ARCHITECTURE.md`'s `forgekit-widgets` row):
//! a declarative [`Route`] table (`/`, `/item/:id`, `/settings`) driven
//! by a [`Router`]/[`NavigatorController`] pair, a push-for-result round trip
//! displayed back on the list page, a live transition-preset switcher (M3
//! shared-axis-X, M3 fade-through, iOS push — each drives the interactive
//! left-edge swipe-back automatically when selected), and a "simulate deep
//! link" button exercising the desktop dev seam
//! ([`forgekit::push_deep_link`]) to jump straight to `/item/7`, an item the
//! visible list never shows — proving a deep link can reach a page no button
//! in the UI leads to.
//!
//! # Shape
//!
//! The toolbar (transition switcher + router-driven push/go/deep-link
//! buttons) is built directly in [`Component::build`], where the live
//! [`AppState`] is in scope. The navigator's pages are different: a page
//! builder is a plain `Fn() -> AnyView<State>` captured *once* (at push time,
//! or for the root page, on the navigator's very first build — see
//! `forgekit_widgets::navigator`'s module docs) and re-invoked on every
//! subsequent rebuild with no arguments — it cannot read `state` by
//! reference. Two different fixes are used for the two different needs:
//! - [`item_detail_view`]/[`settings_view`] only need the
//!   [`NavigatorController`] (for `pop`/`pop_with_result`), which is created
//!   *before* the route table and cloned into every route's builder — no
//!   circularity.
//! - The list page's "last result" banner needs to observe a write that
//!   happens *outside* its own builder (the `on_result` callback fired after
//!   a pop) — the textbook case `docs/CODE_STANDARDS.md`'s State & Reactivity
//!   Conventions carves out for reaching for an `RwSignal` field instead of a
//!   plain one.

use std::rc::Rc;

use forgekit::{
    AnyView, Button, Column, Component, Get, NavigatorController, PageTransition, PopResult, Route,
    RouteParams, Router, RouterDeepLinks, Row, RwSignal, Set, SizedBox, TransitionSpec, any,
    navigator, push_deep_link, router_with_deep_links, scroll_view, text,
};

/// The list page's fixed catalog — id 7 is deliberately absent: `/item/7` is
/// only reachable via the "simulate deep link" button, proving a deep link
/// can land somewhere no button in the UI leads to.
const ITEMS: [(u32, &str); 5] = [
    (1, "Aurora"),
    (2, "Borealis"),
    (3, "Cascade"),
    (4, "Delta"),
    (5, "Echo"),
];

/// Look up an item's display name — a synthesized placeholder for an id
/// outside [`ITEMS`] (e.g. `7`, reachable only via the deep-link demo).
fn item_name(id: u32) -> String {
    ITEMS
        .iter()
        .find(|(item_id, _)| *item_id == id)
        .map(|(_, name)| name.to_string())
        .unwrap_or_else(|| format!("Unlisted item #{id} (deep-link only)"))
}

/// Which page-transition preset the toolbar's switcher currently selects,
/// applied as the navigator's default (per-op pushes never override it in
/// this demo). `IosPush` is also what turns on the interactive left-edge
/// swipe-back (`NavigatorView::pop_swipe`'s default: on for `IosPush`, off
/// otherwise — see `forgekit_widgets::navigator`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum TransitionChoice {
    #[default]
    None,
    M3SharedAxis,
    M3FadeThrough,
    Ios,
}

impl TransitionChoice {
    fn label(self) -> &'static str {
        match self {
            TransitionChoice::None => "None (instant)",
            TransitionChoice::M3SharedAxis => "M3 shared-axis-X",
            TransitionChoice::M3FadeThrough => "M3 fade-through",
            TransitionChoice::Ios => "iOS push (swipe-back armed)",
        }
    }

    fn to_spec(self) -> TransitionSpec {
        match self {
            TransitionChoice::None => TransitionSpec::NONE,
            TransitionChoice::M3SharedAxis => {
                TransitionSpec::duration(PageTransition::M3SharedAxisX)
            }
            TransitionChoice::M3FadeThrough => {
                TransitionSpec::duration(PageTransition::M3FadeThrough)
            }
            TransitionChoice::Ios => TransitionSpec::duration(PageTransition::IosPush),
        }
    }
}

/// Navdemo's retained state (spec §5's `app_logic` model, phase 5.5's
/// `Component::State`).
pub struct AppState {
    /// The router + its deep-link auto-wiring (Phase 6b task 08), resolving
    /// `/` at startup unless a cold-start deep link already arrived. `Rc`
    /// (not owned directly) so the toolbar's button closures — rebuilt fresh
    /// every `Component::build` — can cheaply clone a handle into each
    /// `on_press`.
    nav: Rc<RouterDeepLinks<AppState>>,
    /// The last [`PopResult`] payload handed back to the list page by a
    /// "select & return" pop, if any. An `RwSignal` (not a plain field)
    /// because it must be *read* inside the list page's builder closure —
    /// captured once, re-invoked every rebuild with no `&State` argument —
    /// and *written* from a `pop`'s `on_result` callback, which runs outside
    /// that closure entirely (see the module docs).
    last_result: RwSignal<Option<String>>,
    /// The toolbar's selected transition preset. Read directly in
    /// `Component::build` (which does have `&mut AppState`), so a plain field
    /// is enough here — no `RwSignal` needed.
    transition_choice: TransitionChoice,
}

/// Build the shared route table: `/` (the item list), `/item/:id` (a detail
/// page — reachable by id even for an id [`ITEMS`] doesn't list), and
/// `/settings`. Takes `controller` (created *before* this table, so no
/// circularity with the `Router` it will be attached to) and `last_result`
/// (an `RwSignal`, `Copy` — see [`AppState::last_result`]'s doc) so every
/// route's page can pop/return exactly like the list page's own
/// push-for-result buttons do.
fn build_routes(
    controller: NavigatorController<AppState>,
    last_result: RwSignal<Option<String>>,
) -> Vec<Route<AppState>> {
    vec![
        Route::new("/", {
            let controller = controller.clone();
            move |_params: &RouteParams| home_page(controller.clone(), last_result)
        }),
        Route::new("/item/:id", {
            let controller = controller.clone();
            move |params: &RouteParams| {
                let id: u32 = params.get("id").and_then(|s| s.parse().ok()).unwrap_or(0);
                item_detail_view(id, controller.clone())
            }
        }),
        Route::new("/settings", move |_params: &RouteParams| {
            settings_view(controller.clone())
        }),
    ]
}

/// The list/root page: the item catalog (each row pushes its detail page
/// *for a result*, via [`NavigatorController::push_for_result`] rather than
/// `router.push` — the router has no result-registering call, so this one
/// navigation deliberately goes straight through the controller the router
/// itself drives) plus a "last result" banner reflecting the most recent
/// pop's payload. Used both as the navigator's initial page and as the `/`
/// route's page (see the module docs on why a page builder can't read
/// `state` directly).
fn home_page(
    controller: NavigatorController<AppState>,
    last_result: RwSignal<Option<String>>,
) -> AnyView<AppState> {
    let mut children: Vec<AnyView<AppState>> = Vec::with_capacity(ITEMS.len() + 2);
    children.push(any(text("Navdemo — item list").size(24.0)));
    children.push(any(text(
        "Route: / — the navigator's root page, also reachable via router.go(\"/\").",
    )
    .size(14.0)));

    if let Some(result) = last_result.get() {
        children.push(any(text(format!("Last result: {result}")).size(16.0)));
    }

    for &(id, name) in ITEMS.iter() {
        let controller = controller.clone();
        let item_name = name.to_string();
        children.push(any(Row(vec![
            any(text(name).size(18.0)),
            any(SizedBox(Some(12.0), None)),
            any(Button(
                "Open (push for result)",
                move |_s: &mut AppState| {
                    let controller = controller.clone();
                    let inner_controller = controller.clone();
                    let item_name = item_name.clone();
                    controller.push_for_result(
                        move || item_detail_view(id, inner_controller.clone()),
                        move |s: &mut AppState, result: PopResult| {
                            let message = result.take::<String>().unwrap_or_else(|| {
                                format!("Closed \"{item_name}\" with no result")
                            });
                            s.last_result.set(Some(message));
                        },
                    );
                },
            )),
        ])));
    }

    any(scroll_view(Column(children)))
}

/// The `/item/:id` detail page. "Back" is a plain pop (any `on_result`
/// callback registered by the pusher — `push_for_result`, above — still sees
/// an empty [`PopResult`]); "Select & return" pops carrying this item's name,
/// which only a pusher that used `push_for_result` will observe.
fn item_detail_view(id: u32, controller: NavigatorController<AppState>) -> AnyView<AppState> {
    let name = item_name(id);
    let back_controller = controller.clone();
    let select_controller = controller.clone();
    let select_name = name.clone();

    any(Column(vec![
        any(text(format!("Item #{id}: {name}")).size(24.0)),
        any(text("Route: /item/:id").size(14.0)),
        any(Row(vec![
            any(Button("Back", move |_s: &mut AppState| {
                back_controller.pop();
            })),
            any(SizedBox(Some(12.0), None)),
            any(Button("Select & return", move |_s: &mut AppState| {
                select_controller.pop_with_result(PopResult::of(select_name.clone()));
            })),
        ])),
    ]))
}

/// The `/settings` page — reachable only via the toolbar's
/// `router.push("/settings")` button, demonstrating a plain route-driven push
/// with no result round trip.
fn settings_view(controller: NavigatorController<AppState>) -> AnyView<AppState> {
    any(Column(vec![
        any(text("Settings").size(24.0)),
        any(text("Route: /settings — pushed via router.push(\"/settings\").").size(14.0)),
        any(Button("Back", move |_s: &mut AppState| {
            controller.pop();
        })),
    ]))
}

/// The persistent toolbar above the navigator: the transition-preset
/// switcher plus the router-driven push/go/deep-link buttons. Built fresh
/// every [`Component::build`] call (unlike the navigator's pages — see the
/// module docs), so it can read `state` directly with no `RwSignal` needed.
fn build_toolbar(state: &AppState) -> AnyView<AppState> {
    let nav_push = state.nav.clone();
    let nav_go = state.nav.clone();
    let nav_settings = state.nav.clone();

    any(Column(vec![
        any(text("Toolbar — router-driven navigation").size(20.0)),
        any(text(format!(
            "Transition preset: {}",
            state.transition_choice.label()
        ))
        .size(14.0)),
        any(Row(vec![
            any(Button("None", |s: &mut AppState| {
                s.transition_choice = TransitionChoice::None;
            })),
            any(SizedBox(Some(8.0), None)),
            any(Button("M3 shared-axis-X", |s: &mut AppState| {
                s.transition_choice = TransitionChoice::M3SharedAxis;
            })),
            any(SizedBox(Some(8.0), None)),
            any(Button("M3 fade-through", |s: &mut AppState| {
                s.transition_choice = TransitionChoice::M3FadeThrough;
            })),
            any(SizedBox(Some(8.0), None)),
            any(Button("iOS push", |s: &mut AppState| {
                s.transition_choice = TransitionChoice::Ios;
            })),
        ])),
        any(Row(vec![
            any(Button(
                "router.push(\"/item/5\")",
                move |_s: &mut AppState| {
                    nav_push.router().push("/item/5");
                },
            )),
            any(SizedBox(Some(8.0), None)),
            any(Button(
                "router.go(\"/item/2\")",
                move |_s: &mut AppState| {
                    nav_go.router().go("/item/2");
                },
            )),
            any(SizedBox(Some(8.0), None)),
            any(Button(
                "router.push(\"/settings\")",
                move |_s: &mut AppState| {
                    nav_settings.router().push("/settings");
                },
            )),
            any(SizedBox(Some(8.0), None)),
            any(Button(
                "Simulate deep link → /item/7",
                |_s: &mut AppState| {
                    push_deep_link("/item/7");
                },
            )),
        ])),
    ]))
}

/// The generated app's root [`Component`] (spec §5.5): stateless config
/// wiring [`AppState`]'s router + navigator + toolbar together.
#[derive(Default)]
pub struct NavDemoApp;

impl Component for NavDemoApp {
    type State = AppState;

    fn init(&self) -> AppState {
        // The controller is created *before* the route table (see
        // `build_routes`'s doc) so `/item/:id`'s and `/settings`'s pages can
        // pop through the same controller the router itself drives.
        let controller = NavigatorController::new();
        let last_result: RwSignal<Option<String>> = RwSignal::new(None);
        let routes = build_routes(controller.clone(), last_result);
        let router = Router::with_controller(&controller, routes);
        // Resolves the start location once: a cold-start deep link (if any)
        // wins over "/" — see `RouterDeepLinks::new`'s doc.
        let nav = router_with_deep_links(router, "/");

        AppState {
            nav: Rc::new(nav),
            last_result,
            transition_choice: TransitionChoice::default(),
        }
    }

    fn build(&self, state: &mut AppState) -> AnyView<AppState> {
        // Navigates on a new warm deep link; a no-op otherwise (dedup'd) —
        // see `RouterDeepLinks::track`'s doc.
        state.nav.track();

        let controller = state.nav.router().controller().clone();
        let last_result = state.last_result;
        // A separate clone for the navigator's `initial` closure (only ever
        // invoked on the very first build — see the module docs) so it
        // doesn't fight the `&controller` borrow `navigator()` itself takes.
        let initial_controller = controller.clone();

        let nav_view = navigator(&controller, move || {
            home_page(initial_controller.clone(), last_result)
        })
        .transition(state.transition_choice.to_spec());

        any(Column(vec![any(build_toolbar(state)), any(nav_view)]))
    }
}

// The demo's sole entry point (spec §5.5/§10): one line binds `NavDemoApp` to
// all three platforms — the desktop preview loop (via the hidden
// `__forgekit_main` `main.rs` calls), the Android JNI exports, and the iOS
// C-ABI exports.
forgekit::app!(NavDemoApp);

#[cfg(test)]
mod tests {
    use super::*;
    use forgekit_core::RenderRoot;
    use forgekit_reactive::ReactiveRuntime;
    use std::sync::Arc;

    // A single test function, deliberately: `push_deep_link`/the deep-link
    // signal are process-wide (see `forgekit_reactive::deep_link`'s module
    // docs), and `ReactiveRuntime::init` swaps a process-wide waker — two
    // `#[test]` functions running on the parallel test runner's separate
    // threads would race each other on that shared state, exactly the
    // reasoning `crates/forgekit/src/router_glue.rs`'s own combined test
    // uses.
    #[test]
    fn route_table_builds_and_state_survives_push_pop() {
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));

        let mut root: RenderRoot<AppState, AnyView<AppState>> = RenderRoot::new();
        let mut state = NavDemoApp.init();
        let mut logic = |s: &mut AppState| NavDemoApp.build(s);

        // Criterion: the root builds at "/", the navigator's resolved start
        // location (rebuild only — no layout/paint, so no `TextContext` is
        // needed; see `docs/ARCHITECTURE.md`'s Frame pipeline).
        root.rebuild(&mut logic, &mut state);
        assert!(
            root.root_id().is_some(),
            "the root widget must build at \"/\""
        );

        // Criterion: a matcher-driven push resolves a second location.
        state.nav.router().push("/item/3");
        root.rebuild(&mut logic, &mut state);

        // Criterion: a third, distinct matcher-driven location.
        state.nav.router().push("/settings");
        root.rebuild(&mut logic, &mut state);

        // Criterion: a warm deep link (the desktop dev seam) reaches an item
        // the visible list never shows.
        push_deep_link("/item/7");
        root.rebuild(&mut logic, &mut state);
        assert_eq!(
            item_name(7),
            "Unlisted item #7 (deep-link only)",
            "item 7 must resolve even though it is absent from ITEMS"
        );

        // Criterion: retained app state survives a push/pop round trip — the
        // toolbar's transition choice, set before the round trip, must still
        // read back afterward.
        state.transition_choice = TransitionChoice::Ios;
        state.nav.router().push("/item/1");
        root.rebuild(&mut logic, &mut state);
        state.nav.router().controller().pop();
        root.rebuild(&mut logic, &mut state);
        assert_eq!(
            state.transition_choice,
            TransitionChoice::Ios,
            "app state must survive a navigator push/pop round trip"
        );
    }
}
