//! The gallery's shell: the [`Page`] catalog, the nav rail, and the inset's
//! own top bar.
//!
//! # Why the rail is one flat list, not a grouped/labelled sidebar
//!
//! `examples/shadcn-demo`'s rail is built from `frust_shadcn`'s own
//! `sidebar_group`/`sidebar_group_label` family — a shape this crate cannot
//! reach for, since `beui-demo` depends on `frust_beui` alone (see
//! `Cargo.toml`'s header). `frust_beui::components::animated_sidebar` ports
//! upstream's **panel and menu**, not its group-header slot (see that
//! module's own doc comment on what the port covers), so there is no beUI
//! primitive for a labelled section today.
//!
//! This scaffold uses [`animated_sidebar`] as-is — a single springing list
//! carrying every gallery page — and folds each grouped page's section into
//! its own label (`"Motion \u{b7} Text"`, `"Agents \u{b7} Chat"`, ...) so the
//! four sections the card asks for (`Home`, `Motion`, `Agents`, `Blocks`,
//! `Theming`) still read in the rail without inventing a group-header widget
//! here. A real grouped rail is future work for whichever task next touches
//! `frust_beui::components::animated_sidebar` itself.
//!
//! [`Page::GpuEffects`] joins `Home` and `Theming` as a third page standing
//! outside every section — it is always in the rail, whether or not the
//! crate was built with the `gpu-effects` feature; the page itself explains
//! what is on when the feature is off.

use frust::{
    AnyView, CrossAxisAlignment, EdgeInsets, IconSource, Padding, SizedBox, View, any, column,
    icon, icons, row, scroll_view, text,
};
use frust_beui::components::animated_sidebar::{
    AnimatedSidebarView, SidebarItem, animated_sidebar, sidebar_item,
};
use frust_beui::components::button::{ButtonSize, ButtonTone, button};
use frust_beui::components::theme_toggle::theme_toggle;

use crate::AppState;

/// Every gallery page, flattened: `Home` and `Theming` stand alone, the rest
/// carry their section in [`Page::section`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page {
    Home,
    MotionText,
    MotionButtons,
    MotionControls,
    MotionSelection,
    MotionOverlays,
    MotionNavigation,
    MotionSurfaces,
    MotionData,
    MotionShader,
    AgentsPrimitives,
    AgentsPanels,
    AgentsChat,
    BlocksCommand,
    BlocksMorph,
    BlocksForms,
    BlocksShowcase,
    Theming,
    GpuEffects,
}

impl Page {
    /// Every page, in nav order — the rail's item list, and the array the
    /// rail's `on_select` index looks back into.
    pub const ALL: [Page; 19] = [
        Page::Home,
        Page::MotionText,
        Page::MotionButtons,
        Page::MotionControls,
        Page::MotionSelection,
        Page::MotionOverlays,
        Page::MotionNavigation,
        Page::MotionSurfaces,
        Page::MotionData,
        Page::MotionShader,
        Page::AgentsPrimitives,
        Page::AgentsPanels,
        Page::AgentsChat,
        Page::BlocksCommand,
        Page::BlocksMorph,
        Page::BlocksForms,
        Page::BlocksShowcase,
        Page::Theming,
        Page::GpuEffects,
    ];

