//! `material3demo`: a gallery over the `frust-material` catalog — the frust
//! port of the `material_3_expressive` package's own example app, and the
//! vehicle for that catalog's runtime gate on desktop, Android, and iOS.
//!
//! This module owns the whole app; `src/main.rs` is only the desktop preview
//! entry point calling the `__frust_main` the [`frust::app!`] at the bottom of
//! this file generates. That macro also emits the Android JNI exports and the
//! iOS C-ABI exports, so the same tree drives all three shells — run it with
//! `cargo run` (desktop) or `frust run -d <device>` (Android/iOS).
//!
//! # The shell, and where each screen lives
//!
//! The reference's shell is a `Scaffold` holding a top app bar, one of five
//! section pages, and a bottom navigation bar, with playgrounds and the theme
//! screen pushed over all of it. This port maps that onto the nav stack:
//!
//! * the **outer** navigator holds the gallery shell plus anything pushed over
//!   it (`/playground/:id`, `/theme`);
//! * a [`shell_route`] binds the five section routes (`/do`, `/pick`, ...) to
//!   an **inner** navigator mounted inside the shell page, so switching
//!   sections never rebuilds the chrome around them;
//! * a [`RouteObserver`] over that inner navigator is what the navigation bar
//!   reads its selected index from — the published route stack, not a second
//!   copy of the selection.
//!
//! `/theme` and `/playground/:id` are declared **before** the shell route: a
//! shell route is pathless, so it also matches the empty segment list, and
//! resolution takes the first match.
//!
//! # Transitions
//!
//! Frust has no `MaterialApp`-style auto-wiring — a navigator animates only
//! once an app opts it in via [`frust::NavigatorView::transition`], per
//! navigator. The outer navigator (parent→child pushes onto the gallery:
//! playgrounds, `/theme`) uses M3's shared-axis-X pattern; the inner one
//! (top-level section switches behind the nav bar) uses M3's fade-through
//! pattern — see [`gallery_shell`] and [`Material3Demo::build`].
//!
//! # State
//!
//! Everything a callback needs hangs off [`AppState`], which every view
//! callback receives — that is how a list row reaches the router. A page
//! *builder*, by contrast, gets no state at all (the navigator hands it only
//! its route params), so anything a builder reads is either captured (the
//! selection signals, the route observer) or read from context ([`Theme`],
//! `WindowMetrics`) at build time.

mod catalog;
mod pages;
mod theme;
mod widgets;

// `DesktopConfig` and the menu vocabulary are deliberately absent from this
// list: `frust` gates them off for Android and iOS, and this crate now builds
// for both. They are named fully-qualified inside `frust::app!`'s `desktop =
// { .. }` block instead, which the macro emits only into the desktop entry
// point — the idiom `app!`'s own docs prescribe for desktop-only types.
use frust::{
    AnyView, Brightness, Component, NavigatorController, PageTransition, Route, RouteObserver,
    RouteParams, Router, SizedBox, Theme, TransitionSpec, any, icon, navigator, provide_context,
    safe_area, scaffold, shell_route, use_context,
};
use frust_material::{app_bar, icon_button, icons, nav_item, navigation_bar};

use catalog::DemoSection;
use pages::{
    SectionSelection, THEME_ROUTE, brightness_action, playground_route, section_host,
    theme_config_page,
};
use theme::ThemeSettings;

/// The window and app bar title.
const APP_TITLE: &str = "Material 3 Expressive";

/// The gallery's whole retained state: the theme settings every screen edits,
/// the router and the two controllers it drives, and the section selections
/// the split layout reads back.
pub struct AppState {
    /// Seed/brightness/font/type-style, applied to the running shell.
    pub settings: ThemeSettings,
    /// Resolves every location in this app; callbacks navigate through it.
    pub router: Router<AppState>,
    /// The gallery shell and anything pushed over it.
    pub outer: NavigatorController<AppState>,
    /// The five section pages, inside the shell.
    pub inner: NavigatorController<AppState>,
    /// Each section's split-layout selection.
    pub selection: SectionSelection,
    /// The inner navigator's published route stack — the navigation bar's
    /// source of truth for which destination is lit.
    pub section_routes: RouteObserver,
}

