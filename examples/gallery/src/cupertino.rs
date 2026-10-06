//! `Cupertino`-design cases — the iOS design-system plugin widgets
//! (see [`crate::case::Design::Cupertino`]).
//!
//! Each case is a pure [`frust_core::View`] constructor that renders under the
//! Cupertino theme. See the crate docs and `crate::base` for the pure-`View`/slug-rule
//! contract every case in this registry follows.

use frust_core::AnyView;
use frust_cupertino::{
    CupertinoActionStyle, CupertinoAlertDialogView, CupertinoButtonSize, CupertinoButtonStyle,
    action, cupertino_activity_indicator, cupertino_button, cupertino_nav_bar, cupertino_switch,
    cupertino_tab_bar, tab_item,
};
use frust_widgets::{CrossAxisAlignment, NavigatorController, SizedBox, column, row, text};

use super::base::framed;
use crate::case::{Case, Design};

fn button_case() -> AnyView<()> {
    framed(
        column()
            .child(
                cupertino_button("Small Filled", |_: &mut ()| {})
                    .size(CupertinoButtonSize::Small)
                    .style(CupertinoButtonStyle::Filled),
            )
            .child(SizedBox(None, Some(8.0)))
            .child(
                cupertino_button("Medium Gray", |_: &mut ()| {})
                    .size(CupertinoButtonSize::Medium)
                    .style(CupertinoButtonStyle::Gray),
            )
            .child(SizedBox(None, Some(8.0)))
            .child(
                cupertino_button("Large Glass", |_: &mut ()| {})
                    .size(CupertinoButtonSize::Large)
                    .style(CupertinoButtonStyle::Glass),
            )
            .cross_axis(CrossAxisAlignment::Center),
    )
}

fn activity_indicator_case() -> AnyView<()> {
    framed(cupertino_activity_indicator().animating(false))
}

fn navbar_case() -> AnyView<()> {
    framed(
        column()
            .child(
                cupertino_nav_bar("Settings")
                    .leading(SizedBox(Some(30.0), Some(30.0)))
                    .trailing(SizedBox(Some(30.0), Some(30.0))),
            )
            .child(SizedBox(None, Some(16.0)))
            .child(text("Navbar with leading and trailing content"))
            .cross_axis(CrossAxisAlignment::Center),
    )
}

fn switch_case() -> AnyView<()> {
    framed(
        column()
            .child(
                row()
                    .child(text("Off"))
                    .child(SizedBox(Some(16.0), None))
                    .child(cupertino_switch(false, |_: &mut (), _: bool| {}))
                    .cross_axis(CrossAxisAlignment::Center),
            )
            .child(SizedBox(None, Some(12.0)))
            .child(
                row()
                    .child(text("On"))
                    .child(SizedBox(Some(16.0), None))
                    .child(cupertino_switch(true, |_: &mut (), _: bool| {}))
                    .cross_axis(CrossAxisAlignment::Center),
            )
            .cross_axis(CrossAxisAlignment::Center),
    )
}

fn tabbar_case() -> AnyView<()> {
    framed(
        column()
            .child(text("Tab bar (selected: 0)"))
            .child(SizedBox(None, Some(12.0)))
            .child(cupertino_tab_bar::<(), _>(
                vec![tab_item("Home"), tab_item("Search"), tab_item("Favorites")],
                0,
                |_: &mut (), _: usize| {},
            ))
            .cross_axis(CrossAxisAlignment::Center),
    )
}

fn alert_dialog_case() -> AnyView<()> {
    let controller: NavigatorController<()> = NavigatorController::new();
    framed(CupertinoAlertDialogView {
        title: "Confirm".into(),
        message: Some("Are you sure?".into()),
        actions: vec![
            action("Cancel"),
            action("OK").style(CupertinoActionStyle::Default),
        ],
        controller,
    })
}

fn action_sheet_case() -> AnyView<()> {
    use frust_cupertino::CupertinoActionSheetView;
    let controller: NavigatorController<()> = NavigatorController::new();
    framed(CupertinoActionSheetView {
        actions: vec![
            action("Save"),
            action("Delete").style(CupertinoActionStyle::Destructive),
        ],
        cancel: Some("Cancel".into()),
        controller,
    })
}

/// This module's slice of the registry [`crate::cases`] concatenates.
pub const CASES: &[Case] = &[
    Case {
        slug: "cupertino/button",
        title: "Cupertino Button",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Cupertino,
        build: button_case,
    },
    Case {
        slug: "cupertino/activity-indicator",
        title: "Cupertino Activity Indicator",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Cupertino,
        build: activity_indicator_case,
    },
    Case {
        slug: "cupertino/alert-dialog",
        title: "Cupertino Alert Dialog",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Cupertino,
        build: alert_dialog_case,
    },
    Case {
        slug: "cupertino/action-sheet",
        title: "Cupertino Action Sheet",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Cupertino,
        build: action_sheet_case,
    },
    Case {
        slug: "cupertino/navbar",
        title: "Cupertino Nav Bar",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Cupertino,
        build: navbar_case,
    },
    Case {
        slug: "cupertino/switch",
        title: "Cupertino Switch",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Cupertino,
        build: switch_case,
    },
    Case {
        slug: "cupertino/tabbar",
        title: "Cupertino Tab Bar",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Cupertino,
        build: tabbar_case,
    },
];
