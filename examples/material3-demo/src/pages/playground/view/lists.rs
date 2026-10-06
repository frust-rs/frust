//! Lists: the reference's `ListsPlayground`.
//!
//! Four list shapes behind one `Kind` menu, mirroring the reference's own
//! `_ListKind` switch: a single [`frust_material::list_item()`], a
//! [`frust_material::card_list()`], a swipe-to-dismiss stack, and the
//! spring-revealed [`frust_material::expandable_list()`].
//!
//! # Descoped: `Card variant`
//!
//! The reference's second menu drives `M3ECardList(variant:)`.
//! [`mod@frust_material::card_list`]'s own module docs list `variant`/`border`
//! among the customization escape hatches this catalog does not port — a
//! card list paints the filled-card role (or an explicit
//! [`frust_material::CardListView::color`]), with no elevated/outlined
//! surface to switch to. This page shows that supported surface and omits the
//! control rather than mapping a variant onto a fill it does not mean.
//!
//! # Dismissible: two divergences, both the catalog's own
//!
//! * **A wrapper per row, not a list.** Upstream's `M3EDismissibleColumn`
//!   owns its rows; [`mod@frust_material::dismissible`] wraps exactly one child
//!   (its module docs' composability trade), so the preview is a plain
//!   `Column` of wrapped rows.
//! * **A dismissed row stays mounted.** `on_dismissed` fires at the *commit*,
//!   before the two-stage exit runs (that module's Exit section) — an app
//!   that removes the row inside the callback unmounts it mid-exit and sees
//!   no animation. This page keeps every row mounted (the documented
//!   kept-mounted shape) and records the commit as a caption instead, which
//!   is also what makes the contract visible in a playground.

use std::collections::BTreeSet;

use frust::{
    AnyView, Column, Component, CrossAxisAlignment, SizedBox, Stack, View, any, component, icon,
    text,
};
use frust_material::{
    DismissDirection, ExpandMode, MaterialSpacing, MaterialTokens, OverlayAnchor, card_list_items,
    dismiss_background, dismissible, expandable_item, expandable_list, icons, list_item,
    toggle_expanded, tonal_button,
};

use crate::AppState;
use crate::catalog::DemoEntry;
use crate::widgets::playground::{
    PlaySnippet, ambient_theme, control_panel, play_enum_menu_field, play_enum_menu_panel,
    play_preview_card, play_snippet, play_switch, play_text_field, playground_body,
};

/// The list shapes this page cycles through — the reference's `_ListKind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ListKind {
    Item,
    CardList,
    Dismissible,
    Expandable,
}

/// Every [`ListKind`], in the reference's own declaration order.
const KINDS: [ListKind; 4] = [
    ListKind::Item,
    ListKind::CardList,
    ListKind::Dismissible,
    ListKind::Expandable,
];

/// Menu/snippet label — the reference's `_ListKind.name`.
fn kind_label(kind: ListKind) -> &'static str {
    match kind {
        ListKind::Item => "item",
        ListKind::CardList => "cardList",
        ListKind::Dismissible => "dismissible",
        ListKind::Expandable => "expandable",
    }
}

/// Default headline — the reference's own `_headline` seed.
const DEFAULT_HEADLINE: &str = "Wireless charging";
/// Default supporting text — the reference's own `_supporting` seed.
const DEFAULT_SUPPORTING: &str = "On · Fast charge enabled";
/// How many rows the card-list and dismissible previews show — the
/// reference's own `itemCount: 3`.
const PREVIEW_ROWS: usize = 3;
/// The second expandable item's fixed content — the reference's own literal.
const SYSTEM_UPDATE_TITLE: &str = "System update";
const SYSTEM_UPDATE_SUBTITLE: &str = "Version 2.4.0 is ready";
const SYSTEM_UPDATE_BODY: &str = "Security fixes and performance improvements.";
/// The first expandable item's body copy — the reference's own literal.
const EXPANDED_BODY: &str = "Expanded body content for the list item.";
/// The dismissible rows' supporting text — the reference's own literal.
const SWIPE_SUPPORTING: &str = "Swipe to dismiss";

/// This page's knob state.
struct Knobs {
    kind: ListKind,
    headline: String,
    supporting: String,
    show_leading: bool,
    show_trailing: bool,
    /// The expandable list's app-owned open set (that widget is controlled).
    expanded: BTreeSet<usize>,
    /// The last committed dismissal — see the module docs.
    dismissed: Option<(usize, DismissDirection)>,
    /// Shared with [`kind_menu_panel`] — the "Kind" dropdown's anchor.
    kind_anchor: OverlayAnchor,
    kind_open: bool,
}

