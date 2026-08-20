//! The adaptive section host — one per navigation-bar destination.
//!
//! Below [`SPLIT_BREAKPOINT_PX`] the host *is* the list, and a row pushes its
//! playground as a full-screen route over the whole shell (chrome included),
//! exactly as the reference does. At or above it the host splits: a fixed
//! list pane, a hairline, and the selected entry's playground beside it.
//!
//! The breakpoint is read from `WindowMetrics` at build time — the framework's
//! documented responsive idiom (`examples/playground`'s responsive section is
//! the reference implementation of it). An absent `WindowMetrics` (no shell
//! has published yet) takes the narrow branch, like any genuinely narrow
//! window.

use frust::{
    AnyView, Axis, CrossAxisAlignment, FlexView, Get, RwSignal, SizedBox, Theme, WindowMetrics,
    any, flexible, inflexible, use_context,
};
use frust_material::divider;

use crate::AppState;
use crate::catalog::{self, DemoEntry, DemoSection, SPLIT_BREAKPOINT_PX};
use crate::pages::section_list;

/// Width of the split layout's list pane, in logical px — the reference's own
/// `SizedBox(width: 320)`.
const LIST_PANE_WIDTH: f64 = 320.0;

/// Every section's wide-layout selection, one signal each.
///
/// Owned by [`crate::AppState`] rather than by the host itself: a section's
/// page is rebuilt from its route builder on every pass, so the selection has
/// to outlive it.
#[derive(Clone, Copy)]
pub struct SectionSelection([RwSignal<Option<&'static str>>; DemoSection::ALL.len()]);

impl SectionSelection {
    /// Fresh signals, nothing selected — every section opens on its first
    /// entry until a row is tapped.
    pub fn new() -> Self {
        Self(DemoSection::ALL.map(|_| RwSignal::new(None)))
    }

    /// The selection signal for `section`.
    pub fn of(&self, section: DemoSection) -> RwSignal<Option<&'static str>> {
        self.0[section.index()]
    }
}

impl Default for SectionSelection {
    fn default() -> Self {
        Self::new()
    }
}

/// The section's page: a list, or a list beside the selected playground.
pub fn section_host(section: DemoSection, selection: SectionSelection) -> AnyView<AppState> {
    let wide = use_context::<WindowMetrics>()
        .map(|metrics| metrics.size.width >= SPLIT_BREAKPOINT_PX)
        .unwrap_or(false);
    let selection = selection.of(section);
    let selected = wide
        .then(|| resolve_selection(section, selection.get()))
        .flatten();
    let list = section_list(section, selection, selected.map(|entry| entry.id), wide);

    if !wide {
        return list;
    }

    let theme = use_context::<Theme>().unwrap_or_else(frust_material::baseline);
    let detail = match selected {
        Some(entry) => (entry.build)(entry),
        None => any(SizedBox::<AppState>(None, None)),
    };

    any(FlexView::new(
        Axis::Horizontal,
        vec![
            inflexible(SizedBox::<AppState>(Some(LIST_PANE_WIDTH), None).child(list)),
            inflexible(any(divider()
                .vertical()
                .color(theme.scheme().outline_variant))),
            flexible(1, detail),
        ],
    )
    .cross_axis(CrossAxisAlignment::Stretch))
}

/// The entry the split layout shows: the selected one, else the section's
/// first — the reference's own fallback chain, including "the selected id no
/// longer exists".
fn resolve_selection(section: DemoSection, selected_id: Option<&'static str>) -> Option<DemoEntry> {
    let entries = catalog::for_section(section);
    selected_id
        .and_then(|id| entries.iter().find(|entry| entry.id == id))
        .or_else(|| entries.first())
        .copied()
}

#[cfg(test)]
mod tests {
    use super::{SectionSelection, resolve_selection, section_host};
    use crate::catalog::{self, DemoSection};

    /// With no `WindowMetrics` published — a bare harness, or a genuinely
    /// narrow window — every section builds its list.
    #[test]
    fn every_section_builds_its_narrow_layout() {
        let selection = SectionSelection::new();
        for section in DemoSection::ALL {
            let _view = section_host(section, selection);
        }
    }

    #[test]
    fn no_selection_falls_back_to_the_sections_first_entry() {
        let first = catalog::for_section(DemoSection::View)[0];
        let resolved = resolve_selection(DemoSection::View, None).expect("the section has entries");
        assert_eq!(resolved.id, first.id);
    }

    #[test]
    fn a_stale_id_falls_back_instead_of_showing_nothing() {
        let resolved =
            resolve_selection(DemoSection::Nav, Some("no-such-entry")).expect("a fallback");
        assert_eq!(resolved.id, catalog::for_section(DemoSection::Nav)[0].id);
    }

    #[test]
    fn a_live_id_selects_its_own_entry() {
        let entry = catalog::for_section(DemoSection::Pick)[3];
        let resolved = resolve_selection(DemoSection::Pick, Some(entry.id)).expect("the entry");
        assert_eq!(resolved.id, entry.id);
    }
}
