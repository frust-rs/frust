//! Side sheet: the reference's `SideSheetPlayground`.
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
use frust_material::{filled_button, show_side_sheet, side_sheet, text_button, tonal_button};

use crate::AppState;
use crate::catalog::DemoEntry;
use crate::widgets::playground::{
    PlaySnippet, ambient_theme, control_panel, play_preview_card, play_snippet, play_switch,
    play_text_field, playground_body,
};

/// Default sheet title — the reference's own `_title` seed.
const DEFAULT_TITLE: &str = "Filters";
/// Default sheet body text — the reference's own `_body` seed.
const DEFAULT_BODY: &str = "Side sheet content for detailed options.";

/// This page's knob state, plus the navigator [`show_side_sheet`] pushes
/// onto.
struct Knobs {
    nav: NavigatorController<Knobs>,
    title: String,
    body: String,
    show_actions: bool,
}

struct SideSheetPlayground;

impl Component for SideSheetPlayground {
    type State = Knobs;

    fn init(&self) -> Knobs {
        Knobs {
            nav: NavigatorController::new(),
            title: DEFAULT_TITLE.to_string(),
            body: DEFAULT_BODY.to_string(),
            show_actions: true,
        }
    }

    fn build(&self, state: &mut Knobs) -> AnyView<Knobs> {
        let nav = state.nav.clone();
        let title = state.title.clone();
        let body = state.body.clone();
        let show_actions = state.show_actions;
        any(navigator(&state.nav, move || {
            content(&nav, &title, &body, show_actions)
        }))
    }
}

/// See the page contract in [`crate::pages::playground`].
pub fn page(_entry: DemoEntry) -> AnyView<AppState> {
    any(component(SideSheetPlayground))
}

/// The playground body: the trigger preview, snippet, and controls —
/// everything that varies with this page's knobs, built without touching
/// the navigator (see the module docs).
fn content(
    nav: &NavigatorController<Knobs>,
    title: &str,
    body: &str,
    show_actions: bool,
) -> AnyView<Knobs> {
    playground_body(
        vec![trigger_preview(
            nav.clone(),
            title.to_string(),
            body.to_string(),
            show_actions,
        )],
        vec![snippet(title, body, show_actions)],
        vec![controls(title, body, show_actions)],
    )
}

fn trigger_preview(
    nav: NavigatorController<Knobs>,
    title: String,
    body: String,
    show_actions: bool,
) -> AnyView<Knobs> {
    any(play_preview_card(
        "Trigger",
        tonal_button("Show side sheet", move |_: &mut Knobs| {
            let title = title.clone();
            let body = body.clone();
            let actions_nav = nav.clone();
            show_side_sheet(
                &nav,
                move || {
                    side_sheet(
                        title.clone(),
                        sheet_body::<Knobs>(&body),
                        sheet_actions::<Knobs>(actions_nav.clone(), show_actions),
                    )
                },
                |_state: &mut Knobs, _result: PopResult| {},
            );
        }),
    ))
}

/// The sheet's own body — a single padded paragraph, the reference's
/// `Padding(EdgeInsets.all(24), Text(_body))`.
fn sheet_body<State: 'static>(body: &str) -> AnyView<State> {
    let theme = ambient_theme();
    let mut style = theme.type_scale.body_large.clone();
    style.color = theme.scheme().on_surface;
    any(Padding(
        EdgeInsets::all(24.0),
        text(body.to_string()).style(style),
    ))
}

/// The optional Reset/Apply footer — the reference's own two `M3EButton`s,
/// both popping the sheet.
fn sheet_actions<State: 'static>(
    nav: NavigatorController<State>,
    show_actions: bool,
) -> Vec<AnyView<State>> {
    if !show_actions {
        return Vec::new();
    }
    let reset_nav = nav.clone();
    vec![
        any(text_button("Reset", move |_: &mut State| reset_nav.pop())),
        any(filled_button("Apply", move |_: &mut State| nav.pop())),
    ]
}

fn controls(title: &str, body: &str, show_actions: bool) -> AnyView<Knobs> {
    any(control_panel::<Knobs>(
        "Sheet",
        vec![
            play_text_field::<Knobs>(
                "Title",
                title.to_string(),
                |state: &mut Knobs, next: String| state.title = next,
            ),
            play_text_field::<Knobs>(
                "Body",
                body.to_string(),
                |state: &mut Knobs, next: String| state.body = next,
            ),
            play_switch::<Knobs>(
                "Show actions",
                show_actions,
                |state: &mut Knobs, next: bool| state.show_actions = next,
            ),
        ],
    ))
}

fn snippet(title: &str, body: &str, show_actions: bool) -> PlaySnippet {
    let actions = if show_actions {
        "\n\
         \u{20}   vec![\n\
         \u{20}       any(text_button(\"Reset\", |_: &mut State| nav.pop())),\n\
         \u{20}       any(filled_button(\"Apply\", |_: &mut State| nav.pop())),\n\
         \u{20}   ],"
    } else {
        "\n    Vec::new(),"
    };
    play_snippet(
        "Side sheet",
        format!(
            "show_side_sheet(\n\
             \u{20}   &nav,\n\
             \u{20}   || side_sheet({title:?}, text({body:?}),{actions}\n\
             \u{20}   ),\n\
             \u{20}   |_state, _result| {{}},\n\
             );"
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_builds_across_every_knob_state() {
        let nav: NavigatorController<Knobs> = NavigatorController::new();
        for show_actions in [true, false] {
            let _view = content(&nav, DEFAULT_TITLE, DEFAULT_BODY, show_actions);
        }
        for title in ["", DEFAULT_TITLE, "A long side-sheet title to wrap"] {
            let _view = content(&nav, title, DEFAULT_BODY, true);
        }
    }
}
