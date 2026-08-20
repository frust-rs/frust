//! Snackbar: the reference's `SnackbarPlayground`.
//!
//! **One preview, not two.** The reference shows a raw, always-rendered
//! `M3ESnackbar(...)` value beside its `M3ESnackbar.show(...)` trigger —
//! `frust_material::snackbar` has no standalone counterpart to port that
//! first card from: every rendered bar is `SnackbarHostWidget`'s own
//! internal state, reachable only by driving a host through
//! [`SnackbarController::show`] (see that module's own docs — its one
//! documented mount contract, "wrapping the whole app root", is what this
//! page does at its own scope instead: the host wraps this page's body, its
//! controller lives in this page's knob state). Faking a live "inline"
//! preview by auto-showing a long-duration message would also drift from the
//! reference's own instant-`setState` reactivity — a knob edit would not
//! reach an already-showing bar (the crate's own queue/replace rule) — so
//! this page ports the one preview the public API actually supports.

use frust::{AnyView, Component, any, component};
use frust_material::{SnackbarController, button, snackbar, snackbar_host};

use crate::AppState;
use crate::catalog::DemoEntry;
use crate::widgets::playground::{
    control_panel, play_preview_card, play_snippet, play_switch, play_text_field, playground_body,
};

/// This playground's knobs — the reference's `_SnackbarPlaygroundState`.
struct Knobs {
    message: String,
    action_label: String,
    show_action: bool,
    /// This page's own host controller — see the module docs.
    toasts: SnackbarController<Knobs>,
}

impl Default for Knobs {
    fn default() -> Self {
        Self {
            message: "Draft saved".to_string(),
            action_label: "Undo".to_string(),
            show_action: true,
            toasts: SnackbarController::new(),
        }
    }
}

/// The FRUST snippet text for the current knob state — the reference's
/// `_snippets` getter (its `showSample`; the widget-only `M3ESnackbar(...)`
/// sample has no equivalent to port, per the module docs), ported to real
/// `frust_material` code rather than a Dart string.
fn snippet_code(state: &Knobs) -> String {
    if state.show_action {
        format!(
            "state.toasts.show(\n    snackbar({:?})\n        .action({:?}, |_| {{ /* revert */ }}),\n);",
            state.message, state.action_label
        )
    } else {
        format!("state.toasts.show(snackbar({:?}));", state.message)
    }
}

/// The playground body for the current knob state.
fn body(state: &mut Knobs) -> AnyView<Knobs> {
    let message = state.message.clone();
    let action_label = state.action_label.clone();
    let show_action = state.show_action;
    let trigger = button("Show snackbar", move |s: &mut Knobs| {
        let mut msg = snackbar(message.clone());
        if show_action {
            msg = msg.action(action_label.clone(), |_: &mut Knobs| {});
        }
        s.toasts.show(msg);
    });
    let preview = play_preview_card("Show overlay", trigger);
    let snippet = play_snippet("Show overlay", snippet_code(state));

    let mut rows: Vec<AnyView<Knobs>> = vec![
        play_text_field("Message", state.message.clone(), |s: &mut Knobs, v| {
            s.message = v
        }),
        play_switch("Show action", state.show_action, |s: &mut Knobs, v| {
            s.show_action = v
        }),
    ];
    if state.show_action {
        rows.push(play_text_field(
            "Action label",
            state.action_label.clone(),
            |s: &mut Knobs, v| s.action_label = v,
        ));
    }
    let controls = control_panel("Content", rows);

    playground_body(vec![preview], vec![snippet], vec![controls])
}

/// The nested [`Component`] this page owns its knobs in. See the page
/// contract in [`crate::pages::playground`].
#[derive(Default)]
struct SnackbarPlayground;

impl Component for SnackbarPlayground {
    type State = Knobs;

    fn init(&self) -> Knobs {
        Knobs::default()
    }

    fn build(&self, state: &mut Knobs) -> AnyView<Knobs> {
        let controller = state.toasts.clone();
        let content = body(state);
        any(snackbar_host(&controller, content))
    }
}

/// See the page contract in [`crate::pages::playground`].
pub fn page(_entry: DemoEntry) -> AnyView<AppState> {
    any(component(SnackbarPlayground))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every state this file's own controls can reach still builds a body,
    /// with or without the action row.
    #[test]
    fn the_page_builds_across_every_reachable_control_state() {
        let mut knobs = Knobs::default();
        let _view = body(&mut knobs);
        knobs.show_action = false;
        let _view = body(&mut knobs);
        knobs.show_action = true;
        knobs.message = "Copied".to_string();
        knobs.action_label = "Dismiss".to_string();
        let _view = body(&mut knobs);
    }

    #[test]
    fn the_snippet_includes_the_action_only_when_shown() {
        let mut knobs = Knobs::default();
        assert!(snippet_code(&knobs).contains(".action("));
        knobs.show_action = false;
        assert!(!snippet_code(&knobs).contains(".action("));
    }
}
