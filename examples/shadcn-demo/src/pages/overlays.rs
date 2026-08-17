//! Overlays: the modal family (dialog, alert-dialog, sheet, drawer, command
//! dialog), each mounted the primary documented way — pushed as a
//! transparent navigator page through `overlay::show_*` from a trigger
//! button's `on_press`, never built from the render path itself. A
//! `show_*` call's `build` closure is captured once, at push time; every
//! frozen literal it carries stays identical across the pushed page's
//! rebuilds, which is what lets a live-edited control inside it (the
//! command dialog's search field) keep its own typed text rather than being
//! fought back to the push-time snapshot on every pass.
//!
//! The navigator is this page's outermost view and the trigger list scrolls
//! inside its root page: a pushed modal page inherits the navigator's own
//! constraints, so those have to be the page slot's bounded ones.

use frust::{Column, NavigatorController, PopResult, SizedBox, View, any, navigator, text};
use frust_shadcn::{
    ButtonVariant, DrawerSide, alert_dialog, alert_dialog_action, alert_dialog_cancel,
    alert_dialog_description, alert_dialog_footer, alert_dialog_header, alert_dialog_title, button,
    command, command_dialog, command_item, dialog, dialog_description, dialog_footer,
    dialog_header, dialog_title, drawer, drawer_description, drawer_footer, drawer_header,
    drawer_title, sheet, sheet_description, sheet_footer, sheet_header, sheet_title,
    show_alert_dialog, show_command_dialog, show_dialog, show_drawer, show_sheet,
};

use crate::AppState;

pub struct State {
    pub controller: NavigatorController<AppState>,
    pub last_result: String,
}

impl Default for State {
    fn default() -> Self {
        Self {
            controller: NavigatorController::new(),
            last_result: "(nothing dismissed yet)".to_string(),
        }
    }
}

fn gap() -> frust::AnyView<AppState> {
    any(SizedBox(None, Some(12.0)))
}

/// Record `result` (via `PopResult::take::<&str>`-style tagging) as the last
/// overlay outcome, so a person driving the desktop gate can see the
/// callback actually ran.
fn record(label: &'static str) -> impl Fn(&mut AppState, PopResult) {
    move |s: &mut AppState, result: PopResult| {
        let confirmed = result.take::<bool>();
        s.overlays.last_result = format!("{label}: {confirmed:?}");
    }
}

pub fn page(state: &mut State) -> impl View<AppState> + use<> {
    let controller = state.controller.clone();
    let last_result = state.last_result.clone();

    let root_controller = controller.clone();
    navigator(&controller, move || {
        let dialog_ctrl = root_controller.clone();
        let alert_ctrl = root_controller.clone();
        let sheet_ctrl = root_controller.clone();
        let drawer_ctrl = root_controller.clone();
        let command_ctrl = root_controller.clone();

        // The scroll slot is *inside* the navigator's root page, never around
        // the navigator: a pushed modal page is laid out under the navigator's
        // own constraints, and a scroll view's infinite max height would
        // collapse every panel it hosts to nothing.
        crate::scroll_slot(any(Column(vec![
            any(crate::nav::heading("Overlays")),
            any(SizedBox(None, Some(16.0))),
            any(text(format!("Last result: {last_result}")).size(14.0)),
            gap(),
            any(button("Open Dialog", move |_: &mut AppState| {
                let cancel_ctrl = dialog_ctrl.clone();
                show_dialog(
                    &dialog_ctrl,
                    move || {
                        let cancel_ctrl = cancel_ctrl.clone();
                        dialog(vec![
                            dialog_header(vec![
                                dialog_title("Delete project?"),
                                dialog_description("This action cannot be undone."),
                            ]),
                            dialog_footer(vec![any(button("Cancel", move |_: &mut AppState| {
                                cancel_ctrl.pop()
                            })
                            .variant(ButtonVariant::Outline))]),
                        ])
                    },
                    record("dialog"),
                );
            })),
            gap(),
            any(button("Open Alert Dialog", move |_: &mut AppState| {
                let action_ctrl = alert_ctrl.clone();
                let cancel_ctrl = alert_ctrl.clone();
                show_alert_dialog(
                    &alert_ctrl,
                    move || {
                        let action_ctrl = action_ctrl.clone();
                        let cancel_ctrl = cancel_ctrl.clone();
                        alert_dialog(vec![
                            alert_dialog_header(vec![
                                alert_dialog_title("Are you absolutely sure?"),
                                alert_dialog_description(
                                    "This will permanently delete the selected item.",
                                ),
                            ]),
                            alert_dialog_footer(vec![
                                alert_dialog_cancel(any(button(
                                    "Cancel",
                                    move |_: &mut AppState| {
                                        cancel_ctrl.pop_with_result(PopResult::of(false));
                                    },
                                )
                                .variant(ButtonVariant::Outline))),
                                alert_dialog_action(any(button(
                                    "Continue",
                                    move |_: &mut AppState| {
                                        action_ctrl.pop_with_result(PopResult::of(true));
                                    },
                                )
                                .variant(ButtonVariant::Destructive))),
                            ]),
                        ])
                    },
                    record("alert_dialog"),
                );
            })),
            gap(),
            any(button("Open Sheet", move |_: &mut AppState| {
                show_sheet(
                    &sheet_ctrl,
                    || {
                        sheet(vec![
                            sheet_header(vec![
                                sheet_title("Edit profile"),
                                sheet_description("A right-edge panel over the app."),
                            ]),
                            sheet_footer(vec![any(text("Close with Escape or the scrim."))]),
                        ])
                    },
                    record("sheet"),
                );
            })),
            gap(),
            any(button("Open Drawer", move |_: &mut AppState| {
                show_drawer(
                    &drawer_ctrl,
                    || {
                        drawer(
                            DrawerSide::default(),
                            vec![
                                drawer_header(vec![
                                    drawer_title("Bottom drawer"),
                                    drawer_description(
                                        "Pinned to the bottom edge, with a drag handle.",
                                    ),
                                ]),
                                drawer_footer(vec![any(text(
                                    "Drag the handle or tap it to dismiss.",
                                ))]),
                            ],
                        )
                    },
                    record("drawer"),
                );
            })),
            gap(),
            any(button("Open Command Palette", move |_: &mut AppState| {
                let select_ctrl = command_ctrl.clone();
                show_command_dialog(
                    &command_ctrl,
                    move || {
                        let select_ctrl = select_ctrl.clone();
                        command_dialog(command(
                            vec![
                                command_item("Calendar").group("Suggestions"),
                                command_item("Search Emoji").group("Suggestions"),
                                command_item("Profile").group("Settings").shortcut("⌘P"),
                                command_item("Billing")
                                    .group("Settings")
                                    .shortcut("⌘B")
                                    .disabled(true),
                            ],
                            "",
                            |_: &mut AppState, _query: String| {},
                            move |s: &mut AppState, index: usize| {
                                s.overlays.last_result = format!("command: selected index {index}");
                                select_ctrl.pop();
                            },
                        ))
                    },
                    record("command_dialog"),
                );
            })),
        ])))
    })
}
