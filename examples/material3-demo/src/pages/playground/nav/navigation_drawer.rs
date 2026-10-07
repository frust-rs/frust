//! Navigation drawer: the reference's `NavigationDrawerPlayground`.
//!
//! # Which entry point: content, not the modal host
//!
//! `plugins/material/src/navigation_drawer.rs` exposes two: the bare,
//! non-modal [`navigation_drawer_content`] (upstream's own
//! `M3ENavigationDrawer` shape) and [`frust_material::navigation_drawer`], a
//! [`frust::NavigatorController`]-pushed modal built entirely by *this*
//! port (its own module docs: "the modal presentation ... is entirely this
//! port's own addition, not upstream's"). `navigation_drawer_playground.dart`
//! embeds `M3ENavigationDrawer` directly inside a fixed-height `SizedBox` —
//! no `Scaffold.drawer`, no show/dismiss anywhere in its 142 lines — so this
//! page mirrors that with [`navigation_drawer_content`] and needs no
//! navigator; the modal entry point goes undemoed here exactly as it is
//! upstream.

use frust::{AnyView, Component, SizedBox, Theme, View, any, component, container, icon};
use frust_material::{
    DrawerSection, MaterialDimensions, drawer_destination, drawer_section, icons,
    navigation_drawer_content,
};

use crate::AppState;
use crate::catalog::DemoEntry;
use crate::widgets::playground::{
    ambient_theme, control_panel, play_preview_card, play_snippet, play_switch, play_text_field,
    playground_body,
};

/// See the page contract in [`crate::pages::playground`].
pub fn page(_entry: DemoEntry) -> AnyView<AppState> {
    any(component(NavigationDrawerPlayground))
}

/// Fixed preview height, in logical px — the reference's own
/// `SizedBox(height: 328)`.
const PREVIEW_HEIGHT: f64 = 328.0;

/// Default headline text — the reference's own `_headline` seed.
const DEFAULT_HEADLINE: &str = "Mail";

/// This playground's own knobs — the reference's
/// `_NavigationDrawerPlaygroundState` fields.
struct DrawerState {
    selected: usize,
    headline: String,
    badges: bool,
}

struct NavigationDrawerPlayground;

impl Component for NavigationDrawerPlayground {
    type State = DrawerState;

    fn init(&self) -> DrawerState {
        DrawerState {
            selected: 0,
            headline: DEFAULT_HEADLINE.to_string(),
            badges: true,
        }
    }

    fn build(&self, state: &mut DrawerState) -> impl View<DrawerState> {
        body(state)
    }
}

/// The whole page body: [`playground_body`]'s preview/snippet/controls
/// arrangement — no overlay to mount, so (unlike its `nav` siblings with an
/// enum-menu picker) this page needs no outer [`frust::Stack`].
fn body(state: &DrawerState) -> impl View<DrawerState> {
    let theme = ambient_theme();
    playground_body(
        vec![play_preview_card(
            "Navigation drawer",
            preview(&theme, state),
        )],
        vec![play_snippet("Navigation drawer", snippet_code(state))],
        vec![content_panel(state)],
    )
}

/// Frame `child` in an outlined, rounded box — the reference's `_framed`
/// helper (duplicated per playground page upstream, so this port does the
/// same rather than sharing one across files).
fn framed(theme: &Theme, child: impl View<DrawerState>) -> impl View<DrawerState> {
    let outline = theme.scheme().outline_variant;
    container(child)
        .radius(MaterialDimensions::RADIUS_LARGE)
        .border(outline, 1.0)
}

/// The one section of four destinations — the reference's `_destinations`
/// getter, plus the optional headline (the reference's own `headline`,
/// `null` when the field is empty).
fn section(state: &DrawerState) -> DrawerSection<DrawerState> {
    let mut search = drawer_destination(icon(icons::SEARCH), "Search");
    if state.badges {
        search = search.show_badge(true);
    }
    let mut agenda = drawer_destination(icon(icons::CALENDAR_TODAY), "Agenda");
    if state.badges {
        agenda = agenda.badge_label("3");
    }
    let mut section = drawer_section(vec![
        drawer_destination(icon(icons::HOME), "Home"),
        search,
        agenda,
        drawer_destination(icon(icons::EDIT), "Drafts"),
    ]);
    if !state.headline.is_empty() {
        section = section.header(state.headline.clone());
    }
    section
}

fn preview(theme: &Theme, state: &DrawerState) -> impl View<DrawerState> {
    let content = navigation_drawer_content(
        vec![section(state)],
        state.selected,
        |s: &mut DrawerState, index| s.selected = index,
    );
    framed(
        theme,
        SizedBox::<DrawerState>(None, Some(PREVIEW_HEIGHT)).child(content),
    )
}

fn content_panel(state: &DrawerState) -> AnyView<DrawerState> {
    control_panel(
        "Content",
        vec![
            play_text_field::<DrawerState>("Headline", state.headline.clone(), |s, v| {
                s.headline = v;
            }),
            play_switch::<DrawerState>("Badges", state.badges, |s, v| s.badges = v),
        ],
    )
}

/// The FRUST-equivalent snippet for the current knob state.
fn snippet_code(state: &DrawerState) -> String {
    let headline = if state.headline.is_empty() {
        String::new()
    } else {
        format!(".header({:?})\n    ", state.headline)
    };
    let destinations = if state.badges {
        "drawer_destination(home_icon, \"Home\"),\n        drawer_destination(search_icon, \"Search\").show_badge(true),\n        drawer_destination(agenda_icon, \"Agenda\").badge_label(\"3\"),\n        drawer_destination(edit_icon, \"Drafts\"),"
    } else {
        "drawer_destination(home_icon, \"Home\"),\n        drawer_destination(search_icon, \"Search\"),\n        drawer_destination(agenda_icon, \"Agenda\"),\n        drawer_destination(edit_icon, \"Drafts\"),"
    };
    format!(
        "navigation_drawer_content(\n    vec![drawer_section(vec![\n        {destinations}\n    ]){headline}],\n    selected,\n    on_select,\n);"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_state() -> DrawerState {
        NavigationDrawerPlayground.init()
    }

    /// Every state this file's own pickers can reach still builds a page —
    /// selection, every headline value (including empty, which drops the
    /// header entirely), and the badges toggle.
    #[test]
    fn the_page_builds_across_every_reachable_control_state() {
        let mut state = base_state();
        for index in 0..4 {
            state.selected = index;
            let _view = body(&state);
        }
        for headline in ["", DEFAULT_HEADLINE, "Personal"] {
            state.headline = headline.to_string();
            let _view = body(&state);
        }
        for flag in [true, false] {
            state.badges = flag;
            let _view = body(&state);
        }
    }
}
