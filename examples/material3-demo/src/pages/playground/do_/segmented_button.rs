//! Segmented button: the reference's `SegmentedButtonPlayground`.
//!
//! # One delta from upstream: no "Gradient fill" preview
//!
//! [`mod@frust_material::segmented_button`]'s own module docs: dividers
//! paint solid, never the reference's sampled gradient, and
//! [`frust_material::SegmentedButtonView`] carries no gradient/theme-override
//! seam at all. The reference's second preview rethemes
//! `segmentedButtonTheme.selectedBackgroundGradient`/`outlineGradient`/
//! `dividerGradient`/`selectedForegroundGradient` on a copy of the ambient
//! theme — a knob this port has nothing to drive. Omitted, not stubbed.
//!
//! Knobs live in a page-local [`Knobs`], owned by the nested
//! [`SegmentedButtonPlayground`] `Component` (never [`AppState`]) per the
//! page contract in [`crate::pages::playground`]. Both controls are plain
//! switches, so — unlike the button-family pages in this section — this page
//! needs no [`frust_material::OverlayAnchor`]/outer `frust::Stack` at all.

use frust::{AnyView, Component, View, any, component};
use frust_material::{Segment, icons, segment, segmented_button};

use crate::AppState;
use crate::catalog::DemoEntry;
use crate::widgets::playground::{
    PlaySnippet, control_panel, play_preview_card, play_snippet, play_switch, playground_body,
};

/// This page's own knob state — held by [`SegmentedButtonPlayground`], never
/// [`AppState`] (the page contract in [`crate::pages::playground`]).
struct Knobs {
    multi_select: bool,
    show_selected_icon: bool,
    selected: Vec<usize>,
}

impl Default for Knobs {
    /// The reference's own `_SegmentedButtonPlaygroundState` field
    /// initializers.
    fn default() -> Self {
        Self {
            multi_select: false,
            show_selected_icon: true,
            selected: vec![0],
        }
    }
}

/// The fixed 3-segment set every preview shows — the reference's own
/// `_segments` (Day/Week/Month).
fn segments() -> Vec<Segment<usize>> {
    vec![
        segment(0_usize).label("Day").icon(icons::CALENDAR_TODAY),
        segment(1_usize)
            .label("Week")
            .icon(icons::CALENDAR_VIEW_WEEK),
        segment(2_usize).label("Month").icon(icons::CALENDAR_MONTH),
    ]
}

/// The reference's own guard on the "Multi select" switch: turning multi
/// select off collapses a multi-member selection down to its first member
/// (single-select's controlled `selected` may never carry more than one
/// value) — `PlaySwitch.onChanged`'s own `if (!v && _selected.length > 1)`.
fn on_multi_select_changed(state: &mut Knobs, next: bool) {
    state.multi_select = next;
    if !next && state.selected.len() > 1 {
        state.selected.truncate(1);
    }
}

/// The "Segmented button" preview.
fn preview(state: &Knobs) -> impl View<Knobs> {
    segmented_button(
        segments(),
        state.selected.clone(),
        |state: &mut Knobs, next: Vec<usize>| state.selected = next,
    )
    .multi_select(state.multi_select)
    .show_selected_icon(state.show_selected_icon)
}

/// "Behavior" controls: multi select, show selected icon.
fn behavior_panel(state: &Knobs) -> AnyView<Knobs> {
    control_panel(
        "Behavior",
        vec![
            play_switch("Multi select", state.multi_select, on_multi_select_changed),
            play_switch(
                "Show selected icon",
                state.show_selected_icon,
                |state: &mut Knobs, next: bool| state.show_selected_icon = next,
            ),
        ],
    )
}

/// The paste-ready snippet for the current knob state — the reference's
/// `_snippets`, in Frust rather than Dart.
fn snippet(state: &Knobs) -> PlaySnippet {
    play_snippet(
        "Segmented button",
        format!(
            "segmented_button(\n    vec![\n        \
             segment(0).label(\"Day\").icon(icons::CALENDAR_TODAY),\n        \
             segment(1).label(\"Week\").icon(icons::CALENDAR_VIEW_WEEK),\n        \
             segment(2).label(\"Month\").icon(icons::CALENDAR_MONTH),\n    ],\n    \
             {selected:?},\n    on_selection_changed,\n)\n    .multi_select({multi})\n    \
             .show_selected_icon({show_icon});",
            selected = state.selected,
            multi = state.multi_select,
            show_icon = state.show_selected_icon,
        ),
    )
}

/// The page body: one live preview, one static snippet, and the behavior
/// controls.
fn body(state: &Knobs) -> impl View<Knobs> {
    playground_body(
        vec![play_preview_card("Segmented button", preview(state))],
        vec![snippet(state)],
        vec![behavior_panel(state)],
    )
}

/// This page's knob component — see the [module docs](self).
struct SegmentedButtonPlayground;

impl Component for SegmentedButtonPlayground {
    type State = Knobs;

    fn init(&self) -> Self::State {
        Knobs::default()
    }

    fn build(&self, state: &mut Self::State) -> impl View<Self::State> {
        body(state)
    }
}

/// See the page contract in [`crate::pages::playground`].
pub fn page(_entry: DemoEntry) -> AnyView<AppState> {
    any(component(SegmentedButtonPlayground))
}

#[cfg(test)]
mod tests {
    use super::{Knobs, body, on_multi_select_changed, snippet};

    #[test]
    fn the_page_builds_with_the_default_selection() {
        let knobs = Knobs::default();
        let _view = body(&knobs);
    }

    #[test]
    fn the_page_builds_multi_select_with_two_members_selected() {
        let knobs = Knobs {
            multi_select: true,
            selected: vec![0, 2],
            ..Knobs::default()
        };
        let _view = body(&knobs);
    }

    #[test]
    fn the_page_builds_with_no_selection_and_the_icon_hidden() {
        let knobs = Knobs {
            show_selected_icon: false,
            selected: vec![],
            ..Knobs::default()
        };
        let _view = body(&knobs);
    }

    #[test]
    fn turning_multi_select_off_collapses_a_multi_selection_to_its_first_member() {
        let mut knobs = Knobs {
            multi_select: true,
            selected: vec![1, 2],
            ..Knobs::default()
        };
        on_multi_select_changed(&mut knobs, false);
        assert_eq!(knobs.selected, vec![1]);
        assert!(!knobs.multi_select);
    }

    #[test]
    fn turning_multi_select_on_leaves_a_single_selection_untouched() {
        let mut knobs = Knobs {
            selected: vec![1],
            ..Knobs::default()
        };
        on_multi_select_changed(&mut knobs, true);
        assert_eq!(knobs.selected, vec![1]);
        assert!(knobs.multi_select);
    }

    #[test]
    fn the_snippet_reflects_the_current_selection_and_switches() {
        let knobs = Knobs {
            multi_select: true,
            show_selected_icon: false,
            selected: vec![0, 1],
        };
        let snippet = snippet(&knobs);
        assert!(snippet.code.contains("[0, 1]"));
        assert!(snippet.code.contains("multi_select(true)"));
        assert!(snippet.code.contains("show_selected_icon(false)"));
    }
}
