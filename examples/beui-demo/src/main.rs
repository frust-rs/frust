//! `beui-demo`: the beUI design-system plugin's desktop gallery — see
//! `Cargo.toml`'s header for why this rides the root workspace, and
//! `examples/shadcn-demo`'s own `main.rs` for the precedent this mirrors.
//!
//! Single binary, no `lib.rs`, desktop-only: `main` calls straight into the
//! `frust::app!`-generated `__frust_main`, exactly as `examples/shadcn-demo`
//! does.
//!
//! # Nav shell
//!
//! The rail is `frust_beui::components::animated_sidebar` — the catalog's own
//! motion-driven sidebar — carrying every gallery page in one flat,
//! springing list; see `nav.rs`'s module docs for why it is flat rather than
//! the grouped/labelled shape `examples/shadcn-demo`'s `frust_shadcn` sidebar
//! has. Page content swaps under `frust::motion::switcher`'s
//! `pattern_switcher` with the `FadeThrough` pattern — non-directional, the
//! idiomatic choice for a destination change with no spatial relationship.
//!
//! # Running it
//!
//! `cargo run -p beui-demo` for the desktop preview — see `README.md` for the
//! `frust run` device-targeting gotcha this crate's headless dev rig hits.

mod nav;
mod pages;

use frust::motion::patterns::FadeThrough;
use frust::motion::switcher::pattern_switcher;
use frust::{
    AnyView, Axis, Component, CrossAxisAlignment, DesktopConfig, FlexView, MenuItemSpec, MenuRole,
    MenuSpec, any, flexible, inflexible,
};

use nav::Page;

/// The app's whole retained state: which gallery page is showing, whether the
/// nav rail is expanded, and the state of the two pages that keep theirs here
/// (Home and Theming) — every section page hosts its own in a page-local
/// component (see `pages/mod.rs`).
pub struct AppState {
    pub page: Page,
    pub sidebar_open: bool,
    pub home: pages::home::State,
    pub theming: pages::theming::State,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            page: Page::Home,
            sidebar_open: true,
            home: Default::default(),
            theming: Default::default(),
        }
    }
}

/// Resolve the active page's content view.
fn page_body(page: Page, state: &mut AppState) -> AnyView<AppState> {
    match page {
        Page::Home => any(pages::home::page(&mut state.home)),
        Page::Theming => any(pages::theming::page(&mut state.theming)),
        Page::MotionText => any(pages::motion::text::page()),
        Page::MotionButtons => any(pages::motion::buttons::page()),
        Page::MotionControls => any(pages::motion::controls::page()),
        Page::MotionSelection => any(pages::motion::selection::page()),
        Page::MotionOverlays => any(pages::motion::overlays::page()),
        Page::MotionNavigation => any(pages::motion::navigation::page()),
        Page::MotionSurfaces => any(pages::motion::surfaces::page()),
        Page::MotionData => any(pages::motion::data::page()),
        Page::MotionShader => any(pages::motion::shader::page()),
        Page::AgentsPrimitives => any(pages::agents::primitives::page()),
        Page::AgentsPanels => any(pages::agents::panels::page()),
        Page::AgentsChat => any(pages::agents::chat::page()),
        Page::BlocksCommand => any(pages::blocks::command::page()),
        Page::BlocksMorph => any(pages::blocks::morph::page()),
        Page::BlocksForms => any(pages::blocks::forms::page()),
        Page::BlocksShowcase => any(pages::blocks::showcase::page()),
        Page::GpuEffects => any(pages::gpu_effects::page()),
    }
}

#[derive(Default)]
struct BeuiDemoApp;

impl Component for BeuiDemoApp {
    type State = AppState;

    fn init(&self) -> AppState {
        AppState::default()
    }

    fn build(&self, state: &mut AppState) -> AnyView<AppState> {
        let page = state.page;
        let open = state.sidebar_open;

        let body = page_body(page, state);
        let content = any(pattern_switcher::<AppState, FadeThrough, _>(
            page as usize,
            FadeThrough,
            nav::scroll_slot(body),
        ));

        let sidebar = any(nav::shell_sidebar(state));

        any(FlexView::new(
            Axis::Horizontal,
            vec![
                inflexible(sidebar),
                flexible(1, any(nav::shell_inset(page, open, content))),
            ],
        )
        .cross_axis(CrossAxisAlignment::Stretch))
    }
}

frust::app!(
    BeuiDemoApp,
    setup = {
        frust_beui::install();
    },
    desktop = {
        let quit = MenuSpec::new().with_item(MenuItemSpec::role(MenuRole::Quit));
        DesktopConfig::new()
            .with_app_name("beUI Gallery")
            .with_app_id("dev.frust.beuidemo")
            .with_menu_spec(MenuSpec::new().with_item(MenuItemSpec::submenu("File", quit)))
    }
);

fn main() {
    __frust_main();
}
