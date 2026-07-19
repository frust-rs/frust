//! User profile card (`/user/:id`) — the shared-element hero destination an
//! avatar tap anywhere in the app (Home/Search/Activity — Phase C tasks of
//! their own) navigates to.
//!
//! The avatar wraps [`hero`] under the `avatar-{user_id}` tag — the exact
//! convention those screens' own small avatars must also use for the morph to
//! activate (see `crate::nav`'s hero contract — a push/pop transition where
//! both pages carry the same tag). Fields come from
//! [`ProfileController::load`] (a synchronous mock lookup — see its module
//! docs); an unknown id (a bad `/user/:id` deep link/param) falls back to a
//! plain not-found scaffold rather than panicking.
//!
//! Message pushes into the existing DM channel feed
//! (`crate::features::messages::presentation::pages::channel_feed`, routed through `/channel/:id`) when one
//! exists in the mock dataset; Huddle call is deliberately inert (no huddle
//! feature exists yet) but still shows `Button`'s native pressed feedback.
//! Both act on `state.nav` directly — `HuddleState`'s own router handle —
//! since this route (`src/routes.rs`, a frozen hub file) calls this function
//! with no `NavigatorController` argument, unlike `channel_feed`/`settings`.

use frust::{
    Align, Alignment, AnyView, Button, Column, CrossAxisAlignment, Row, SizedBox, Theme, any,
    filled_card, hero, text, use_context,
};

use crate::HuddleState;
use crate::features::profile::ProfileController;
use crate::mock;
use crate::screens::{placeholder_body, scaffold};

/// Overall avatar tile side length (the [`filled_card`] wrapper adds its own
/// padding on top — see [`initials_tile`]).
const AVATAR_TILE: f64 = 64.0;
/// Avatar initials font size — large, matching [`AVATAR_TILE`].
const AVATAR_FONT: f32 = 28.0;

/// The profile card for `user_id` (the `/user/:id` route param — a string,
/// parsed against the mock dataset's numeric ids).
pub fn profile_screen(user_id: String) -> AnyView<HuddleState> {
    let Some(id) = user_id.parse::<u32>().ok() else {
        return unknown_user_screen(&user_id);
    };
    let Some(profile) = ProfileController::load(id) else {
        return unknown_user_screen(&user_id);
    };

    // Status-dot color from the theme's semantic roles (never a hardcoded
    // literal — see `docs/CODE_STANDARDS.md`'s Theming conventions): tertiary
    // for online, secondary for away, error for do-not-disturb.
    let theme = use_context::<Theme>().unwrap_or_else(Theme::m3_baseline);
    let scheme = theme.scheme();
    let status_color = match profile.user.status {
        mock::UserStatus::Online => scheme.tertiary,
        mock::UserStatus::Away => scheme.secondary,
        mock::UserStatus::Dnd => scheme.error,
    };

    let dm_channel_id = profile.dm_channel_id.clone();

    let avatar = any(hero(
        format!("avatar-{id}"),
        initials_tile::<HuddleState>(profile.user.initials, AVATAR_TILE, AVATAR_FONT),
    ));

    let status_row = any(Row(vec![
        any(text("\u{25CF}").color(status_color).size(14.0)),
        any(SizedBox(Some(6.0), None)),
        any(text(profile.user.status.label()).size(14.0)),
    ]));

    let actions = any(Row(vec![
        any(Button("Message", move |s: &mut HuddleState| {
            // Inert when there is no existing DM with this user (see the
            // module docs) — the button still shows its native pressed
            // feedback, just fires nothing.
            if let Some(dm_id) = &dm_channel_id {
                s.nav.router().push(&format!("/channel/{dm_id}"));
            }
        })),
        any(SizedBox(Some(12.0), None)),
        // Deliberately inert (no huddle-call feature exists yet) — see the
        // module docs. `Button` already paints its own pressed state on
        // `Down`/`Move`, so this needs no extra wiring to satisfy "inert with
        // pressed state".
        any(Button("Huddle call", |_s: &mut HuddleState| {})),
    ]));

    let body = any(Column(vec![
        avatar,
        any(SizedBox(None, Some(16.0))),
        any(text(profile.user.name).size(24.0)),
        any(text(profile.handle.clone()).size(14.0)),
        any(SizedBox(None, Some(8.0))),
        status_row,
        any(SizedBox(None, Some(4.0))),
        any(text(profile.local_time.clone()).size(13.0)),
        any(text(profile.role_team.clone()).size(13.0)),
        any(SizedBox(None, Some(16.0))),
        actions,
    ])
    .cross_axis(CrossAxisAlignment::Center));

    scaffold(profile.user.name, body)
}

/// A rounded, theme-colored initials tile — the avatar/workspace-tile visual
/// this screen and `crate::screens::workspace_drawer` both use.
///
/// There is no perfect-circle or per-corner-radius primitive in the public
/// `frust` widget vocabulary (only [`filled_card`]'s uniform corner
/// radius), so this is a uniformly-rounded square tile rather than a true
/// circular avatar — a documented simplification, not an oversight.
fn initials_tile<State: 'static>(initials: &str, tile_size: f64, font_size: f32) -> AnyView<State> {
    any(filled_card(
        SizedBox(Some(tile_size), Some(tile_size))
            .child(Align(Alignment::CENTER, text(initials).size(font_size))),
    ))
}

/// Fallback for an id that doesn't resolve against the mock dataset (an
/// invalid `/user/:id` deep link/param) — renders instead of panicking.
fn unknown_user_screen(user_id: &str) -> AnyView<HuddleState> {
    scaffold(
        "Profile",
        placeholder_body(&format!("No such user: {user_id}")),
    )
}
