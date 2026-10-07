//! Checkbox: the reference's `CheckboxPlayground`.
//!
//! One preview (a checkbox beside its live checked/unchecked/indeterminate
//! label), one snippet, and one control panel: `Tristate`, `Error` —
//! `frust_material::checkbox`/[`tristate_checkbox`] carry the reference's
//! same tap-cycle order (`Some(false) -> Some(true) -> tristate ? None :
//! Some(false)`), so the tristate switch's own `if (!v && value == null)`
//! reset guard is ported verbatim into its `on_changed` closure below.
//!
//! # Descoped: `Enabled`
//!
//! The reference's third control gates `onChanged`, which also dims
//! `M3ECheckbox`'s own disabled colors. Neither `checkbox` nor
//! `tristate_checkbox` model a disabled state at all (`checkbox.rs`'s own
//! module docs, "No disabled state" — unlike [`super::switch`]'s
//! `.enabled()` builder), so there is no prop for the control to drive; it is
//! omitted here rather than faking a functional-only no-op.

use frust::{
    AnyView, Component, CrossAxisAlignment, Get, RwSignal, Set, SizedBox, View, any, component,
    row, text,
};
use frust_material::{checkbox, tristate_checkbox};

use crate::AppState;
use crate::catalog::DemoEntry;
use crate::widgets::playground::{
    ambient_theme, control_panel, play_preview_card, play_snippet, play_switch, playground_body,
};

/// See the page contract in [`crate::pages::playground`]. The reference's
/// `CheckboxPlayground` shows no page-level heading of its own — the section
/// list's row already carries `entry.title`, and a pushed route's app bar
/// titles it (`crate::pages::playground_scaffold`) — so `entry` goes unused
/// here, like every other real playground in this catalog.
pub fn page(_entry: DemoEntry) -> AnyView<AppState> {
    any(component(CheckboxPlayground))
}

/// Stateless configuration; all state lives in [`CheckboxPlaygroundState`].
struct CheckboxPlayground;

/// This page's own knobs — nested per the page contract, never on
/// [`AppState`].
struct CheckboxPlaygroundState {
    /// `None` is the indeterminate state, only reachable while `tristate`.
    value: RwSignal<Option<bool>>,
    tristate: RwSignal<bool>,
    error: RwSignal<bool>,
}

impl Component for CheckboxPlayground {
    type State = CheckboxPlaygroundState;

    fn init(&self) -> CheckboxPlaygroundState {
        CheckboxPlaygroundState {
            value: RwSignal::new(Some(true)),
            tristate: RwSignal::new(false),
            error: RwSignal::new(false),
        }
    }

    fn build(&self, state: &mut CheckboxPlaygroundState) -> impl View<CheckboxPlaygroundState> {
        let value = state.value.get();
        let tristate = state.tristate.get();
        let error = state.error.get();
        // Clamped exactly like the reference's `_tristate ? _value : (_value ?? false)`.
        let displayed = if tristate {
            value
        } else {
            Some(value.unwrap_or(false))
        };

        let preview = play_preview_card("Checkbox", preview_row(displayed, tristate, error));
        let snippet = play_snippet("Checkbox", checkbox_snippet(displayed, tristate, error));

        playground_body(
            vec![preview],
            vec![snippet],
            vec![control_panel(
                "State",
                vec![
                    play_switch(
                        "Tristate",
                        tristate,
                        |s: &mut CheckboxPlaygroundState, v: bool| {
                            s.tristate.set(v);
                            if !v && s.value.get().is_none() {
                                s.value.set(Some(false));
                            }
                        },
                    ),
                    play_switch(
                        "Error",
                        error,
                        |s: &mut CheckboxPlaygroundState, v: bool| s.error.set(v),
                    ),
                ],
            )],
        )
    }
}

/// The control beside its live state label — the reference's preview `Row`.
fn preview_row(
    value: Option<bool>,
    tristate: bool,
    error: bool,
) -> impl View<CheckboxPlaygroundState> {
    let theme = ambient_theme();
    let mut body = theme.type_scale.body_large.clone();
    body.color = theme.scheme().on_surface;
    let label = match value {
        None => "indeterminate",
        Some(true) => "checked",
        Some(false) => "unchecked",
    };

    let control: AnyView<CheckboxPlaygroundState> = if tristate {
        any(tristate_checkbox(
            value,
            |s: &mut CheckboxPlaygroundState, next: Option<bool>| s.value.set(next),
        )
        .error(error))
    } else {
        any(checkbox(
            value.unwrap_or(false),
            |s: &mut CheckboxPlaygroundState, next: bool| s.value.set(Some(next)),
        )
        .error(error))
    };

    row()
        .child(control)
        .child(SizedBox::<CheckboxPlaygroundState>(Some(12.0), None))
        .child(text(label).style(body))
        .cross_axis(CrossAxisAlignment::Center)
}

/// The paste-ready Frust equivalent of the current preview state.
fn checkbox_snippet(value: Option<bool>, tristate: bool, error: bool) -> String {
    if tristate {
        let value_text = match value {
            None => "None".to_string(),
            Some(v) => format!("Some({v})"),
        };
        format!(
            "frust_material::tristate_checkbox(\n    {value_text},\n    |state, next| state.value = next,\n)\n.error({error});"
        )
    } else {
        format!(
            "frust_material::checkbox(\n    {},\n    |state, next| state.checked = next,\n)\n.error({error});",
            value.unwrap_or(false)
        )
    }
}

#[cfg(test)]
mod tests {
    use frust::Component;

    use super::{CheckboxPlayground, CheckboxPlaygroundState};

    fn state() -> CheckboxPlaygroundState {
        CheckboxPlayground.init()
    }

    /// Every reachable `(value, tristate, error)` combination still builds a
    /// page, including the tristate-off reset guard this file's own
    /// `on_changed` closure applies.
    #[test]
    fn the_page_builds_across_every_knob_state() {
        use frust::Set;

        let mut s = state();
        for value in [None, Some(false), Some(true)] {
            for tristate in [false, true] {
                for error in [false, true] {
                    s.value.set(value);
                    s.tristate.set(tristate);
                    s.error.set(error);
                    let _view = CheckboxPlayground.build(&mut s);
                }
            }
        }
    }

    #[test]
    fn the_page_fn_builds_from_its_catalog_entry() {
        let entry = crate::catalog::find_by_id("checkbox").expect("catalog entry exists");
        let _view = super::page(entry);
    }
}