/// The root page both navigators are constructed with.
///
/// Never seen: the first navigation replaces it before the first frame (the
/// pre-first-frame op drain), and it exists only because a navigator needs a
/// root page to be constructed at all.
fn blank_page() -> AnyView<AppState> {
    any(SizedBox::<AppState>(None, None))
}

/// The app's route table. See the module docs for why order matters here.
fn routes(
    inner: &NavigatorController<AppState>,
    selection: SectionSelection,
    section_routes: RouteObserver,
) -> Vec<Route<AppState>> {
    let sections: Vec<Route<AppState>> = DemoSection::ALL
        .into_iter()
        .map(|section| {
            Route::new(section.path(), move |_: &RouteParams| {
                section_host(section, selection)
            })
        })
        .collect();

    let shell_inner = inner.clone();
    vec![
        Route::new(THEME_ROUTE, |_: &RouteParams| theme_config_page()),
        Route::new("/playground/:id", playground_route),
        shell_route(
            inner,
            move |_: &RouteParams| gallery_shell(&shell_inner, section_routes),
            sections,
        ),
    ]
}

/// The gallery shell: the app bar, the section currently routed onto the
/// inner navigator, and the navigation bar switching between them.
fn gallery_shell(
    inner: &NavigatorController<AppState>,
    section_routes: RouteObserver,
) -> AnyView<AppState> {
    let theme = use_context::<Theme>().unwrap_or_else(frust_material::baseline);
    let selected = DemoSection::from_path(&section_routes.path())
        .map(DemoSection::index)
        .unwrap_or(0);

    let bar = app_bar::<AppState>(APP_TITLE).actions(vec![
        any(
            icon_button(any(icon(icons::PALETTE)), |state: &mut AppState| {
                state.router.push(THEME_ROUTE)
            })
            .semantic_label("Theme settings"),
        ),
        brightness_action(theme.brightness),
    ]);

    let destinations = DemoSection::ALL
        .into_iter()
        .map(|section| {
            nav_item::<AppState>(section.nav_label()).icon(any(icon(section.nav_icon())))
        })
        .collect();
    let nav_bar = navigation_bar(
        destinations,
        selected,
        |state: &mut AppState, index: usize| {
            if let Some(section) = DemoSection::ALL.get(index) {
                state.router.go(section.path());
            }
        },
    );

    // Section switches are top-level-destination changes (M3's fade-through
    // pattern), not parent→child navigation, so the inner navigator gets its
    // own preset rather than inheriting the outer one's shared-axis-X. `go`
    // reaches this navigator as a `replace` carrying no per-op override
    // (`shell_route`'s keep-rule table), so it honors this default.
    let inner_view = navigator(inner, blank_page)
        .transition(TransitionSpec::duration(PageTransition::M3FadeThrough));

    // The bars self-size but consume no window inset of their own (the
    // catalog leaves that to the composer), so each is wrapped for the edge it
    // sits on.
    any(scaffold(any(section_routes.observe(inner_view)))
        .app_bar(any(safe_area(bar).bottom(false)))
        .bottom_bar(any(safe_area(nav_bar).top(false)))
        .background(theme.scheme().surface))
}

#[derive(Default)]
struct Material3Demo;

impl Component for Material3Demo {
    type State = AppState;

    fn init(&self) -> AppState {
        let settings = ThemeSettings::new();
        // The reference's `ExampleThemeScope`: every route can read the live
        // settings without them being threaded through its builder.
        provide_context(settings);
        // `init` runs before the shell is constructed, which is the one moment
        // it reads the default-theme slot — so the seeded (not baked) base is
        // in place for the first frame.
        settings.apply(Brightness::Light);

        let outer = NavigatorController::new();
        let inner = NavigatorController::new();
        let selection = SectionSelection::new();
        let section_routes = RouteObserver::new();
        let router = Router::with_controller(&outer, routes(&inner, selection, section_routes));
        // Places the shell and its first section; both controllers drain these
        // ops on their first build, so the first painted frame is already the
        // Do section rather than the blank root page.
        router.go(DemoSection::ALL[0].path());

        AppState {
            settings,
            router,
            outer,
            inner,
            selection,
            section_routes,
        }
    }

