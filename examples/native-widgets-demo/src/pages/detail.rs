//! A pushed detail page, shaped like a GitHub pull request: a markdown-ish
//! body (headings, bullets, a three-column table, inline code chips) long
//! enough to scroll under both bars, and, while the toolbar's Comment is on,
//! a composer pinned under the scroller. Its bar title and subtitle travel as
//! its [`DetailRoute`]; the page draws the body only, on the shell's detail
//! scroll controller, and never sets chrome.
//!
//! The composer is the LAST child of a column whose first child is the flexed
//! scroller, so the scaffold's keyboard avoidance lifts it onto the keyboard
//! (no manual padding here). frust's text input has no autofocus, so the
//! composer opens unfocused and a tap focuses it.

use frust::authoring::text::FontWeight;
use frust::{
    AnyView, Axis, EdgeInsets, FlexChild, FlexView, Get, GetUntracked, Padding, Set, View, any,
    container, divider, flexible, inflexible, safe_area, scroll_view, text, text_input,
};

use super::common::{S, caption, chip, gap, gap_h, local_sig, page_column, readout, row, theme};
use crate::{DetailRoute, push_toast};

local_sig!(composer_text, String, String::new());

/// The table: a header row, then these `(name, status, owner)` rows.
const TABLE_ROWS: [(&str, &str, &str); 4] = [
    ("Present", "Merged", "engine"),
    ("Damage", "In review", "render"),
    ("Scroll", "Open", "core"),
    ("Chrome", "Draft", "shells"),
];

/// See the page-fn contract in [`crate::pages`]; `route` is what this page
/// was pushed with.
pub fn page(state: &S, route: &DetailRoute) -> AnyView<S> {
    let scroller =
        scroll_view(safe_area(page_column(body(route)))).controller(state.detail_scroll.clone());
    let mut children = vec![flexible(1, scroller)];
    if state.composer_open.get() {
        children.push(inflexible(composer(state)));
    }
    any(FlexView::new(Axis::Vertical, children))
}

/// The pull request's body, top to bottom.
fn body(route: &DetailRoute) -> Vec<FlexChild<S>> {
    vec![
        inflexible(
            text(route.title.clone())
                .size(22.0)
                .weight(FontWeight::BOLD)
                .color(theme().scheme().on_surface),
        ),
        gap(4.0),
        inflexible(caption(format!(
            "{} \u{b7} opened by frust-bot",
            route.subtitle
        ))),
        gap(16.0),
        heading("Summary"),
        paragraph(
            "Presenting a frame no longer repaints the whole window. Each frame now reports the rectangles that changed, and the surface presents only those.",
        ),
        gap(8.0),
        inflexible(row(vec![
            inflexible(readout("Set")),
            gap_h(6.0),
            inflexible(code("present_with_damage")),
            gap_h(6.0),
            inflexible(readout("to opt in.")),
        ])),
        gap(16.0),
        heading("Changes"),
        bullet("Track dirty rectangles per strip during paint."),
        bullet("Merge overlapping rectangles before the present call."),
        bullet("Fall back to a full present when the platform refuses damage."),
        bullet("Keep the first frame after a resize a full present."),
        gap(16.0),
        heading("Status by area"),
        inflexible(table()),
        gap(16.0),
        heading("Testing"),
        paragraph(
            "Headless paint tests cover the merge and the fallback. The device gate compares power draw while scrolling a long list on both phones.",
        ),
        gap(8.0),
        inflexible(row(vec![
            inflexible(readout("Run")),
            gap_h(6.0),
            inflexible(code("cargo test -p frust-engine")),
        ])),
        gap(16.0),
        heading("Notes"),
        bullet("The merge keeps at most eight rectangles per frame."),
        bullet("A rectangle covering most of the window collapses to a full present."),
        bullet("Debug builds log every fallback with its reason."),
        gap(16.0),
        paragraph(
            "Reviewers: please try the branch on a phone before approving, since desktop compositors hide most of the difference.",
        ),
        gap(48.0),
    ]
}

/// A section heading.
fn heading(title: &str) -> FlexChild<S> {
    inflexible(
        text(title)
            .size(18.0)
            .weight(FontWeight::BOLD)
            .color(theme().scheme().on_surface),
    )
}

/// A body paragraph followed by a little air.
fn paragraph(copy: &str) -> FlexChild<S> {
    inflexible(Padding(EdgeInsets::symmetric(0.0, 3.0), readout(copy)))
}

/// A bullet line: a dot, then the text filling the rest of the row.
fn bullet(copy: &str) -> FlexChild<S> {
    inflexible(Padding(
        EdgeInsets::symmetric(0.0, 3.0),
        row(vec![
            inflexible(readout("\u{2022}")),
            gap_h(8.0),
            flexible(1, readout(copy)),
        ]),
    ))
}

/// An inline code chip.
fn code(snippet: &str) -> impl View<S> {
    let theme = theme();
    let scheme = theme.scheme();
    container(Padding(
        EdgeInsets::symmetric(6.0, 2.0),
        text(snippet).size(12.0).color(scheme.on_surface),
    ))
    .fill(scheme.surface_container_high)
    .radius(4.0)
}

/// The three-column table: a header row and [`TABLE_ROWS`], dividers between.
fn table() -> impl View<S> {
    let mut children = vec![inflexible(table_row("Area", "Status", "Owner", true))];
    for (name, status, owner) in TABLE_ROWS {
        children.push(inflexible(divider(theme().scheme().outline_variant)));
        children.push(inflexible(table_row(name, status, owner, false)));
    }
    container(FlexView::new(Axis::Vertical, children))
        .border(theme().scheme().outline_variant, 1.0)
        .radius(8.0)
}

/// One table row of three equal columns.
fn table_row(a: &str, b: &str, c: &str, header: bool) -> impl View<S> {
    let cell = |copy: &str| {
        let ink = theme().scheme().on_surface;
        let weight = if header {
            FontWeight::BOLD
        } else {
            FontWeight::REGULAR
        };
        Padding(
            EdgeInsets::symmetric(10.0, 8.0),
            text(copy).size(12.0).weight(weight).color(ink),
        )
    };
    row(vec![
        flexible(1, cell(a)),
        flexible(1, cell(b)),
        flexible(1, cell(c)),
    ])
}

/// The composer row: a text field and a Send chip that toasts the text,
/// clears it and closes the composer.
fn composer(state: &S) -> impl View<S> {
    let theme = theme();
    let scheme = theme.scheme();
    let (toasts, open) = (state.toasts, state.composer_open);
    let send = chip("Send", move |_: &mut S| {
        let message = composer_text().get_untracked();
        push_toast(toasts, format!("Comment: {message}"));
        composer_text().set(String::new());
        open.set(false);
    });
    container(Padding(
        EdgeInsets::all(10.0),
        row(vec![
            flexible(
                1,
                text_input(composer_text().get(), |_: &mut S, value| {
                    composer_text().set(value)
                })
                .placeholder("Leave a comment"),
            ),
            gap_h(8.0),
            inflexible(send),
        ]),
    ))
    .fill(scheme.surface_container)
}