impl Default for Knobs {
    /// The reference's own `_ListsPlaygroundState` field initializers.
    fn default() -> Self {
        Self {
            kind: ListKind::Item,
            headline: DEFAULT_HEADLINE.to_string(),
            supporting: DEFAULT_SUPPORTING.to_string(),
            show_leading: true,
            show_trailing: true,
            expanded: BTreeSet::new(),
            dismissed: None,
            kind_anchor: OverlayAnchor::new(),
            kind_open: false,
        }
    }
}

struct ListsPlayground;

impl Component for ListsPlayground {
    type State = Knobs;

    fn init(&self) -> Knobs {
        Knobs::default()
    }

    fn build(&self, state: &mut Knobs) -> impl View<Knobs> {
        body(state)
    }
}

/// See the page contract in [`crate::pages::playground`].
pub fn page(_entry: DemoEntry) -> AnyView<AppState> {
    any(component(ListsPlayground))
}

/// The page body: the playground content plus the `Kind` dropdown panel it
/// anchors, stacked so the menu paints above the scrollable content (the
/// two-piece control's own contract — see [`crate::widgets::playground`]).
fn body(state: &Knobs) -> AnyView<Knobs> {
    let content = playground_body(
        vec![any(play_preview_card("List", preview(state)))],
        vec![snippet(state)],
        vec![controls(state)],
    );
    any(Stack(vec![content, kind_menu_panel(state)]))
}

/// The preview for the selected [`ListKind`] — the reference's own `switch`.
fn preview(state: &Knobs) -> AnyView<Knobs> {
    match state.kind {
        ListKind::Item => item_preview(state),
        ListKind::CardList => card_list_preview(state),
        ListKind::Dismissible => dismissible_preview(state),
        ListKind::Expandable => expandable_preview(state),
    }
}

/// One interactive row — the reference's `_ItemPreview`.
fn item_preview(state: &Knobs) -> AnyView<Knobs> {
    let mut row = list_item(state.headline.clone())
        .supporting(state.supporting.clone())
        .on_press(|_: &mut Knobs| {});
    if state.show_leading {
        row = row.leading(icon(icons::SCHEDULE));
    }
    if state.show_trailing {
        row = row.trailing(icon(icons::CHEVRON_RIGHT));
    }
    any(row)
}

/// Three card-backed rows — the reference's `_CardListPreview`.
fn card_list_preview(state: &Knobs) -> AnyView<Knobs> {
    let rows = (0..PREVIEW_ROWS).map(|index| {
        let mut row =
            list_item(format!("{} {index}", state.headline)).supporting(state.supporting.clone());
        if state.show_leading {
            row = row.leading(icon(icons::INBOX));
        }
        if state.show_trailing {
            row = row.trailing(icon(icons::CHEVRON_RIGHT));
        }
        row
    });
    any(card_list_items(rows))
}

/// Three swipe-to-dismiss rows plus the commit caption — the reference's
/// `_DismissiblePreview` (see the module docs for both divergences).
fn dismissible_preview(state: &Knobs) -> AnyView<Knobs> {
    let theme = ambient_theme();
    let semantic = theme
        .extension::<MaterialTokens>()
        .map(|tokens| *tokens.colors(theme.brightness));
    let mut caption = theme.type_scale.body_medium.clone();
    caption.color = theme.scheme().on_surface_variant;

    let mut rows: Vec<AnyView<Knobs>> = Vec::new();
    for index in 0..PREVIEW_ROWS {
        if index > 0 {
            rows.push(any(SizedBox::<Knobs>(None, Some(MaterialSpacing::XS))));
        }
        let mut row = list_item(format!("{} {index}", state.headline))
            .supporting(SWIPE_SUPPORTING)
            .contained(true);
        if state.show_leading {
            row = row.leading(icon(icons::SCHEDULE));
        }
        let mut forward = dismiss_background().icon(icons::CHECK);
        let mut backward = dismiss_background().icon(icons::CLOSE);
        if let Some(colors) = semantic {
            forward = forward.color(colors.success);
            backward = backward.color(colors.danger);
        }
        rows.push(any(dismissible(row)
            .background(forward)
            .secondary_background(backward)
            .on_dismissed(
                move |state: &mut Knobs, direction: DismissDirection| {
                    state.dismissed = Some((index, direction));
                },
            )));
    }
    rows.push(any(SizedBox::<Knobs>(None, Some(MaterialSpacing::MD))));
    rows.push(any(text(dismissed_caption(state)).style(caption)));

    any(Column(rows).cross_axis(CrossAxisAlignment::Stretch))
}

