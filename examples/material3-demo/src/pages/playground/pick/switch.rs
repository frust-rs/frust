//! Switch: the reference's `SwitchPlayground`.
//!
//! One preview (a switch beside its live On/Off label), one snippet, and two
//! control panels: `Appearance` (`Show icons`, `State layer size`) and
//! `State` (`Value`, `Enabled`). Every control maps cleanly onto
//! `frust_material::switch`'s own surface — unlike [`super::checkbox`]/
//! [`super::chips`], nothing here is descoped: `switch` carries a real
//! `.enabled()` builder (`switch.rs`'s own module docs), so `Enabled` wires
//! straight through it.

use frust::{
    AnyView, Component, CrossAxisAlignment, Get, RwSignal, Set, SizedBox, View, any, component,
    row, text,
};

use frust_material::{icons, switch};

use crate::AppState;
use crate::catalog::DemoEntry;
use crate::widgets::playground::{
    ambient_theme, control_panel, play_preview_card, play_slider, play_snippet, play_switch,
    playground_body,
};

/// The reference's `PlaySlider(min: 32, max: 64, divisions: 8)`.
const STATE_LAYER_RANGE: std::ops::RangeInclusive<f64> = 32.0..=64.0;
/// The reference's `PlaySlider(divisions: 8)`.
const STATE_LAYER_DIVISIONS: u32 = 8;

/// See the page contract in [`crate::pages::playground`]. No page-level
/// heading of its own — see [`super::checkbox::page`]'s doc for why `entry`
/// goes unused.
pub fn page(_entry: DemoEntry) -> AnyView<AppState> {
    any(component(SwitchPlayground))
}

/// Stateless configuration; all state lives in [`SwitchPlaygroundState`].
struct SwitchPlayground;

/// This page's own knobs — nested per the page contract, never on
/// [`AppState`].
struct SwitchPlaygroundState {
    value: RwSignal<bool>,
    enabled: RwSignal<bool>,
    show_icons: RwSignal<bool>,
    state_layer_size: RwSignal<f64>,
}

impl Component for SwitchPlayground {
    type State = SwitchPlaygroundState;

    fn init(&self) -> SwitchPlaygroundState {
        SwitchPlaygroundState {
            value: RwSignal::new(true),
            enabled: RwSignal::new(true),
            show_icons: RwSignal::new(true),
            state_layer_size: RwSignal::new(40.0),
        }
    }

    fn build(&self, state: &mut SwitchPlaygroundState) -> impl View<SwitchPlaygroundState> {
        let value = state.value.get();
        let enabled = state.enabled.get();
        let show_icons = state.show_icons.get();
        let state_layer_size = state.state_layer_size.get();

        let preview = play_preview_card(
            "Switch",
            preview_row(value, enabled, show_icons, state_layer_size),
        );
        let snippet = play_snippet(
            "Switch",
            switch_snippet(value, enabled, show_icons, state_layer_size),
        );

        playground_body(
            vec![preview],
            vec![snippet],
            vec![
                control_panel(
                    "Appearance",
                    vec![
                        play_switch(
                            "Show icons",
                            show_icons,
                            |s: &mut SwitchPlaygroundState, v: bool| s.show_icons.set(v),
                        ),
                        play_slider(
                            "State layer size",
                            state_layer_size,
                            STATE_LAYER_RANGE,
                            Some(STATE_LAYER_DIVISIONS),
                            |s: &mut SwitchPlaygroundState, v: f64| s.state_layer_size.set(v),
                        ),
                    ],
                ),
                control_panel(
                    "State",
                    vec![
                        play_switch("Value", value, |s: &mut SwitchPlaygroundState, v: bool| {
                            s.value.set(v)
                        }),
                        play_switch(
                            "Enabled",
                            enabled,
                            |s: &mut SwitchPlaygroundState, v: bool| s.enabled.set(v),
                        ),
                    ],
                ),
            ],
        )
    }
}

/// The control beside its live On/Off label — the reference's preview `Row`.
fn preview_row(
    value: bool,
    enabled: bool,
    show_icons: bool,
    state_layer_size: f64,
) -> AnyView<SwitchPlaygroundState> {
    let theme = ambient_theme();
    let mut body = theme.type_scale.body_large.clone();
    body.color = theme.scheme().on_surface;

    let mut control = switch(value, |s: &mut SwitchPlaygroundState, next: bool| {
        s.value.set(next)
    })
    .enabled(enabled)
    .state_layer_size(state_layer_size);
    if show_icons {
        control = control
            .selected_icon(icons::CHECK)
            .unselected_icon(icons::CLOSE);
    }

    any(row()
        .child(control)
        .child(SizedBox::<SwitchPlaygroundState>(Some(16.0), None))
        .child(text(if value { "On" } else { "Off" }).style(body))
        .cross_axis(CrossAxisAlignment::Center))
}

/// The paste-ready Frust equivalent of the current preview state.
fn switch_snippet(value: bool, enabled: bool, show_icons: bool, state_layer_size: f64) -> String {
    let icons_lines = if show_icons {
        "\n    .selected_icon(icons::CHECK)\n    .unselected_icon(icons::CLOSE)".to_string()
    } else {
        String::new()
    };
    let layer = if state_layer_size == state_layer_size.round() {
        format!("{state_layer_size:.0}")
    } else {
        format!("{state_layer_size}")
    };
    format!(
        "frust_material::switch({value}, |state, next| state.value = next)\n    .enabled({enabled}){icons_lines}\n    .state_layer_size({layer});"
    )
}

#[cfg(test)]
mod tests {
    use frust::Component;

    use super::{SwitchPlayground, SwitchPlaygroundState};

    fn state() -> SwitchPlaygroundState {
        SwitchPlayground.init()
    }

    /// Every reachable `(value, enabled, show_icons, state_layer_size)`
    /// combination still builds a page.
    #[test]
    fn the_page_builds_across_every_knob_state() {
        use frust::Set;

        let mut s = state();
        for value in [false, true] {
            for enabled in [false, true] {
                for show_icons in [false, true] {
                    for size in [32.0, 48.0, 64.0] {
                        s.value.set(value);
                        s.enabled.set(enabled);
                        s.show_icons.set(show_icons);
                        s.state_layer_size.set(size);
                        let _view = SwitchPlayground.build(&mut s);
                    }
                }
            }
        }
    }

    #[test]
    fn the_page_fn_builds_from_its_catalog_entry() {
        let entry = crate::catalog::find_by_id("switch").expect("catalog entry exists");
        let _view = super::page(entry);
    }
}
