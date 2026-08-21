//! Side sheet: the reference's `SideSheetPlayground`.
//!
//! # Every knob is an [`RwSignal`], because this page's body is a nav page
//!
//! A [`frust::navigator`] captures its **root page builder once**, at build,
//! and re-runs *that* closure against live state on every later rebuild
//! (`frust_widgets::nav`'s reconcile loop) — it is never replaced by the
//! closure a later `Component::build` hands it. Plain fields cloned into the
//! closure would therefore freeze at their first-frame reading — this page
//! shipped exactly that bug until this fix — so `title`/`body`/`show_actions`
//! are signal handles the closure re-reads instead: the sanctioned
//! "something outside the component's own `build` observes this write" case
//! in `docs/CODE_STANDARDS.md`'s State & Reactivity conventions. The reads
//! all happen inside the rebuild pass, so the shell's own `TrackedScope`
//! subscribes to them exactly as it would to a read in `build`.
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
    AnyView, Component, EdgeInsets, Get, NavigatorController, Padding, PopResult, RwSignal, Set,
    any, component, navigator, text,
};
use frust_material::{
    ModalDismiss, filled_button, show_side_sheet, side_sheet, text_button, tonal_button,
};

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
/// onto. Every knob is a live handle — see the module docs.
struct Knobs {
    nav: NavigatorController<Knobs>,
    title: RwSignal<String>,
    body: RwSignal<String>,
    show_actions: RwSignal<bool>,
}

struct SideSheetPlayground;

impl Component for SideSheetPlayground {
    type State = Knobs;

    fn init(&self) -> Knobs {
        Knobs {
            nav: NavigatorController::new(),
            title: RwSignal::new(DEFAULT_TITLE.to_string()),
            body: RwSignal::new(DEFAULT_BODY.to_string()),
            show_actions: RwSignal::new(true),
        }
    }

    fn build(&self, state: &mut Knobs) -> AnyView<Knobs> {
        let nav = state.nav.clone();
        let title = state.title;
        let body = state.body;
        let show_actions = state.show_actions;
        any(navigator(&state.nav, move || {
            content(&nav, &title.get(), &body.get(), show_actions.get())
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
            // Minted once, here, outside the page builder below — which
            // re-runs on every navigator rebuild (see [`ModalDismiss`]).
            let dismiss = ModalDismiss::new();
            show_side_sheet(
                &nav,
                move || {
                    side_sheet(
                        title.clone(),
                        sheet_body::<Knobs>(&body),
                        sheet_actions::<Knobs>(&dismiss, show_actions),
                    )
                    .dismiss_handle(dismiss.clone())
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
/// both closing the sheet.
///
/// Both go through the sheet's [`ModalDismiss`] rather than
/// `NavigatorController::pop`: a raw pop takes the page out from under the
/// host with no exit ramp, so the sheet vanishes instead of sliding out the
/// way every other dismissal does.
fn sheet_actions<State: 'static>(
    dismiss: &ModalDismiss,
    show_actions: bool,
) -> Vec<AnyView<State>> {
    if !show_actions {
        return Vec::new();
    }
    let reset = dismiss.clone();
    let apply = dismiss.clone();
    vec![
        any(text_button("Reset", move |_: &mut State| reset.dismiss())),
        any(filled_button("Apply", move |_: &mut State| apply.dismiss())),
    ]
}

fn controls(title: &str, body: &str, show_actions: bool) -> AnyView<Knobs> {
    any(control_panel::<Knobs>(
        "Sheet",
        vec![
            play_text_field::<Knobs>(
                "Title",
                title.to_string(),
                |state: &mut Knobs, next: String| state.title.set(next),
            ),
            play_text_field::<Knobs>(
                "Body",
                body.to_string(),
                |state: &mut Knobs, next: String| state.body.set(next),
            ),
            play_switch::<Knobs>(
                "Show actions",
                show_actions,
                |state: &mut Knobs, next: bool| state.show_actions.set(next),
            ),
        ],
    ))
}

fn snippet(title: &str, body: &str, show_actions: bool) -> PlaySnippet {
    let actions = if show_actions {
        "\n\
         \u{20}   vec![\n\
         \u{20}       any(text_button(\"Reset\", move |_: &mut State| reset.dismiss())),\n\
         \u{20}       any(filled_button(\"Apply\", move |_: &mut State| apply.dismiss())),\n\
         \u{20}   ],"
    } else {
        "\n    Vec::new(),"
    };
    play_snippet(
        "Side sheet",
        format!(
            "let dismiss = ModalDismiss::new();\n\
             show_side_sheet(\n\
             \u{20}   &nav,\n\
             \u{20}   move || side_sheet({title:?}, text({body:?}),{actions}\n\
             \u{20}   )\n\
             \u{20}   .dismiss_handle(dismiss.clone()),\n\
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

    /// A write through any knob's signal must be visible to the next read
    /// the navigator's frozen closure would perform — the exact round trip
    /// `Component::build`'s `move || content(&nav, &title.get(), ..)` relies
    /// on.
    #[test]
    fn a_knob_write_through_the_signal_is_visible_to_the_next_read() {
        let knobs = Knobs {
            nav: NavigatorController::new(),
            title: RwSignal::new(DEFAULT_TITLE.to_string()),
            body: RwSignal::new(DEFAULT_BODY.to_string()),
            show_actions: RwSignal::new(true),
        };
        knobs.title.set("Updated title".to_string());
        knobs.body.set("Updated body".to_string());
        knobs.show_actions.set(false);
        assert_eq!(knobs.title.get(), "Updated title");
        assert_eq!(knobs.body.get(), "Updated body");
        assert!(!knobs.show_actions.get());
    }
}
