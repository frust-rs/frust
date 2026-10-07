//! The gallery's screens, plus the chrome they share.
//!
//! Two of them live *inside* the shell (a section host and its list pane, one
//! per navigation-bar destination); two cover it as pushed routes (a
//! playground and the theme settings screen). [`playground`] holds one file
//! per catalog entry — the bodies those hosts show.

pub mod playground;
mod playground_scaffold;
mod section_host;
mod section_list;
mod theme_config_page;

pub use playground_scaffold::playground_scaffold;
pub use section_host::{SectionSelection, section_host};
pub use section_list::section_list;
pub use theme_config_page::theme_config_page;

use frust::{
    AnyView, Brightness, EdgeInsets, Padding, RouteParams, Theme, View, any, icon, text,
    use_context,
};
use frust_material::{icon_button, icons};

use crate::AppState;
use crate::catalog;

/// The theme settings route, pushed over the shell from the app bar.
pub const THEME_ROUTE: &str = "/theme";

/// The app bar action that flips light/dark.
///
/// `brightness` is what is showing right now (read from the ambient
/// [`Theme`] by the caller) — the reference's `fallback`, which decides both
/// the icon and which way the toggle goes.
pub fn brightness_action(brightness: Brightness) -> impl View<AppState> {
    let glyph = match brightness {
        Brightness::Dark => icons::LIGHT_MODE,
        Brightness::Light => icons::DARK_MODE,
    };
    icon_button(icon(glyph), move |state: &mut AppState| {
        state.settings.toggle_brightness(brightness)
    })
    .semantic_label("Toggle theme")
}

/// The `/playground/:id` route: one entry's playground, under its own chrome.
///
/// An unknown id renders a plain message rather than an empty screen — it is
/// reachable from a hand-typed deep link, not just from a list row.
pub fn playground_route(params: &RouteParams) -> AnyView<AppState> {
    any(
        match params.get("id").and_then(|id| catalog::find_by_id(id)) {
            Some(entry) => playground_scaffold(entry.title, (entry.build)(entry)),
            None => playground_scaffold("Not found", any(unknown_entry())),
        },
    )
}

/// The body a `/playground/:id` with no matching catalog entry shows.
fn unknown_entry() -> impl View<AppState> {
    let theme = use_context::<Theme>().unwrap_or_else(frust_material::baseline);
    let mut style = theme.type_scale.body_large.clone();
    style.color = theme.scheme().on_surface_variant;
    Padding(
        EdgeInsets::all(16.0),
        text("No catalog entry has that id.").style(style),
    )
}

#[cfg(test)]
mod tests {
    use frust::RouteParams;

    use super::{playground_route, theme_config_page};

    #[test]
    fn a_known_id_and_an_unknown_one_both_build_a_page() {
        let mut params = RouteParams::new();
        params.insert("id".to_string(), "buttons".to_string());
        let _known = playground_route(&params);

        params.insert("id".to_string(), "no-such-entry".to_string());
        let _unknown = playground_route(&params);

        // No id at all: a `/playground/` with the capture missing.
        let _empty = playground_route(&RouteParams::new());
    }

    #[test]
    fn the_theme_screen_builds_without_a_settings_scope() {
        let _view = theme_config_page();
    }
}
