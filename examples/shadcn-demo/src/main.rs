//! `shadcn-demo`: a desktop-first gallery exercising every shipped
//! `frust-shadcn` component — the vehicle for the design system's desktop
//! runtime gate (hover, cursor, overlays, and both bundled fonts all get
//! their first live-window verification here).
//!
//! Single binary, no `lib.rs`: `frust::app!` self-gates per platform, but
//! this crate is deliberately desktop-only (see `Cargo.toml`'s header), so
//! `main` calls straight into the generated `__frust_main` the way
//! `examples/design-system-sample/sample-app` does.

mod nav;
mod pages;

use frust::{
    AnyView, Component, CrossAxisAlignment, DesktopConfig, EdgeInsets, FlexView, MenuItemSpec,
    MenuRole, MenuSpec, Padding, any, scroll_view,
};

use nav::{Page, sidebar};

/// The app's whole retained state: which gallery page is showing, plus the
/// per-page interactive state each page owns.
pub struct AppState {
    pub page: Page,
    pub primitives: pages::primitives::State,
    pub controls: pages::controls::State,
    pub inputs_table: pages::inputs_table::State,
    pub overlays: pages::overlays::State,
    pub anchored: pages::anchored::State,
    pub theming: pages::theming::State,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            page: Page::Primitives,
            primitives: Default::default(),
            controls: Default::default(),
            inputs_table: Default::default(),
            overlays: Default::default(),
            anchored: Default::default(),
            theming: Default::default(),
        }
    }
}

#[derive(Default)]
struct ShadcnDemoApp;

impl Component for ShadcnDemoApp {
    type State = AppState;

    fn init(&self) -> AppState {
        AppState::default()
    }

    fn build(&self, state: &mut AppState) -> AnyView<AppState> {
        let page = state.page;
        let content = match page {
            Page::Primitives => any(pages::primitives::page(&mut state.primitives)),
            Page::Controls => any(pages::controls::page(&mut state.controls)),
            Page::InputsTable => any(pages::inputs_table::page(&mut state.inputs_table)),
            Page::Overlays => any(pages::overlays::page(&mut state.overlays)),
            Page::Anchored => any(pages::anchored::page(&mut state.anchored)),
            Page::Theming => any(pages::theming::page(&mut state.theming)),
        };
        let scrollable_content = scroll_view(Padding(EdgeInsets::all(24.0), content));
        any(FlexView::new(
            frust::Axis::Horizontal,
            vec![
                frust::inflexible(any(sidebar(page))),
                frust::flexible(1, any(scrollable_content)),
            ],
        )
        .cross_axis(CrossAxisAlignment::Stretch))
    }
}

frust::app!(
    ShadcnDemoApp,
    setup = {
        frust_shadcn::install();
    },
    desktop = {
        let quit = MenuSpec::new().with_item(MenuItemSpec::role(MenuRole::Quit));
        DesktopConfig::new()
            .with_app_name("Shadcn Demo")
            .with_app_id("dev.frust.shadcndemo")
            .with_menu_spec(MenuSpec::new().with_item(MenuItemSpec::submenu("File", quit)))
    }
);

fn main() {
    __frust_main();
}
