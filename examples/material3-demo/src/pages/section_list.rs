//! A section's list pane: every catalog entry filed under it, as a card list.

use frust::{
    AnyView, EdgeInsets, Padding, RwSignal, Set, Theme, any, icon, scroll_view, use_context,
};
use frust_material::{card_list_items, icons, list_item};

use crate::AppState;
use crate::catalog::{self, DemoSection};

/// Padding around the list, matching the reference's own `ListView` inset.
const LIST_PADDING: f64 = 16.0;

/// The list of entries in `section`.
///
/// `wide` decides what a row does: in the split layout it selects (writing
/// `selection`, which the host reads back for its detail pane); otherwise it
/// pushes that entry's playground route, and every row carries the trailing
/// chevron that advertises it.
pub fn section_list(
    section: DemoSection,
    selection: RwSignal<Option<&'static str>>,
    selected_id: Option<&'static str>,
    wide: bool,
) -> AnyView<AppState> {
    let theme = use_context::<Theme>().unwrap_or_else(frust_material::baseline);
    let chevron = theme.scheme().on_surface_variant;
    let entries = catalog::for_section(section);

    let rows = entries.iter().map(|entry| {
        let row = list_item::<AppState>(entry.title)
            .supporting(entry.subtitle)
            .leading(icon(entry.icon))
            .selected(wide && selected_id == Some(entry.id));
        if wide {
            row
        } else {
            row.trailing(icon(icons::CHEVRON_RIGHT).color(chevron))
        }
    });

    let selected_index = selected_id
        .filter(|_| wide)
        .and_then(|id| entries.iter().position(|entry| entry.id == id));

    any(scroll_view(Padding(
        EdgeInsets::all(LIST_PADDING),
        card_list_items(rows).selected(selected_index).on_press(
            move |state: &mut AppState, index: usize| {
                let Some(entry) = catalog::for_section(section).get(index).copied() else {
                    return;
                };
                if wide {
                    selection.set(Some(entry.id));
                } else {
                    state.router.push(&entry.route());
                }
            },
        ),
    )))
}
