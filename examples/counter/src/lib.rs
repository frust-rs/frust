//! The Phase 4A exit-criterion demo: an interactive counter (spec §14).
//!
//! Proves the whole 4A stack live — layout containers, interactive widgets, the
//! pointer pipeline, redraw wiring — with a single screen the user can click,
//! toggle, drag, and scroll. The identical UI is what `forgekit create` now
//! scaffolds (`templates/app/src/lib.rs.tmpl`), so this example and a fresh
//! project render the same thing.
//!
//! Phase 5.5: migrated to the [`Component`] model. [`CounterApp`] owns the
//! plain [`AppState`] below as its retained `Component::State` — the shape
//! (fields, `filler_rows`, `BASE_ROWS`/`EXTRA_ROWS`) is unchanged from the
//! pre-5.5 demo, and stays `pub` so the headless interaction test
//! (`tests/interaction.rs`) can drive it through the framework's `RenderRoot`
//! with synthetic input, exactly as before.
//!
//! `CounterApp::build` also nests a second, independently-stateful
//! [`CollapsibleSection`] component — the LOCAL-state showcase this phase's
//! `Component` model exists for: its expanded/collapsed flag lives in its own
//! retained `ComponentWidget`, not in `AppState`, so dragging the slider,
//! tapping +/-, or toggling "Extra rows" all rebuild `CounterApp`'s subtree
//! without ever touching it. Proven headlessly in
//! `tests/interaction.rs::local_section_state_survives_parent_rebuilds`.

use forgekit::{
    AnyView, Button, Checkbox, Column, Component, Row, SizedBox, Slider, any, component,
    scroll_view, text,
};

/// Counter demo state (spec §5 `app_logic` model).
///
/// The view is a pure function of these three fields; every interaction mutates
/// one of them and the next rebuild reflects it.
pub struct AppState {
    /// The counter value, driven by the −/+ buttons.
    pub count: i64,
    /// Whether the "Extra rows" checkbox is on — grows the filler list, proving
    /// rebuild-driven list length (the child count is a function of state).
    pub extra_rows: bool,
    /// The slider position, `0.0..=1.0`.
    pub slider: f64,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            count: 0,
            extra_rows: false,
            slider: 0.5,
        }
    }
}

/// Filler rows shown with "Extra rows" off — enough to overflow an 800×600
/// preview window so the list genuinely scrolls.
pub const BASE_ROWS: usize = 30;
/// Filler rows shown with "Extra rows" on — a bigger list to prove the row
/// count is a function of state, rebuilt from scratch each frame.
pub const EXTRA_ROWS: usize = 60;

/// Number of filler rows for the current state.
pub fn filler_rows(state: &AppState) -> usize {
    if state.extra_rows {
        EXTRA_ROWS
    } else {
        BASE_ROWS
    }
}

/// A nested, independently-stateful component: a collapsible "tips" section.
///
/// Its `State` (`bool`, expanded/collapsed) lives in this component's own
/// retained widget rather than in [`AppState`], so it is untouched by a
/// parent-driven rebuild — the LOCAL-state retention every `Component` gets
/// for free. See the module doc and `tests/interaction.rs`'s
/// `local_section_state_survives_parent_rebuilds` for the headless proof.
pub struct CollapsibleSection;

impl Component for CollapsibleSection {
    type State = bool;

    fn init(&self) -> bool {
        false
    }

    fn build(&self, state: &mut bool) -> AnyView<bool> {
        let expanded = *state;
        let mut children: Vec<AnyView<bool>> = Vec::with_capacity(3);
        children.push(any(Button(
            if expanded { "Hide tips" } else { "Show tips" },
            |s: &mut bool| *s = !*s,
        )));
        if expanded {
            children.push(any(text("Tip: drag the slider left-to-right.").size(16.0)));
            children.push(any(
                text("Tip: the checkbox grows the list below.").size(16.0)
            ));
        }
        any(Column(children))
    }
}

/// The counter demo's root [`Component`]: retained state is the plain
/// [`AppState`] above, unchanged in shape from the pre-Component demo.
pub struct CounterApp;

impl Component for CounterApp {
    type State = AppState;

    fn init(&self) -> AppState {
        AppState::default()
    }

    /// Pure view function: renders `AppState` into a scrollable counter screen
    /// (spec §5). Re-run every frame, so it is cheap by construction.
    fn build(&self, state: &mut AppState) -> AnyView<AppState> {
        let count = state.count;
        let extra = state.extra_rows;
        let slider_value = state.slider;
        let rows = filler_rows(state);

        let mut children: Vec<AnyView<AppState>> = Vec::with_capacity(rows + 5);

        children.push(any(text(format!("Count: {count}")).size(32.0)));
        children.push(any(Row(vec![
            any(Button("-", |s: &mut AppState| s.count -= 1)),
            any(SizedBox(Some(16.0), None)),
            any(Button("+", |s: &mut AppState| s.count += 1)),
        ])));
        children.push(any(Checkbox(
            extra,
            "Extra rows",
            |s: &mut AppState, on| {
                s.extra_rows = on;
            },
        )));
        children.push(any(Slider(slider_value, |s: &mut AppState, v| {
            s.slider = v;
        })));
        // The nested LOCAL-state component: its `expanded` flag survives every
        // rebuild triggered above (it is not a field of `AppState`).
        children.push(any(component(CollapsibleSection)));
        for i in 0..rows {
            children.push(any(text(format!("Filler row {}", i + 1)).size(20.0)));
        }

        any(scroll_view(Column(children)))
    }
}
