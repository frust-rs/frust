//! You tab (`/you`) — the current user's card and the entry into the settings
//! stack.
//!
//! The header is the current user's card: a hero-wrapped avatar (tag
//! `avatar-u1`, the shared-element source for the `/user/:id` profile page), the
//! display name, `@handle`, and presence. Below it, a settings entry row plus two
//! inert-but-styled account rows (Saved items, Preferences) — each a
//! [`list_item`] with a leading glyph and a trailing [`icons::CHEVRON_RIGHT`].

use std::sync::Arc;

use frust::{
    Align, Alignment, AnyView, Color, CrossAxisAlignment, EdgeInsets, Image, ImageFit,
    NavigatorController, Padding, SizedBox, Theme, any, column, hero, icon, icons, scroll_view,
    stack, text, use_context,
};
use frust_material::{app_bar, list_item};

use crate::HuddleState;
use crate::features::profile::domain::CURRENT_USER_ID;
use crate::features::profile::domain::repositories::ProfileRepository;
use crate::features::settings::presentation::pages::settings::settings_screen;
use crate::ui::solid_source::solid_source;

use super::profile;

/// The You tab root. `controller` pushes the settings stack and the profile page.
pub fn you_screen(controller: NavigatorController<HuddleState>) -> AnyView<HuddleState> {
    let theme = use_context::<Theme>().unwrap_or_else(frust_material::baseline);
    let scheme = theme.scheme();

    let repo = use_context::<Arc<dyn ProfileRepository + Send + Sync>>()
        .expect("ProfileRepository is provided by lib.rs's composition root");
    let me = repo.user(CURRENT_USER_ID);
    let (name, initials, handle, status) = match me {
        Some(u) => (
            u.name.to_string(),
            u.initials.to_string(),
            handle_for(u.name),
            u.status.label().to_string(),
        ),
        None => (
            "You".to_string(),
            "?".to_string(),
            "@you".to_string(),
            "Online".to_string(),
        ),
    };

    // The hero-wrapped avatar block (tag `avatar-u1` → the profile page's hero).
    let avatar = any(hero(
        "avatar-u1",
        avatar_block(initials, scheme.primary, scheme.on_primary),
    ));

    let profile_controller = controller.clone();
    let user_id = me.map(|u| u.id).unwrap_or(CURRENT_USER_ID);
    let user_card = list_item(name)
        .supporting(format!("{handle} · {status}"))
        .leading(avatar)
        .trailing(icon(icons::CHEVRON_RIGHT))
        .on_press(move |_s: &mut HuddleState| {
            let id = user_id.to_string();
            profile_controller.push(move || profile::profile_screen(id.clone()));
        });

    let settings_controller = controller.clone();
    let settings_row = list_item("Settings")
        .supporting("Notifications, appearance, about")
        .leading(icon(icons::SETTINGS))
        .trailing(icon(icons::CHEVRON_RIGHT))
        .on_press(move |_s: &mut HuddleState| {
            let c = settings_controller.clone();
            settings_controller.push(move || settings_screen(c.clone()));
        });

    // Inert-but-styled account rows (no `on_press`): the showcase renders the
    // affordance without a destination.
    let saved_row = list_item("Saved items")
        .supporting("Bookmarked messages")
        .leading(icon(icons::STAR))
        .trailing(icon(icons::CHEVRON_RIGHT));
    let prefs_row = list_item("Preferences")
        .supporting("Language, accessibility")
        .leading(icon(icons::PALETTE))
        .trailing(icon(icons::CHEVRON_RIGHT));

    let body = any(scroll_view(Padding(
        EdgeInsets::all(12.0),
        column()
            .child(user_card)
            .child(SizedBox(None, Some(16.0)))
            .child(settings_row)
            .child(saved_row)
            .child(prefs_row),
    )));

    any(column()
        .child(app_bar::<HuddleState>("You"))
        .flex(1, body)
        .cross_axis(CrossAxisAlignment::Stretch))
}

/// A square avatar block: a solid `primary`-filled 56×56 tile with the user's
/// initials centered in `on_primary`.
fn avatar_block(initials: String, fill: Color, on_fill: Color) -> AnyView<HuddleState> {
    let tile =
        any(SizedBox(Some(56.0), Some(56.0)).child(Image(solid_source(fill)).fit(ImageFit::Fill)));
    // Center the monogram with the SizedBox+Align idiom (a bare `Align` under a
    // `Stack` shrink-wraps to the origin;
    // `profile::presentation::pages::profile`'s `initials_tile` is the
    // precedent).
    let label = any(SizedBox(Some(56.0), Some(56.0)).child(Align(
        Alignment::CENTER,
        text(initials).size(20.0).color(on_fill),
    )));
    any(stack().child(tile).child(label))
}

/// Derive an `@handle` from a display name — the first name, lowercased.
fn handle_for(name: &str) -> String {
    let first = name.split_whitespace().next().unwrap_or(name);
    format!("@{}", first.to_lowercase())
}
