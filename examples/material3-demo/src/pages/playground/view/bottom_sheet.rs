//! Bottom sheet: the reference's `BottomSheetPlayground`.
//!
//! # Descoped: `showDragHandle`
//!
//! The reference's `_showDragHandle` knob toggles `M3EBottomSheet.show`'s own
//! `showDragHandle` parameter. `frust_material::bottom_sheet`'s drag handle
//! is not optional — the module doc's Panel anatomy section: "a centered drag
//! handle ... " is unconditional, with no builder to turn it off — so this
//! page shows the supported (always-on) handle and omits the control
//! entirely rather than inventing one.
//!
//! # Why `page`/`Component::build` aren't tested directly
//!
//! `Component::build` mounts a [`frust::navigator`] (the pattern
//! [`crate::pages::playground`]'s module docs point to for an overlay-owning
//! page), which auto-wires back handling against the process's running
//! reactive runtime — the same reason `main.rs` never builds its own
//! navigator-mounted shell view in a host-side test. [`content`] below is the
//! part that actually varies with this page's knob state, built with no
//! navigator touched, and is what this file's tests exercise instead.

use frust::{
    AnyView, Component, EdgeInsets, NavigatorController, Padding, PopResult, any, component,
    navigator, text,
};
use frust_material::{bottom_sheet, show_bottom_sheet, tonal_button};

use crate::AppState;
use crate::catalog::DemoEntry;
use crate::widgets::playground::{
    PlaySnippet, ambient_theme, control_panel, play_preview_card, play_snippet, play_text_field,
    playground_body,
};

/// Default sheet body text — the reference's own `_body` seed.
const DEFAULT_BODY: &str = "A modal bottom sheet with a drag handle.";

/// This page's knob state, plus the navigator [`show_bottom_sheet`] pushes
/// onto.
struct Knobs {
    nav: NavigatorController<Knobs>,
    body: String,
}

struct BottomSheetPlayground;

impl Component for BottomSheetPlayground {
    type State = Knobs;

    fn init(&self) -> Knobs {
        Knobs {
            nav: NavigatorController::new(),
            body: DEFAULT_BODY.to_string(),
        }
    }

    fn build(&self, state: &mut Knobs) -> AnyView<Knobs> {
        let nav = state.nav.clone();
        let body = state.body.clone();
        any(navigator(&state.nav, move || content(&nav, &body)))
    }
}

/// See the page contract in [`crate::pages::playground`].
pub fn page(_entry: DemoEntry) -> AnyView<AppState> {
    any(component(BottomSheetPlayground))
}

/// The playground body: the trigger preview, snippet, and controls —
/// everything that varies with `body`, built without touching the
/// navigator (see the module docs).
fn content(nav: &NavigatorController<Knobs>, body: &str) -> AnyView<Knobs> {
    playground_body(
        vec![trigger_preview(nav.clone(), body.to_string())],
        vec![snippet(body)],
        vec![controls(body)],
    )
}

fn trigger_preview(nav: NavigatorController<Knobs>, body: String) -> AnyView<Knobs> {
    any(play_preview_card(
        "Trigger",
        tonal_button("Show bottom sheet", move |_: &mut Knobs| {
            let body = body.clone();
            show_bottom_sheet(
                &nav,
                move || bottom_sheet(sheet_content::<Knobs>(&body)),
                |_state: &mut Knobs, _result: PopResult| {},
            );
        }),
    ))
}

/// The sheet's own content — a single padded paragraph, the reference's
/// `Padding(EdgeInsets.all(24), Text(_body))`.
fn sheet_content<State: 'static>(body: &str) -> AnyView<State> {
    let theme = ambient_theme();
    let mut style = theme.type_scale.body_large.clone();
    style.color = theme.scheme().on_surface;
    any(Padding(
        EdgeInsets::all(24.0),
        text(body.to_string()).style(style),
    ))
}

fn controls(body: &str) -> AnyView<Knobs> {
    any(control_panel::<Knobs>(
        "Sheet",
        vec![play_text_field::<Knobs>(
            "Body",
            body.to_string(),
            |state: &mut Knobs, next: String| state.body = next,
        )],
    ))
}

fn snippet(body: &str) -> PlaySnippet {
    play_snippet(
        "Bottom sheet",
        format!(
            "show_bottom_sheet(\n\
             \u{20}   &nav,\n\
             \u{20}   || bottom_sheet(text({body:?})),\n\
             \u{20}   |_state, _result| {{}},\n\
             );"
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_builds_across_every_body_value() {
        let nav: NavigatorController<Knobs> = NavigatorController::new();
        for body in ["", DEFAULT_BODY, "Custom body text for a resized sheet."] {
            let _view = content(&nav, body);
        }
    }
}