    /// The section this page belongs to, or `None` for the three pages that
    /// stand outside every section.
    fn section(self) -> Option<&'static str> {
        match self {
            Page::Home | Page::Theming | Page::GpuEffects => None,
            Page::MotionText
            | Page::MotionButtons
            | Page::MotionControls
            | Page::MotionSelection
            | Page::MotionOverlays
            | Page::MotionNavigation
            | Page::MotionSurfaces
            | Page::MotionData
            | Page::MotionShader => Some("Motion"),
            Page::AgentsPrimitives | Page::AgentsPanels | Page::AgentsChat => Some("Agents"),
            Page::BlocksCommand | Page::BlocksMorph | Page::BlocksForms | Page::BlocksShowcase => {
                Some("Blocks")
            }
        }
    }

    /// The page's own name, without its section prefix.
    fn label(self) -> &'static str {
        match self {
            Page::Home => "Home",
            Page::MotionText => "Text",
            Page::MotionButtons => "Buttons",
            Page::MotionControls => "Controls",
            Page::MotionSelection => "Selection",
            Page::MotionOverlays => "Overlays",
            Page::MotionNavigation => "Navigation",
            Page::MotionSurfaces => "Surfaces",
            Page::MotionData => "Data",
            Page::MotionShader => "Shader",
            Page::AgentsPrimitives => "Primitives",
            Page::AgentsPanels => "Panels",
            Page::AgentsChat => "Chat",
            Page::BlocksCommand => "Command",
            Page::BlocksMorph => "Morph",
            Page::BlocksForms => "Forms",
            Page::BlocksShowcase => "Showcase",
            Page::Theming => "Theming",
            Page::GpuEffects => "GPU Effects",
        }
    }

    /// The rail's label and the top bar's title: `"<section> \u{b7}
    /// <label>"` for a grouped page, just `label()` for the two standalone
    /// ones.
    pub fn title(self) -> String {
        match self.section() {
            Some(section) => format!("{section} \u{b7} {}", self.label()),
            None => self.label().to_string(),
        }
    }

    /// The rail glyph for this page.
    fn icon(self) -> IconSource {
        match self {
            Page::Home => icons::HOME,
            Page::MotionText => icons::FORMAT_SIZE,
            Page::MotionButtons => icons::TAG,
            Page::MotionControls => icons::SETTINGS,
            Page::MotionSelection => icons::DONE_ALL,
            Page::MotionOverlays => icons::SCAN_MARK,
            Page::MotionNavigation => icons::ARROW_FORWARD,
            Page::MotionSurfaces => icons::PANE_MARK,
            Page::MotionData => icons::DESCRIPTION,
            Page::MotionShader => icons::IMAGE,
            Page::AgentsPrimitives => icons::STAR,
            Page::AgentsPanels => icons::GROUP,
            Page::AgentsChat => icons::FORUM,
            Page::BlocksCommand => icons::SEARCH,
            Page::BlocksMorph => icons::REFRESH,
            Page::BlocksForms => icons::EDIT,
            Page::BlocksShowcase => icons::CONTENT_COPY,
            Page::Theming => icons::LIGHT_MODE,
            Page::GpuEffects => icons::GLOBE,
        }
    }
}

/// The nav rail: [`animated_sidebar`], carrying every gallery page as one
/// springing, collapsible list. See the [module docs](self) for why it is
/// flat rather than grouped.
pub fn shell_sidebar(state: &AppState) -> AnimatedSidebarView<AppState> {
    let active = state.page;
    let items: Vec<SidebarItem<AppState>> = Page::ALL
        .iter()
        .map(|&page| {
            sidebar_item(icon(page.icon()).size(16.0), page.title()).active(page == active)
        })
        .collect();

    animated_sidebar(
        state.sidebar_open,
        items,
        |s: &mut AppState, index: usize| {
            if let Some(&page) = Page::ALL.get(index) {
                s.page = page;
            }
        },
    )
}

/// The inset's top bar: a rail-collapse trigger, the active page's title, and
/// a persistent [`theme_toggle`] — the scheme switch reachable from every
/// page, not just [`crate::pages::theming`]'s own copy.
pub fn top_bar(page: Page, open: bool) -> impl View<AppState> + use<> {
    let bar = row()
        .child(
            button(
                if open { "Collapse" } else { "Expand" },
                |s: &mut AppState| s.sidebar_open = !s.sidebar_open,
            )
            .tone(ButtonTone::Ghost)
            .size(ButtonSize::Sm),
        )
        .child(SizedBox(Some(12.0), None))
        .child(text(page.title()).size(16.0))
        .child(SizedBox(Some(12.0), None))
        .child(theme_toggle::<AppState>())
        .cross_axis(CrossAxisAlignment::Center);

    Padding(
        EdgeInsets {
            left: 16.0,
            top: 10.0,
            right: 16.0,
            bottom: 10.0,
        },
        bar,
    )
}

/// The gallery's ordinary page slot: 24px of padding inside a vertical
/// scroll view.
pub fn scroll_slot(content: AnyView<AppState>) -> AnyView<AppState> {
    any(scroll_view(Padding(EdgeInsets::all(24.0), content)))
}

/// The inset's whole content: the top bar over the swapped page content,
/// stretched to the finite height the root row hands it.
pub fn shell_inset(
    page: Page,
    open: bool,
    content: AnyView<AppState>,
) -> impl View<AppState> + use<> {
    column()
        .child(top_bar(page, open))
        .flex(1, content)
        .cross_axis(CrossAxisAlignment::Stretch)
}

/// A section heading used atop every gallery page.
pub fn heading(title: &str) -> impl View<AppState> + use<> {
    text(title.to_string()).size(24.0)
}

/// A page's small print — the caption style every gallery page explains
/// itself in.
pub fn caption(body: impl Into<String>) -> frust::TextView {
    text(body.into()).size(12.0)
}
