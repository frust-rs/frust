//! Route × theme matrix smoke test (task 07, group (a)): every showcase route
//! must build under both design languages and both brightness states.
//!
//! Deliberately one `#[test]` iterating the whole matrix, rather than one test
//! per combination: `ReactiveRuntime::init` installs a process-wide runtime
//! (idempotent, but still one shared instance per test binary — see
//! `forgekit_reactive::ReactiveRuntime::init`'s doc) and `forgekit::provide_context`
//! writes to the ambient reactive `Owner`'s context map, so keeping the whole
//! sweep in one test avoids any ordering assumption between parallel `#[test]`
//! threads over that shared state (the same reasoning `examples/navdemo`'s own
//! single-test-per-file convention documents, applied here to a single test
//! function instead of a whole file since nothing else in this binary touches
//! `provide_context`).
//!
//! Rebuild only — no layout/paint, so no `TextContext` is needed (mirrors
//! `examples/navdemo`'s ported test in `tests/ported.rs`).

use forgekit::{AnyView, Brightness, Component, DesignLanguage, Theme};
use forgekit_core::RenderRoot;

use huddle::{ShellApp, ShellState};

mod support;
use support::setup;

type Root = RenderRoot<ShellState, AnyView<ShellState>>;

/// Every route the showcase's route table declares (`routes.rs`'s module doc:
/// "Thirteen routes covering every destination in the plan") — kept as a
/// literal list here since `routes`/`routes::build_routes` is a private module
/// this external test crate cannot import. `/member/:id` is exercised via a
/// concrete id.
const ROUTES: [&str; 13] = [
    "/",
    "/member/1",
    "/profile",
    "/widgets/controls",
    "/widgets/cards",
    "/widgets/modals",
    "/widgets/appbars",
    "/showcase",
    "/theme",
    "/motion",
    "/notes",
    "/nav",
    "/settings",
];

#[test]
fn every_route_builds_under_both_design_languages_and_brightness_states() {
    let _ambient = setup();

    let combos = [
        (DesignLanguage::Material3, Brightness::Light),
        (DesignLanguage::Material3, Brightness::Dark),
        (DesignLanguage::Cupertino, Brightness::Light),
        (DesignLanguage::Cupertino, Brightness::Dark),
    ];

    for (design, brightness) in combos {
        let theme = match design {
            DesignLanguage::Material3 => Theme::m3_baseline(),
            DesignLanguage::Cupertino => Theme::cupertino_baseline(),
        }
        .with_brightness(brightness);
        forgekit::provide_context(theme);

        // A fresh mount per combo keeps the navigator's page stack from
        // compounding across 13 routes × 4 combos — `go` resets the stack to
        // each route in turn, but starting clean per combo keeps this test's
        // failure output attributable to one (design, brightness, route)
        // triple.
        let mut root: Root = RenderRoot::new();
        let mut state = ShellApp.init();
        let mut logic = |s: &mut ShellState| ShellApp.build(s);

        // The initial build resolves "/" (or a cold-start deep link, none
        // pending here) before any `go` call below.
        root.rebuild(&mut logic, &mut state);
        assert!(
            root.root_id().is_some(),
            "the shell must build at its start location under {design:?}/{brightness:?}"
        );

        for route in ROUTES {
            state.nav.router().go(route);
            root.rebuild(&mut logic, &mut state);
            assert!(
                root.root_id().is_some(),
                "route {route:?} must build under {design:?}/{brightness:?}"
            );
        }
    }
}
