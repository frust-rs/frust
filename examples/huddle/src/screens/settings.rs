//! Settings menu (`/you/settings`) — the entry into the nested settings pages.
//!
//! **Placeholder-ish** (Phase C task: Settings menu — grouped `ListView` rows).
//! Today it renders a small menu that pushes each nested settings page so the
//! `/you/settings/*` stack is clickable end-to-end.

use forgekit::{AnyView, Button, Column, NavigatorController, SizedBox, any};

use crate::HuddleState;
use crate::screens::{scaffold, settings_about, settings_appearance, settings_notifications};

/// The settings menu. `controller` lets each row push its nested page.
pub fn settings_screen(controller: NavigatorController<HuddleState>) -> AnyView<HuddleState> {
    let c_notif = controller.clone();
    let c_appear = controller.clone();
    let c_about = controller.clone();

    let body = any(Column(vec![
        any(Button("Notifications", move |_s: &mut HuddleState| {
            c_notif.push(settings_notifications::notifications_screen);
        })),
        any(SizedBox(None, Some(8.0))),
        any(Button("Appearance", move |_s: &mut HuddleState| {
            c_appear.push(settings_appearance::appearance_screen);
        })),
        any(SizedBox(None, Some(8.0))),
        any(Button("About", move |_s: &mut HuddleState| {
            c_about.push(settings_about::about_screen);
        })),
    ]));
    scaffold("Settings", body)
}