/// The caption under the dismissible rows: the last commit `on_dismissed`
/// reported, or the prompt before any.
fn dismissed_caption(state: &Knobs) -> String {
    match state.dismissed {
        Some((index, direction)) => {
            format!("Dismissed {} {index} ({direction:?})", state.headline)
        }
        None => "Swipe a row to fire on_dismissed".to_string(),
    }
}

/// Two disclosure items in accordion mode — the reference's
/// `_ExpandablePreview`.
fn expandable_preview(state: &Knobs) -> AnyView<Knobs> {
    let theme = ambient_theme();
    let mut body_style = theme.type_scale.body_medium.clone();
    body_style.color = theme.scheme().on_surface;

    let mut first_header = list_item(state.headline.clone()).supporting(state.supporting.clone());
    let mut second_header = list_item(SYSTEM_UPDATE_TITLE).supporting(SYSTEM_UPDATE_SUBTITLE);
    if state.show_leading {
        first_header = first_header.leading(icon(icons::BATTERY_ALERT));
        second_header = second_header.leading(icon(icons::SYSTEM_UPDATE));
    }

    let first_body = Column(vec![
        any(text(EXPANDED_BODY).style(body_style.clone())),
        any(SizedBox::<Knobs>(None, Some(MaterialSpacing::SM))),
        any(tonal_button("Action", |_: &mut Knobs| {})),
    ])
    .cross_axis(CrossAxisAlignment::Start);

    any(expandable_list(vec![
        expandable_item(first_header, first_body),
        expandable_item(second_header, text(SYSTEM_UPDATE_BODY).style(body_style)),
    ])
    .expanded(state.expanded.iter().copied())
    .on_toggle(|state: &mut Knobs, index: usize| {
        state.expanded = toggle_expanded(&state.expanded, index, ExpandMode::SingleOpen);
    }))
}

fn controls(state: &Knobs) -> AnyView<Knobs> {
    any(control_panel::<Knobs>(
        "Content",
        vec![
            play_enum_menu_field::<Knobs, ListKind>(
                "Kind",
                state.kind,
                &KINDS,
                kind_label,
                &state.kind_anchor,
                state.kind_open,
                |state: &mut Knobs, open: bool| state.kind_open = open,
            ),
            play_text_field::<Knobs>(
                "Headline",
                state.headline.clone(),
                |state: &mut Knobs, next: String| state.headline = next,
            ),
            play_text_field::<Knobs>(
                "Supporting",
                state.supporting.clone(),
                |state: &mut Knobs, next: String| state.supporting = next,
            ),
            play_switch::<Knobs>(
                "Leading",
                state.show_leading,
                |state: &mut Knobs, next: bool| state.show_leading = next,
            ),
            play_switch::<Knobs>(
                "Trailing",
                state.show_trailing,
                |state: &mut Knobs, next: bool| state.show_trailing = next,
            ),
        ],
    ))
}

/// The "Kind" menu's popup half — mounted at this page's outer [`Stack`].
fn kind_menu_panel(state: &Knobs) -> AnyView<Knobs> {
    play_enum_menu_panel::<Knobs, ListKind>(
        state.kind,
        &KINDS,
        kind_label,
        &state.kind_anchor,
        state.kind_open,
        |state: &mut Knobs, open: bool| state.kind_open = open,
        |state: &mut Knobs, next: ListKind| state.kind = next,
    )
}

