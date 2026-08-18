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
//! Every dismissal here **animates out**: the modal host stages the exit,
//! reversing whatever entrance it played (a fade-zoom for the centered panels,
//! the edge slide for a sheet or drawer), and only pops the navigator page once
//! the ramp settles. Nothing on this page wires that up — `show_*` installs the
//! close hook itself.
//!
//! The navigator is this page's outermost view and the trigger list scrolls
//! inside its root page: a pushed modal page inherits the navigator's own
//! constraints, so those have to be the page slot's bounded ones.

use frust::{
    AnyView, Column, EdgeInsets, NavigatorController, Padding, PopResult, SizedBox, View, any,
    navigator, scroll_view, text,
};
use frust_shadcn::{
    ButtonVariant, DrawerSide, SheetSide, alert_dialog, alert_dialog_action, alert_dialog_cancel,
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

fn gap() -> AnyView<AppState> {
    any(SizedBox(None, Some(12.0)))
}

fn hgap() -> AnyView<AppState> {
    any(SizedBox(Some(8.0), None))
}

/// A row of trigger buttons, each separated by a fixed gap.
fn trigger_row(buttons: Vec<AnyView<AppState>>) -> AnyView<AppState> {
    let mut children = Vec::with_capacity(buttons.len() * 2);
    for (index, child) in buttons.into_iter().enumerate() {
        if index > 0 {
            children.push(hgap());
        }
        children.push(child);
    }
    any(frust::Row(children))
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

/// A trigger that pushes a sheet pinned to `side`.
fn sheet_trigger(
    label: &'static str,
    controller: &NavigatorController<AppState>,
    side: SheetSide,
) -> AnyView<AppState> {
    let controller = controller.clone();
    any(button(label, move |_: &mut AppState| {
        show_sheet(
            &controller,
            move || {
                sheet(vec![
                    sheet_header(vec![
                        sheet_title(format!("{label} sheet")),
                        sheet_description(
                            "Dismiss it and watch the panel slide back out the \
                                 edge it came from before the page pops.",
                        ),
                    ]),
                    sheet_footer(vec![any(text("Escape, the scrim, or the X."))]),
                ])
                .side(side)
            },
            record("sheet"),
        );
    })
    .variant(ButtonVariant::Outline))
}

/// A trigger that pushes a drawer pinned to `side`.
fn drawer_trigger(
    label: &'static str,
    controller: &NavigatorController<AppState>,
    side: DrawerSide,
) -> AnyView<AppState> {
    let controller = controller.clone();
    any(button(label, move |_: &mut AppState| {
        show_drawer(
            &controller,
            move || {
                drawer(
                    side,
                    vec![
                        drawer_header(vec![
                            drawer_title(format!("{label} drawer")),
                            drawer_description(
                                "Drag the panel toward its edge: past the halfway \
                                     point (or with a flick) it keeps going and closes; \
                                     short of it, it springs back open.",
                            ),
                        ]),
                        drawer_footer(vec![any(text(
                            "Only a bottom drawer draws the handle \u{2014} every \
                                 side drags.",
                        ))]),
                    ],
                )
            },
            record("drawer"),
        );
    })
    .variant(ButtonVariant::Outline))
}

pub fn page(state: &mut State) -> impl View<AppState> + use<> {
    let controller = state.controller.clone();
    let last_result = state.last_result.clone();

    let root_controller = controller.clone();
    navigator(&controller, move || {
        let dialog_ctrl = root_controller.clone();
        let alert_ctrl = root_controller.clone();
        let command_ctrl = root_controller.clone();
        let scroll_sheet_ctrl = root_controller.clone();
        let bare_sheet_ctrl = root_controller.clone();
        let snap_ctrl = root_controller.clone();

        // The scroll slot is *inside* the navigator's root page, never around
        // the navigator: a pushed modal page is laid out under the navigator's
        // own constraints, and a scroll view's infinite max height would
        // collapse every panel it hosts to nothing.
        crate::scroll_slot(any(Column(vec![
            any(crate::nav::heading("Overlays")),
            any(SizedBox(None, Some(16.0))),
            any(text(format!("Last result: {last_result}")).size(14.0)),
            any(SizedBox(None, Some(8.0))),
            any(crate::nav::caption(
                "Watch every dismissal, not just every opening: the panel plays its \
                 entrance in reverse (fade + zoom for the centered ones, the edge \
                 slide for sheets and drawers) and the scrim fades down with it \
                 before the navigator page pops. Under reduced motion it closes \
                 outright instead.",
            )),
            gap(),
            // --- The centered family ---
            any(text("Dialogs").size(16.0)),
            any(SizedBox(None, Some(8.0))),
            trigger_row(vec![
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
                                dialog_footer(vec![any(button(
                                    "Cancel",
                                    move |_: &mut AppState| cancel_ctrl.pop(),
                                )
                                .variant(ButtonVariant::Outline))]),
                            ])
                        },
                        record("dialog"),
                    );
                })),
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
                                    command_item("Profile")
                                        .group("Settings")
                                        .shortcut("\u{2318}P"),
                                    command_item("Billing")
                                        .group("Settings")
                                        .shortcut("\u{2318}B")
                                        .disabled(true),
                                ],
                                "",
                                |_: &mut AppState, _query: String| {},
                                move |s: &mut AppState, index: usize| {
                                    s.overlays.last_result =
                                        format!("command: selected index {index}");
                                    select_ctrl.pop();
                                },
                            ))
                        },
                        record("command_dialog"),
                    );
                })),
            ]),
            gap(),
            any(frust_shadcn::separator()),
            gap(),
            // --- Sheets: four sides plus two content variants ---
            any(text("Sheets").size(16.0)),
            any(SizedBox(None, Some(8.0))),
            any(crate::nav::caption(
                "A sheet is dialog-family, so it does NOT drag: it is dismissed by \
                 the scrim, Escape or its close X, and only then slides out.",
            )),
            any(SizedBox(None, Some(8.0))),
            trigger_row(vec![
                sheet_trigger("Right", &root_controller, SheetSide::Right),
                sheet_trigger("Left", &root_controller, SheetSide::Left),
                sheet_trigger("Top", &root_controller, SheetSide::Top),
                sheet_trigger("Bottom", &root_controller, SheetSide::Bottom),
            ]),
            any(SizedBox(None, Some(8.0))),
            trigger_row(vec![
                any(button(
                    "Sheet with scrolling content",
                    move |_: &mut AppState| {
                        show_sheet(
                            &scroll_sheet_ctrl,
                            || {
                                sheet(vec![
                                    sheet_header(vec![
                                        sheet_title("Long content"),
                                        sheet_description(
                                            "The panel keeps its size; the body scrolls.",
                                        ),
                                    ]),
                                    any(Padding(
                                        EdgeInsets::all(16.0),
                                        frust::SizedBox::<AppState>(None, Some(320.0)).child(
                                            scroll_view(Column(
                                                (1..=40)
                                                    .map(|i| {
                                                        any::<AppState, _>(
                                                            text(format!("Line {i}")).size(13.0),
                                                        )
                                                    })
                                                    .collect(),
                                            )),
                                        ),
                                    )),
                                ])
                            },
                            record("sheet_scroll"),
                        );
                    },
                )),
                any(button(
                    "Sheet without a close button",
                    move |_: &mut AppState| {
                        show_sheet(
                            &bare_sheet_ctrl,
                            || {
                                sheet(vec![
                                    sheet_header(vec![
                                        sheet_title("No X in the corner"),
                                        sheet_description(
                                            "close_button(false) \u{2014} the scrim and Escape \
                                         are the only ways out.",
                                        ),
                                    ]),
                                    sheet_footer(vec![any(text("Press Escape."))]),
                                ])
                                .close_button(false)
                            },
                            record("sheet_bare"),
                        );
                    },
                )),
            ]),
            gap(),
            any(frust_shadcn::separator()),
            gap(),
            // --- Drawers: four directions, drag-to-close, snap points ---
            any(text("Drawers").size(16.0)),
            any(SizedBox(None, Some(8.0))),
            any(crate::nav::caption(
                "A drawer drags. Press the panel (the handle on a bottom drawer, or \
                 anywhere the content does not take the press) and pull it toward its \
                 own edge: the panel follows the pointer and the scrim fades with it. \
                 Release past the halfway point, or flick it, and the exit ramp \
                 continues from wherever the drag left it; release short of it and it \
                 springs back. Catching a closing drawer cancels its dismissal.",
            )),
            any(SizedBox(None, Some(8.0))),
            trigger_row(vec![
                drawer_trigger("Bottom", &root_controller, DrawerSide::Bottom),
                drawer_trigger("Top", &root_controller, DrawerSide::Top),
                drawer_trigger("Left", &root_controller, DrawerSide::Left),
                drawer_trigger("Right", &root_controller, DrawerSide::Right),
            ]),
            any(SizedBox(None, Some(8.0))),
            trigger_row(vec![any(button(
                "Drawer with snap points (40% / 100%)",
                move |_: &mut AppState| {
                    show_drawer(
                        &snap_ctrl,
                        || {
                            drawer(
                                DrawerSide::Bottom,
                                vec![
                                    drawer_header(vec![
                                        drawer_title("Snap points"),
                                        drawer_description(
                                            "It opens at the first point (40% of its \
                                             extent). Drag up to fill it, drag down to \
                                             come back \u{2014} a release snaps to \
                                             whichever resting point is nearest, and \
                                             closed counts as one of them.",
                                        ),
                                    ]),
                                    drawer_footer(vec![any(text(
                                        "The scrim rides the open fraction, so a \
                                         part-open drawer sits under a lighter scrim.",
                                    ))]),
                                ],
                            )
                            .snap_points(&[0.4, 1.0])
                        },
                        record("drawer_snap"),
                    );
                },
            ))]),
            any(SizedBox(None, Some(24.0))),
        ])))
    })
}
