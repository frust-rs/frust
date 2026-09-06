//! Settings menu (`/you/settings`) — the list-stack root into the nested
//! settings pages.
//!
//! Three [`list_item`] rows (Notifications / Appearance / About), each with a
//! leading glyph and a trailing [`icons::CHEVRON_RIGHT`], pushing its nested page
//! through the shared [`NavigatorController`].

use frust::{
    AnyView, Axis, Column, CrossAxisAlignment, EdgeInsets, FlexView, NavigatorController, Padding,
    any, flexible, icon, icons, inflexible, scroll_view,
};
use frust_material::{app_bar, list_item};

use crate::HuddleState;

use super::{settings_about, settings_appearance, settings_notifications};

/// The settings menu. `controller` pushes each nested page.
pub fn settings_screen(controller: NavigatorController<HuddleState>) -> AnyView<HuddleState> {
    let c_notif = controller.clone();
    let c_appear = controller.clone();
    let c_about = controller;

    let rows = Column(vec![
        any(list_item("Notifications")
            .supporting("Frequency and alert types")
            .leading(icon(icons::NOTIFICATIONS))
            .trailing(icon(icons::CHEVRON_RIGHT))
            .on_press(move |_s: &mut HuddleState| {
                c_notif.push(settings_notifications::notifications_screen);
            })),
        any(list_item("Appearance")
            .supporting("Theme, accent, and text size")
            .leading(icon(icons::PALETTE))
            .trailing(icon(icons::CHEVRON_RIGHT))
            .on_press(move |_s: &mut HuddleState| {
                c_appear.push(settings_appearance::appearance_screen);
            })),
        any(list_item("About")
            .supporting("Version, credits, licenses")
            .leading(icon(icons::DESCRIPTION))
            .trailing(icon(icons::CHEVRON_RIGHT))
            .on_press(move |_s: &mut HuddleState| {
                c_about.push(settings_about::about_screen);
            })),
    ]);

    let body = any(scroll_view(Padding(EdgeInsets::all(8.0), rows)));

    any(FlexView::new(
        Axis::Vertical,
        vec![
            inflexible(any(app_bar::<HuddleState>("Settings"))),
            flexible(1, body),
        ],
    )
    .cross_axis(CrossAxisAlignment::Stretch))
}