fn snippet(state: &Knobs) -> PlaySnippet {
    let headline = &state.headline;
    let supporting = &state.supporting;
    let code = match state.kind {
        ListKind::Item => {
            let leading = optional_call(state.show_leading, "    .leading(icon(icons::SCHEDULE))");
            let trailing = optional_call(
                state.show_trailing,
                "    .trailing(icon(icons::CHEVRON_RIGHT))",
            );
            format!(
                "list_item({headline:?})\n\
                 \u{20}   .supporting({supporting:?})\
                 {leading}{trailing}\n\
                 \u{20}   .on_press(|_state| {{}});"
            )
        }
        ListKind::CardList => {
            let leading = optional_call(state.show_leading, "        .leading(icon(icons::INBOX))");
            let trailing = optional_call(
                state.show_trailing,
                "        .trailing(icon(icons::CHEVRON_RIGHT))",
            );
            format!(
                "card_list_items((0..3).map(|index| {{\n\
                 \u{20}   list_item(format!(\"{headline} {{index}}\"))\n\
                 \u{20}       .supporting({supporting:?})\
                 {leading}{trailing}\n\
                 }}));"
            )
        }
        ListKind::Dismissible => {
            let leading = optional_call(
                state.show_leading,
                "        .leading(icon(icons::SCHEDULE))",
            );
            format!(
                "dismissible(\n\
                 \u{20}   list_item(format!(\"{headline} {{index}}\"))\n\
                 \u{20}       .supporting({SWIPE_SUPPORTING:?})\
                 {leading}\n\
                 \u{20}       .contained(true),\n\
                 )\n\
                 .background(dismiss_background().icon(icons::CHECK))\n\
                 .secondary_background(dismiss_background().icon(icons::CLOSE))\n\
                 .on_dismissed(|state, direction| state.dismissed = Some((index, direction)));"
            )
        }
        ListKind::Expandable => {
            let leading = optional_call(
                state.show_leading,
                "    .leading(icon(icons::BATTERY_ALERT))",
            );
            format!(
                "let header = list_item({headline:?})\n\
                 \u{20}   .supporting({supporting:?})\
                 {leading};\n\
                 \n\
                 expandable_list(vec![\n\
                 \u{20}   expandable_item(header, text({EXPANDED_BODY:?})),\n\
                 ])\n\
                 .expanded(state.expanded.iter().copied())\n\
                 .on_toggle(|state, index| {{\n\
                 \u{20}   state.expanded = toggle_expanded(\n\
                 \u{20}       &state.expanded,\n\
                 \u{20}       index,\n\
                 \u{20}       ExpandMode::SingleOpen,\n\
                 \u{20}   );\n\
                 }});"
            )
        }
    };
    play_snippet("List", code)
}

/// A newline plus `line` when `shown`, nothing otherwise — one optional call
/// in a builder chain, the shape the reference's own inline
/// `${cond ? '\n  ...' : ''}` takes.
fn optional_call(shown: bool, line: &str) -> String {
    if shown {
        format!("\n{line}")
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_builds_across_every_reachable_control_state() {
        let mut knobs = Knobs::default();
        for kind in KINDS {
            for show_leading in [true, false] {
                for show_trailing in [true, false] {
                    knobs.kind = kind;
                    knobs.show_leading = show_leading;
                    knobs.show_trailing = show_trailing;
                    let _view = body(&knobs);
                }
            }
        }

        knobs.kind = ListKind::Item;
        knobs.kind_open = true;
        knobs.headline = String::new();
        knobs.supporting = String::new();
        let _view = body(&knobs);

        knobs.kind = ListKind::Expandable;
        knobs.expanded = BTreeSet::from([0]);
        let _view = body(&knobs);

        knobs.kind = ListKind::Dismissible;
        knobs.dismissed = Some((1, DismissDirection::EndToStart));
        let _view = body(&knobs);
    }

    #[test]
    fn the_accordion_set_keeps_at_most_one_item_open() {
        let opened = toggle_expanded(&BTreeSet::from([0]), 1, ExpandMode::SingleOpen);
        assert_eq!(opened, BTreeSet::from([1]));
    }

    #[test]
    fn the_caption_reports_the_last_commit() {
        let mut knobs = Knobs::default();
        assert!(dismissed_caption(&knobs).contains("Swipe a row"));
        knobs.dismissed = Some((2, DismissDirection::StartToEnd));
        let caption = dismissed_caption(&knobs);
        assert!(caption.contains("Dismissed"));
        assert!(caption.contains("StartToEnd"));
    }

    #[test]
    fn the_snippet_tracks_the_selected_kind() {
        let mut knobs = Knobs::default();
        assert!(snippet(&knobs).code.starts_with("list_item("));
        knobs.kind = ListKind::CardList;
        assert!(snippet(&knobs).code.starts_with("card_list_items("));
        knobs.kind = ListKind::Dismissible;
        assert!(snippet(&knobs).code.starts_with("dismissible("));
        knobs.kind = ListKind::Expandable;
        assert!(snippet(&knobs).code.contains("expandable_list(vec!["));
    }

    #[test]
    fn the_snippet_drops_the_slot_lines_the_switches_hide() {
        let mut knobs = Knobs::default();
        assert!(snippet(&knobs).code.contains(".leading("));
        knobs.show_leading = false;
        knobs.show_trailing = false;
        assert!(!snippet(&knobs).code.contains(".leading("));
        assert!(!snippet(&knobs).code.contains(".trailing("));
    }
}
