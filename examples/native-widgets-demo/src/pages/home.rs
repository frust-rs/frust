//! The Home tab's root page, after the GitHub app's home: a "My Work" card of
//! icon-tile rows that each push a [`detail`](super::detail) page, a
//! "Favorites" card whose one control is a real native button, and a
//! "Shortcuts" note. The body is frust, the bars are the shell's native
//! chrome, and the button that stays put is native.

use frust::authoring::text::FontWeight;
use frust::{
    AnyView, Axis, Color, EdgeInsets, FlexChild, FlexView, GestureDetector, IconSource, Padding,
    Set, Theme, View, container, divider, flexible, icon, icons, inflexible, text,
};
use frust_native_widgets::native_button;

use super::common::{S, caption, chip, gap, gap_h, page_column, readout, row, tab_body, theme};
use super::detail;
use crate::{DetailRoute, Tab, push_toast};

/// The repository every pushed page is subtitled with.
const REPOSITORY: &str = "frust-rs/frust";

/// The most characters a pushed page's bar title keeps before its ellipsis.
const TITLE_CHARS: usize = 32;

/// Side length of a row's coloured icon tile.
const TILE: f64 = 40.0;

/// Width and height of the native "Add Favorites" button.
const FAVORITES_BUTTON: (f64, f64) = (180.0, 44.0);

/// One "My Work" row: its label, icon, the theme colour that fills its tile,
/// and the colour of the icon on it.
struct WorkRow {
    label: &'static str,
    icon: IconSource,
    tile: fn(&Theme) -> Color,
    glyph: fn(&Theme) -> Color,
}

/// The "My Work" rows, in the GitHub app's order.
const WORK_ROWS: [WorkRow; 7] = [
    WorkRow {
        label: "Issues",
        icon: icons::DESCRIPTION,
        tile: |t| t.scheme().primary,
        glyph: |t| t.scheme().surface,
    },
    WorkRow {
        label: "Pull Requests",
        icon: icons::ARROW_FORWARD,
        tile: |t| t.scheme().tertiary,
        glyph: |t| t.scheme().surface,
    },
    WorkRow {
        label: "Discussions",
        icon: icons::FORUM,
        tile: |t| t.scheme().secondary,
        glyph: |t| t.scheme().surface,
    },
    WorkRow {
        label: "Projects",
        icon: icons::PUSH_PIN,
        tile: |t| t.scheme().error,
        glyph: |t| t.scheme().surface,
    },
    WorkRow {
        label: "Top Repositories",
        icon: icons::TAG,
        tile: |t| t.scheme().tertiary_container,
        glyph: |t| t.scheme().on_tertiary_container,
    },
    WorkRow {
        label: "Organizations",
        icon: icons::GROUP,
        tile: |t| t.scheme().on_tertiary_container,
        glyph: |t| t.scheme().surface,
    },
    WorkRow {
        label: "Starred",
        icon: icons::STAR,
        tile: |t| t.scheme().on_secondary_container,
        glyph: |t| t.scheme().surface,
    },
];

/// See the page-fn contract in [`crate::pages`].
pub fn page(state: &S) -> AnyView<S> {
    let sections = vec![
        section_title("My Work", Some("...")),
        gap(8.0),
        inflexible(card(work_rows(state))),
        gap(20.0),
        section_title("Favorites", None),
        gap(8.0),
        inflexible(card(favorites_body(state))),
        gap(20.0),
        section_title("Shortcuts", None),
        gap(8.0),
        inflexible(caption(
            "Shortcuts you add show up here for one-tap access.",
        )),
    ];
    tab_body(state, Tab::Home, vec![inflexible(page_column(sections))])
}

/// A section heading, with an optional trailing chip (the "..." menu).
fn section_title(title: &str, trailing: Option<&str>) -> FlexChild<S> {
    let mut children = vec![flexible(
        1,
        text(title)
            .size(20.0)
            .weight(FontWeight::BOLD)
            .color(theme().scheme().on_surface),
    )];
    if let Some(trailing) = trailing {
        children.push(inflexible(chip(trailing, |_: &mut S| {})));
    }
    inflexible(row(children))
}

/// A rounded surface card around `children`.
fn card(children: Vec<FlexChild<S>>) -> impl View<S> {
    container(Padding(
        EdgeInsets::all(12.0),
        FlexView::new(Axis::Vertical, children),
    ))
    .fill(theme().scheme().surface_container)
    .radius(12.0)
}

/// The "My Work" rows, a divider between each pair.
fn work_rows(state: &S) -> Vec<FlexChild<S>> {
    let mut children = Vec::new();
    for (index, work) in WORK_ROWS.iter().enumerate() {
        if index > 0 {
            children.push(gap(8.0));
            children.push(inflexible(divider(theme().scheme().outline_variant)));
            children.push(gap(8.0));
        }
        children.push(inflexible(work_row(state, work)));
    }
    children
}

/// One tappable row: coloured icon tile, label, trailing chevron. Tapping
/// pushes the detail page onto the Home navigator.
fn work_row(state: &S, work: &WorkRow) -> impl View<S> {
    let theme = theme();
    let tile = container(icon(work.icon).size(22.0).color((work.glyph)(&theme)))
        .fill((work.tile)(&theme))
        .radius(10.0)
        .size_centered(TILE, TILE);
    let label = text(work.label).size(16.0).color(theme.scheme().on_surface);
    let chevron = icon(icons::CHEVRON_RIGHT)
        .size(22.0)
        .color(theme.scheme().on_surface_variant);
    let title = detail_title(work.label);
    let handles = state.clone();
    GestureDetector(row(vec![
        inflexible(tile),
        gap_h(12.0),
        flexible(1, label),
        inflexible(chevron),
    ]))
    .on_tap(move |state: &mut S| open_detail(state, &handles, &title))
}

/// The bar title for a pushed row: its label over the sample subject,
/// truncated with an ellipsis like the GitHub app does.
fn detail_title(label: &str) -> String {
    let full = format!("{label}: damage-aware present pipeline");
    if full.chars().count() <= TITLE_CHARS {
        return full;
    }
    let kept: String = full.chars().take(TITLE_CHARS - 1).collect();
    format!("{}\u{2026}", kept.trim_end())
}

/// Push a detail page titled `title` onto the Home navigator. Runs in a tap
/// handler, never in `build`. The page opens with its composer closed.
fn open_detail(state: &mut S, handles: &S, title: &str) {
    state.composer_open.set(false);
    let route = DetailRoute::new(title, REPOSITORY);
    let (handles, shown) = (handles.clone(), route.clone());
    route.push(state.nav(Tab::Home), move || detail::page(&handles, &shown));
}

/// The Favorites card: two lines of explanation and the one native control in
/// this body.
fn favorites_body(state: &S) -> Vec<FlexChild<S>> {
    let toasts = state.toasts;
    vec![
        inflexible(readout(
            "Favorites give you quick access to your most used items.",
        )),
        gap(4.0),
        inflexible(caption(
            "Add repositories, issues or pull requests to see them here.",
        )),
        gap(12.0),
        inflexible(
            native_button("Add Favorites")
                .content_description("Add a favorite")
                .size(FAVORITES_BUTTON.0, FAVORITES_BUTTON.1)
                .on_press(move || push_toast(toasts, "Favorites added".to_string())),
        ),
    ]
}
