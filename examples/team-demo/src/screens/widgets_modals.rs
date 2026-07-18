//! Widgets · Modals screen — REAL (ported from `examples/catalog` in task 03).
//!
//! The catalog's Modals tab: four buttons driving [`show_dialog`]/
//! [`show_bottom_sheet`]/[`show_cupertino_alert`]/[`show_action_sheet`], each
//! landing its [`PopResult`] in a "last result" banner.
//!
//! # Shape
//!
//! The `show_*` helpers push a transparent modal page over a *mounted*
//! [`NavigatorController`]. `routes.rs` (a finalized shared file) calls this
//! screen no-arg and never hands it the shell's own controller, so the screen
//! is a nested [`Component`] that owns its own [`NavigatorController`] and hosts
//! its own [`navigator`]: the four modals push over that inner navigator's base
//! page (covering just this screen's content region, leaving the persistent
//! top/bottom bars visible), and their `PopResult`s round-trip back into the
//! banner. The controller + banner signal live in the component's [retained
//! state](ModalsState) so they survive the router's per-rebuild page rebuild.

use forgekit::{
    AnyView, Button, Column, Component, CupertinoActionStyle, Get, NavigatorController, PopResult,
    RwSignal, Set, SizedBox, action, any, bottom_sheet, component, dialog, navigator, scroll_view,
    show_action_sheet, show_bottom_sheet, show_cupertino_alert, show_dialog, text,
};

use crate::ShellState;

/// The Modals exhibit route entry point: a nested [`Component`] owning its own
/// navigator so the `show_*` helpers have a controller to push over (see the
/// [module docs](self)).
pub fn modals_screen() -> AnyView<ShellState> {
    any(component(ModalsExhibit::default()))
}

/// The Modals exhibit's retained state: the inner navigator's controller (the
/// `show_*` helpers push over it) plus the last-result banner signal.
struct ModalsState {
    controller: NavigatorController<ModalsState>,
    last_result: RwSignal<Option<String>>,
}

/// The Modals exhibit component: stateless configuration; the controller +
/// banner signal live in its [`ModalsState`].
#[derive(Default)]
struct ModalsExhibit;

impl Component for ModalsExhibit {
    type State = ModalsState;

    fn init(&self) -> ModalsState {
        ModalsState {
            controller: NavigatorController::new(),
            last_result: RwSignal::new(None),
        }
    }

    fn build(&self, state: &mut ModalsState) -> AnyView<ModalsState> {
        let controller = state.controller.clone();
        // A separate clone for the page-builder closure (re-invoked each
        // rebuild) so it doesn't fight the `&controller` borrow `navigator`
        // itself takes.
        let page_controller = controller.clone();
        let last_result = state.last_result;

        any(navigator(&controller, move || {
            modals_base_page(page_controller.clone(), last_result)
        }))
    }
}

/// The inner navigator's base page: the four modal-trigger buttons plus the
/// "last result" banner. Every `show_*` helper pushes over `controller` and
/// surfaces its [`PopResult`] into `last_result`.
fn modals_base_page(
    controller: NavigatorController<ModalsState>,
    last_result: RwSignal<Option<String>>,
) -> AnyView<ModalsState> {
    let dialog_controller = controller.clone();
    let sheet_controller = controller.clone();
    let alert_controller = controller.clone();
    let action_sheet_controller = controller;

    any(scroll_view(Column(vec![
        any(text("Modals").size(24.0)),
        any(text(format!(
            "Last result: {}",
            last_result.get().unwrap_or_else(|| "(none)".to_string())
        ))
        .size(16.0)),
        any(text(
            "Cupertino Alert/Action Sheet render on the iOS-27 glass material \
             (task 6f-14) — trigger them below to see it.",
        )
        .size(12.0)),
        any(Button("Show Dialog", move |_s: &mut ModalsState| {
            let confirm_ctrl = dialog_controller.clone();
            let cancel_ctrl = dialog_controller.clone();
            show_dialog(
                &dialog_controller,
                move || {
                    let confirm_ctrl = confirm_ctrl.clone();
                    let cancel_ctrl = cancel_ctrl.clone();
                    dialog::<ModalsState>()
                        .title("Delete item?")
                        .body("This action can't be undone.")
                        .action(any(Button("Cancel", move |_s: &mut ModalsState| {
                            cancel_ctrl.pop();
                        })))
                        .action(any(Button("Confirm", move |_s: &mut ModalsState| {
                            confirm_ctrl.pop_with_result(PopResult::of("confirmed".to_string()));
                        })))
                },
                move |_s: &mut ModalsState, result: PopResult| {
                    let msg = result
                        .take::<String>()
                        .unwrap_or_else(|| "dismissed".to_string());
                    last_result.set(Some(format!("Dialog: {msg}")));
                },
            );
        })),
        any(SizedBox(None, Some(8.0))),
        any(Button("Show Bottom Sheet", move |_s: &mut ModalsState| {
            let close_ctrl = sheet_controller.clone();
            show_bottom_sheet(
                &sheet_controller,
                move || {
                    let close_ctrl = close_ctrl.clone();
                    bottom_sheet(Column(vec![
                        any(text("Bottom sheet content").size(18.0)),
                        any(Button("Close", move |_s: &mut ModalsState| {
                            close_ctrl.pop()
                        })),
                    ]))
                },
                move |_s: &mut ModalsState, _result: PopResult| {
                    last_result.set(Some("Bottom sheet dismissed".to_string()));
                },
            );
        })),
        any(SizedBox(None, Some(8.0))),
        any(Button(
            "Show Cupertino Alert",
            move |_s: &mut ModalsState| {
                show_cupertino_alert(
                    &alert_controller,
                    "iOS Alert",
                    Some("This is a Cupertino-style alert.".to_string()),
                    vec![
                        action("Cancel").style(CupertinoActionStyle::Cancel),
                        action("Delete").style(CupertinoActionStyle::Destructive),
                    ],
                    move |_s: &mut ModalsState, result: PopResult| {
                        let idx = result.take::<usize>();
                        last_result.set(Some(format!("Cupertino alert action: {idx:?}")));
                    },
                );
            },
        )),
        any(SizedBox(None, Some(8.0))),
        any(Button("Show Action Sheet", move |_s: &mut ModalsState| {
            show_action_sheet(
                &action_sheet_controller,
                vec![
                    action("Share"),
                    action("Delete").style(CupertinoActionStyle::Destructive),
                ],
                Some("Cancel".to_string()),
                move |_s: &mut ModalsState, result: PopResult| {
                    let idx = result.take::<usize>();
                    last_result.set(Some(format!("Action sheet action: {idx:?}")));
                },
            );
        })),
    ])))
}
