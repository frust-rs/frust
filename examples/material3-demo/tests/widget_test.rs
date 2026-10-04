//! Host-side port of the reference example's `test/widget_test.dart` — three
//! cases: "Example app renders the gallery shell", "Every section list
//! renders", "Do opens Buttons playground".
//!
//! # Why this isn't a literal port
//!
//! Every fixture below is built fresh from `frust`'s public surface
//! (`Router`/`Route`/`shell_route`/`NavigatorController`) plus
//! `frust_material`'s widget constructors, reproducing the *shape* of
//! `src/lib.rs`'s own route table — sections declared as children of a
//! pathless shell route, a `/playground/:id` route declared ahead of it so
//! the shell's own empty-segment match can't shadow it — rather than
//! depending on it. The section paths/labels and the one playground id below
//! mirror `src/catalog/section.rs`/`src/catalog/entries.rs` as read at
//! implementation time; they are not imported from there.
//!
//! Reproduction is a choice, not a constraint. The crate ships an `rlib`
//! (`Cargo.toml`'s `[lib] crate-type`, needed anyway for the desktop preview
//! binary), so `material3demo::` *is* linkable from here — but `routes` and
//! the section/entry tables it reads are crate-private, and widening them to
//! `pub` purely to let a test observe them would export app internals as API.
//! Rebuilding the same shape from the public surface pins the router
//! contract this app depends on without that.
//!
//! # What's pinned vs. what's out of reach
//!
//! Every case below pins what a host-side test can actually observe: route
//! resolution (chain length, matched params) and that each resolved page's
//! view tree *constructs* without panicking — the same "does it build"
//! contract `theme_config_page.rs`'s own in-crate tests already lean on.
//! None of them can search on-screen text: this crate declares no
//! `dev-dependencies` (deliberately out of scope for this file to add), so
//! there is no `RenderRoot`/paint-recording harness reachable from outside
//! the crate to walk a widget tree and assert a literal string appears. The
//! Dart suite's on-screen text assertions (the title, "Do"/"Buttons")
//! therefore have no host-side equivalent here and are left to a real
//! window/device check instead.
//!
//! Also out of scope for a route-table reproduction: mounting a
//! [`frust::navigator`] itself. A view that does can't build outside a
//! running `ReactiveRuntime` (back-glue wiring panics — see `src/lib.rs`'s
//! own test-module doc), so the shell page below composes the same chrome
//! `gallery_shell` does (`frust_material::{app_bar, navigation_bar,
//! nav_item}`) without mounting an inner navigator.

use frust::{
    AnyView, NavigatorController, Resolution, Route, RouteParams, Router, SizedBox, any, safe_area,
    scaffold, shell_route, text,
};
use frust_material::{app_bar, card_list_items, list_item, nav_item, navigation_bar};

/// This app's test state is never read by anything below — no callback here
/// needs to reach into it.
type St = ();

/// The five gallery sections' paths and nav-bar labels, in nav-bar order —
/// mirrors `DemoSection::ALL`/`DemoSection::path`/`DemoSection::nav_label`
/// (`src/catalog/section.rs`).
const SECTIONS: [(&str, &str); 5] = [
    ("/do", "Do"),
    ("/pick", "Pick"),
    ("/view", "View"),
    ("/nav", "Nav"),
    ("/find", "Find"),
];

/// The one catalog entry this file exercises through `/playground/:id` —
/// mirrors the real `buttons` entry under the Do section
/// (`src/pages/playground/do_/buttons.rs`), the Dart suite's own "Buttons"
/// playground.
const ENTRY_ID: &str = "buttons";

/// The gallery shell's chrome — a top app bar plus a bottom navigation bar,
/// the same two `frust_material` constructors `gallery_shell` composes in
/// `src/lib.rs`. Its content slot is a placeholder rather than a mounted
/// inner navigator (see this module's docs).
fn shell_page(_params: &RouteParams) -> AnyView<St> {
    let bar = app_bar::<St>("Material 3 Expressive");
    let nav_bar = navigation_bar(
        SECTIONS
            .iter()
            .map(|(_, label)| nav_item::<St>(*label))
            .collect(),
        0,
        |_: &mut St, _: usize| {},
    );
    any(scaffold(any(SizedBox::<St>(None, None)))
        .app_bar(any(bar))
        .bottom_bar(any(safe_area(nav_bar).top(false))))
}

/// One section's list content — a single-row card list, mirroring
/// `section_host`'s narrow-layout list shape closely enough to exercise the
/// same `frust_material` list constructors it uses.
fn section_content(_params: &RouteParams) -> AnyView<St> {
    any(card_list_items(vec![list_item::<St>("Buttons")]))
}

/// The playground route's page — no chrome, just enough content to prove the
/// route's builder constructs (mirroring `playground_route`'s shape: a page
/// keyed by the matched `:id`).
fn playground_route(params: &RouteParams) -> AnyView<St> {
    let id = params.get("id").map(String::as_str).unwrap_or("");
    any(scaffold(any(text(id.to_string()))))
}

/// The route table this file reproduces: the playground route declared
/// ahead of the pathless shell route (so the shell's empty-segment match
/// can't shadow it — `shell_route`'s own ordering contract), the shell
/// route carrying every section as a child.
fn build_router() -> Router<St> {
    let inner = NavigatorController::<St>::new();
    let sections: Vec<Route<St>> = SECTIONS
        .iter()
        .map(|(path, _label)| Route::new(*path, section_content))
        .collect();
    Router::new(vec![
        Route::new("/playground/:id", playground_route),
        shell_route(&inner, shell_page, sections),
    ])
}

/// Ports "Example app renders the gallery shell": the app's initial route
/// (its first section) resolves through the shell plus its own page, and
/// both pages' views construct without panicking.
#[test]
fn app_renders_the_gallery_shell() {
    let router = build_router();
    match router.resolve(SECTIONS[0].0) {
        Resolution::Matched { pages, .. } => {
            assert_eq!(
                pages.len(),
                2,
                "the initial section route must resolve through the shell plus its own page"
            );
            for page in &pages {
                let _view = page.build();
            }
        }
        Resolution::Error { location } => {
            panic!("the initial section route did not match: {location:?}")
        }
    }
}

/// Ports "Every section list renders": every section's route resolves
/// through the shell, and every resolved page's view constructs.
#[test]
fn every_section_list_renders() {
    let router = build_router();
    for (path, label) in SECTIONS {
        match router.resolve(path) {
            Resolution::Matched { pages, .. } => {
                assert_eq!(
                    pages.len(),
                    2,
                    "{label} ({path}) should resolve through the shell route"
                );
                for page in &pages {
                    let _view = page.build();
                }
            }
            Resolution::Error { location } => {
                panic!("{label} ({path}) did not match: {location:?}")
            }
        }
    }
}

/// Ports "Do opens Buttons playground": the Buttons entry's playground route
/// resolves outside the shell, carrying the matched `id`, and its view
/// constructs.
#[test]
fn do_opens_buttons_playground() {
    let router = build_router();
    let location = format!("/playground/{ENTRY_ID}");
    match router.resolve(&location) {
        Resolution::Matched { pages, params, .. } => {
            assert_eq!(
                pages.len(),
                1,
                "a playground route covers the shell, not nests in it"
            );
            assert_eq!(params.get("id").map(String::as_str), Some(ENTRY_ID));
            let _view = pages[0].build();
        }
        Resolution::Error { location } => panic!("{location:?} did not match"),
    }
}
