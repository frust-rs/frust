//! Radio: the reference's `RadioPlayground`.
//!
//! One preview (three grouped radios: `standard`/`pro`/`team`), one snippet,
//! and one control panel: `Show labels`, `Error`, `Enabled`. All three map
//! cleanly onto `frust_material::radio`'s own surface — unlike
//! [`super::checkbox`]/[`super::chips`], nothing here is descoped: `radio`
//! genuinely has no disabled state modeled via a flag (`radio.rs`'s own
//! module docs), so `Enabled` is wired the way that module documents —
//! omitting [`frust_material::radio::RadioView::on_changed`] altogether
//! leaves a radio disabled (dimmed ring/dot, ignores every pointer event).

use frust::{
    AnyView, Column, Component, CrossAxisAlignment, EdgeInsets, Get, Padding, RwSignal, Set, View,
    any, component,
};
use frust_material::radio;

use crate::AppState;
use crate::catalog::DemoEntry;
use crate::widgets::playground::{
    control_panel, play_preview_card, play_snippet, play_switch, playground_body,
};

/// The reference's `_plans` group values.
const PLANS: [&str; 3] = ["standard", "pro", "team"];

/// See the page contract in [`crate::pages::playground`]. No page-level
/// heading of its own — see [`super::checkbox::page`]'s doc for why `entry`
/// goes unused.
pub fn page(_entry: DemoEntry) -> AnyView<AppState> {
    any(component(RadioPlayground))
}

/// Stateless configuration; all state lives in [`RadioPlaygroundState`].
struct RadioPlayground;

/// This page's own knobs — nested per the page contract, never on
/// [`AppState`].
struct RadioPlaygroundState {
    /// The group's current selection, one of [`PLANS`].
    plan: RwSignal<&'static str>,
    error: RwSignal<bool>,
    enabled: RwSignal<bool>,
    show_labels: RwSignal<bool>,
}

impl Component for RadioPlayground {
    type State = RadioPlaygroundState;

    fn init(&self) -> RadioPlaygroundState {
        RadioPlaygroundState {
            plan: RwSignal::new(PLANS[0]),
            error: RwSignal::new(false),
            enabled: RwSignal::new(true),
            show_labels: RwSignal::new(true),
        }
    }

    fn build(&self, state: &mut RadioPlaygroundState) -> impl View<RadioPlaygroundState> {
        let plan = state.plan.get();
        let error = state.error.get();
        let enabled = state.enabled.get();
        let show_labels = state.show_labels.get();

        let preview = play_preview_card(
            "Radio group",
            preview_column(plan, error, show_labels, enabled),
        );
        let snippet = play_snippet(
            "Radio group",
            radio_snippet(plan, error, enabled, show_labels),
        );

        playground_body(
            vec![preview],
            vec![snippet],
            vec![control_panel(
                "State",
                vec![
                    play_switch(
                        "Show labels",
                        show_labels,
                        |s: &mut RadioPlaygroundState, v: bool| s.show_labels.set(v),
                    ),
                    play_switch("Error", error, |s: &mut RadioPlaygroundState, v: bool| {
                        s.error.set(v)
                    }),
                    play_switch(
                        "Enabled",
                        enabled,
                        |s: &mut RadioPlaygroundState, v: bool| s.enabled.set(v),
                    ),
                ],
            )],
        )
    }
}

/// The three grouped radios, top to bottom — the reference's preview
/// `Column`.
fn preview_column(
    current: &'static str,
    error: bool,
    show_labels: bool,
    enabled: bool,
) -> impl View<RadioPlaygroundState> {
    let rows: Vec<_> = PLANS
        .iter()
        .map(|&plan| radio_row(plan, current, error, show_labels, enabled))
        .collect();
    Column(rows).cross_axis(CrossAxisAlignment::Start)
}

/// One grouped radio, 4px above the next — the reference's
/// `Padding(bottom: 4)`.
fn radio_row(
    plan: &'static str,
    current: &'static str,
    error: bool,
    show_labels: bool,
    enabled: bool,
) -> impl View<RadioPlaygroundState> {
    let mut view = radio::<RadioPlaygroundState, &'static str>(plan, current).error(error);
    if show_labels {
        view = view.label(plan);
    }
    if enabled {
        view = view.on_changed(|s: &mut RadioPlaygroundState, next: &'static str| s.plan.set(next));
    }
    Padding(
        EdgeInsets {
            left: 0.0,
            top: 0.0,
            right: 0.0,
            bottom: 4.0,
        },
        view,
    )
}

/// The paste-ready Frust equivalent of the currently-selected radio's state.
fn radio_snippet(plan: &str, error: bool, enabled: bool, show_labels: bool) -> String {
    let label_line = if show_labels {
        format!("\n    .label({plan:?})")
    } else {
        String::new()
    };
    let changed_line = if enabled {
        "\n    .on_changed(|state, next| state.plan = next)".to_string()
    } else {
        String::new()
    };
    format!(
        "frust_material::radio({plan:?}, {plan:?})\n    .error({error}){label_line}{changed_line};"
    )
}

#[cfg(test)]
mod tests {
    use frust::Component;

    use super::{PLANS, RadioPlayground, RadioPlaygroundState};

    fn state() -> RadioPlaygroundState {
        RadioPlayground.init()
    }

    /// Every reachable `(plan, error, enabled, show_labels)` combination
    /// still builds a page.
    #[test]
    fn the_page_builds_across_every_knob_state() {
        use frust::Set;

        let mut s = state();
        for &plan in &PLANS {
            for error in [false, true] {
                for enabled in [false, true] {
                    for show_labels in [false, true] {
                        s.plan.set(plan);
                        s.error.set(error);
                        s.enabled.set(enabled);
                        s.show_labels.set(show_labels);
                        let _view = RadioPlayground.build(&mut s);
                    }
                }
            }
        }
    }

    #[test]
    fn the_page_fn_builds_from_its_catalog_entry() {
        let entry = crate::catalog::find_by_id("radio").expect("catalog entry exists");
        let _view = super::page(entry);
    }
}