    fn build(&self, state: &mut AppState) -> AnyView<AppState> {
        // Parent→child navigation (a playground or `/theme` pushed over the
        // gallery shell): M3's shared-axis-X pattern. Pop reversal, including
        // the edge-swipe gesture, comes free — the navigator reverses the
        // stored spec on pop.
        any(navigator(&state.outer, blank_page)
            .transition(TransitionSpec::duration(PageTransition::M3SharedAxisX)))
    }
}

frust::app!(
    Material3Demo,
    setup = {
        frust_material::install();
    },
    desktop = {
        let quit =
            frust::MenuSpec::new().with_item(frust::MenuItemSpec::role(frust::MenuRole::Quit));
        frust::DesktopConfig::new()
            .with_app_name(APP_TITLE)
            .with_app_id("dev.frust.material3demo")
            .with_menu_spec(
                frust::MenuSpec::new().with_item(frust::MenuItemSpec::submenu("File", quit)),
            )
    }
);

/// Route-table coverage. The shell *view* is deliberately not built here:
/// [`navigator`] auto-wires back handling, which reads a reactive source the
/// facade's own entry point installs — so anything mounting a navigator only
/// builds under a running app. Everything below the navigator (the section
/// hosts, the playgrounds) is covered by its own module's tests.
#[cfg(test)]
mod tests {
    use frust::{NavigatorController, Resolution, RouteObserver, Router};

    use super::{AppState, DemoSection, SectionSelection, THEME_ROUTE, routes};
    use crate::catalog;

    /// The app's real route table, over throwaway controllers.
    fn router() -> Router<AppState> {
        let outer = NavigatorController::new();
        let inner = NavigatorController::new();
        Router::with_controller(
            &outer,
            routes(&inner, SectionSelection::new(), RouteObserver::new()),
        )
    }

    #[test]
    fn every_section_resolves_to_the_shell_page_plus_its_own() {
        let router = router();
        for section in DemoSection::ALL {
            match router.resolve(section.path()) {
                Resolution::Matched { pages, .. } => assert_eq!(
                    pages.len(),
                    2,
                    "{} should resolve through the shell route",
                    section.path()
                ),
                Resolution::Error { location } => {
                    panic!("{} did not match: {location:?}", section.path())
                }
            }
        }
    }

    /// The ordering rule the route table depends on: a pathless shell route
    /// also matches the empty segment list, so these two must not be shadowed
    /// by it.
    #[test]
    fn the_routes_pushed_over_the_shell_resolve_outside_it() {
        let router = router();
        for location in [THEME_ROUTE, "/playground/buttons"] {
            match router.resolve(location) {
                Resolution::Matched { pages, .. } => {
                    assert_eq!(
                        pages.len(),
                        1,
                        "{location} covers the shell, not nests in it"
                    )
                }
                Resolution::Error { location } => panic!("{location:?} did not match"),
            }
        }
    }

    #[test]
    fn every_catalog_entry_has_a_resolvable_playground_route() {
        let router = router();
        for entry in catalog::for_section(DemoSection::Do)
            .iter()
            .chain(catalog::for_section(DemoSection::Find))
        {
            let resolution = router.resolve(&entry.route());
            assert!(resolution.is_matched(), "{} has no route", entry.id);
            assert_eq!(
                resolution.params().get("id").map(String::as_str),
                Some(entry.id)
            );
        }
    }

    #[test]
    fn an_unknown_location_falls_through_to_the_error_page() {
        assert!(!router().resolve("/no-such-route").is_matched());
    }
}
