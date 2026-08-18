//! `shadcn-demo`: a desktop-first gallery exercising every shipped
//! `frust-shadcn` component — the vehicle for the design system's desktop
//! runtime gate (hover, cursor, overlays, motion, and both bundled fonts all
//! get their live-window verification here).
//!
//! Single binary, no `lib.rs`: `frust::app!` self-gates per platform, but
//! this crate is deliberately desktop-only (see `Cargo.toml`'s header), so
//! `main` calls straight into the generated `__frust_main` the way
//! `examples/design-system-sample/sample-app` does.
//!
//! # The constraint chain, end to end
//!
//! `frust_shadcn::overlay`'s mounting contract makes **bounded constraints**
//! part of hosting an overlay: a navigator or a full-area `Stack` fills the
//! area it is handed, and a `scroll_view` hands its child an infinite max
//! height, which those hosts coerce to zero. The app shell therefore has to
//! deliver a finite height all the way down to a page:
//!
//! 1. the root gets the window's own tight constraints;
//! 2. `sidebar_provider` splits that row and lays the inset out tight
//!    (`rest × height`);
//! 3. `sidebar_inset` hands its child `width` tight and `0..=height`;
//! 4. **but** the inset wraps its children in a plain `Column`, whose
//!    inflexible children are laid out under an *unbounded* main axis — so the
//!    finite height stops there;
//! 5. [`shell_body`] re-tightens it: one `SizedBox` at the window's own logical
//!    height (read from [`frust::WindowMetrics`]), holding a vertical `FlexView`
//!    whose flexible page slot therefore gets a tight, finite height;
//! 6. a page then either scrolls inside that slot ([`scroll_slot`]) or hosts
//!    overlays against it — never both in that order.
//!
//! Step 5 is this app's workaround for step 4, not a framework requirement.

mod nav;
mod pages;

use frust::{
    AnyView, Axis, Component, CrossAxisAlignment, DesktopConfig, EdgeInsets, FlexView,
    MenuItemSpec, MenuRole, MenuSpec, Padding, SizedBox, View, WindowMetrics, any, flexible,
    inflexible, scroll_view, use_context,
};
use frust_shadcn::{sidebar_inset, sidebar_provider};

use nav::Page;

/// The app's whole retained state: which gallery page is showing, whether the
/// nav panel is expanded, plus the per-page interactive state each page owns.
pub struct AppState {
    pub page: Page,
    pub sidebar_open: bool,
    pub primitives: pages::primitives::State,
    pub controls: pages::controls::State,
    pub inputs_table: pages::inputs_table::State,
    pub layout: pages::layout::State,
    pub overlays: pages::overlays::State,
    pub anchored: pages::anchored::State,
    pub chat: pages::chat::State,
    pub questionnaire: pages::questionnaire::State,
    pub data_table: pages::data_table::State,
    pub theming: pages::theming::State,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            page: Page::Primitives,
            sidebar_open: true,
            primitives: Default::default(),
            controls: Default::default(),
            inputs_table: Default::default(),
            layout: Default::default(),
            overlays: Default::default(),
            anchored: Default::default(),
            chat: Default::default(),
            questionnaire: Default::default(),
            data_table: Default::default(),
            theming: Default::default(),
        }
    }
}

/// The height the shell falls back to before the first `WindowMetrics` publish
/// (and on any host that publishes none) — the desktop shell's own default
/// window height.
const FALLBACK_SHELL_HEIGHT: f64 = 720.0;

/// The gallery's ordinary page slot: 24px of padding inside a vertical
/// scroll view.
///
/// Pages that host an overlay, or that own a scroll surface of their own, take
/// the page slot's bounded constraints directly instead (see this module's
/// constraint chain).
pub fn scroll_slot(content: AnyView<AppState>) -> AnyView<AppState> {
    any(scroll_view(Padding(EdgeInsets::all(24.0), content)))
}

/// The inset's content: the top bar over a page slot with a **finite** height.
///
/// See the module docs for why the height is restated here rather than
/// inherited — `sidebar_inset`'s own `Column` wrapper lays an inflexible child
/// out under an unbounded main axis, so a page hosted through it would see an
/// infinite height and every scroll surface and overlay host below it would
/// collapse.
fn shell_body(
    page: Page,
    open: bool,
    height: f64,
    content: AnyView<AppState>,
) -> impl View<AppState> + use<> {
    SizedBox::<AppState>(None, Some(height)).child(
        FlexView::new(
            Axis::Vertical,
            vec![
                inflexible(any(nav::top_bar(page, open))),
                flexible(1, content),
            ],
        )
        .cross_axis(CrossAxisAlignment::Stretch),
    )
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
        let open = state.sidebar_open;
        let height =
            use_context::<WindowMetrics>().map_or(FALLBACK_SHELL_HEIGHT, |m| m.size.height);

        // Four pages own their own bounded-constraint layout: the two overlay
        // hosts (a navigator, a full-area `Stack`), the data table (a `Stack`
        // carrying its two menus), and the chat page (whose message scroller is
        // a scroll surface that needs a finite viewport). Every other page is
        // wrapped in the shared scroll slot here.
        let content = match page {
            Page::Primitives => scroll_slot(any(pages::primitives::page(&mut state.primitives))),
            Page::Controls => scroll_slot(any(pages::controls::page(&mut state.controls))),
            Page::InputsTable => {
                scroll_slot(any(pages::inputs_table::page(&mut state.inputs_table)))
            }
            Page::Layout => scroll_slot(any(pages::layout::page(&mut state.layout))),
            Page::Overlays => any(pages::overlays::page(&mut state.overlays)),
            Page::Anchored => any(pages::anchored::page(&mut state.anchored)),
            Page::Chat => any(pages::chat::page(&mut state.chat)),
            Page::Questionnaire => {
                scroll_slot(any(pages::questionnaire::page(&mut state.questionnaire)))
            }
            Page::DataTable => any(pages::data_table::page(&mut state.data_table)),
            Page::Theming => scroll_slot(any(pages::theming::page(&mut state.theming))),
        };

        any(sidebar_provider(
            nav::shell_sidebar(state),
            sidebar_inset(vec![any(shell_body(page, open, height, content))]),
            open,
            |s: &mut AppState, next: bool| {
                s.sidebar_open = next;
            },
        ))
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
