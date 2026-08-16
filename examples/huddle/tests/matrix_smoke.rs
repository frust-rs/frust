//! Route × theme matrix smoke test: every Huddle route must build under every
//! design language and both brightness states.
//!
//! Deliberately one `#[test]` iterating the whole matrix, rather than one test
//! per combination: `frust::provide_context` writes to the ambient reactive
//! `Owner`'s context map, so keeping the whole sweep in one test avoids any
//! ordering assumption between parallel `#[test]` threads over that state.
//!
//! Rebuild only — no layout/paint, so no `TextContext` is needed.

use frust::{AnyView, Brightness, Component, DesignLanguage, Theme};
use frust_core::RenderRoot;

use huddle::{HuddleApp, HuddleState};

mod support;
use support::setup;

type Root = RenderRoot<HuddleState, AnyView<HuddleState>>;

/// Every route the Huddle route table declares (`routes.rs`'s module doc:
/// "Twelve routes covering every destination in the plan") — kept as a literal
/// list here since `routes`/`routes::build_routes` is a private module this
/// external test crate cannot import. `:id` routes are exercised via concrete
/// ids.
const ROUTES: [&str; 12] = [
    "/",
    "/search",
    "/activity",
    "/you",
    "/channel/general",
    "/thread/1",
    "/user/1",
    "/you/settings",
    "/you/settings/notifications",
    "/you/settings/appearance",
    "/you/settings/about",
    "/workspace-switcher",
];

#[test]
fn every_route_builds_under_both_design_languages_and_brightness_states() {
    let _ambient = setup();

    let combos = [
        (DesignLanguage::Material3, Brightness::Light),
        (DesignLanguage::Material3, Brightness::Dark),
        (DesignLanguage::Cupertino, Brightness::Light),
        (DesignLanguage::Cupertino, Brightness::Dark),
        (DesignLanguage::Glyph, Brightness::Light),
        (DesignLanguage::Glyph, Brightness::Dark),
    ];

    for (design, brightness) in combos {
        let theme = match design {
            DesignLanguage::Material3 => Theme::m3_baseline(),
            DesignLanguage::Cupertino => Theme::cupertino_baseline(),
            DesignLanguage::Glyph => Theme::glyph_baseline(),
            _ => {
                // external design systems (DesignLanguage::Custom) fall back to Material chrome here
                Theme::m3_baseline()
            }
        }
        .with_brightness(brightness);
        frust::provide_context(theme);

        // A fresh mount per combo keeps the navigator's page stack from
        // compounding across routes × combos.
        let mut root: Root = RenderRoot::new();
        let mut state = HuddleApp.init();
        let mut logic = |s: &mut HuddleState| HuddleApp.build(s);

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
