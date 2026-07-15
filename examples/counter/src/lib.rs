//! The Phase 4A exit-criterion demo: an interactive counter (spec §14).
//!
//! Proves the whole 4A stack live — layout containers, interactive widgets, the
//! pointer pipeline, redraw wiring — with a single screen the user can click,
//! toggle, drag, and scroll. The identical UI is what `forgekit create` now
//! scaffolds (`templates/app/src/lib.rs.tmpl`), so this example and a fresh
//! project render the same thing.
//!
//! `AppState`/`app_logic` are `pub` so the headless interaction test
//! (`tests/interaction.rs`) can drive them through the framework's `RenderRoot`
//! with synthetic input — the same event seam the desktop shell feeds.

use forgekit::{
    AnyView, Button, Checkbox, Column, Row, SizedBox, Slider, View, any, scroll_view, text,
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

/// Pure view function: renders `AppState` into a scrollable counter screen
/// (spec §5). Re-run every frame, so it is cheap by construction.
///
/// `+ use<>` opts the return type out of edition-2024's implicit lifetime
/// capture — the view borrows nothing from `state` (views are `'static`).
pub fn app_logic(state: &mut AppState) -> impl View<AppState> + use<> {
    let count = state.count;
    let extra = state.extra_rows;
    let slider_value = state.slider;
    let rows = filler_rows(state);

    let mut children: Vec<AnyView<AppState>> = Vec::with_capacity(rows + 4);

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
    for i in 0..rows {
        children.push(any(text(format!("Filler row {}", i + 1)).size(20.0)));
    }

    scroll_view(Column(children))
}
