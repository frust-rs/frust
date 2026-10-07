//! About page (`/you/settings/about`) — app icon, version, and a few card rows.
//!
//! The app icon is painted from `assets/logo.png` via [`Image`] when the bytes
//! decode, degrading to an initials block (the same solid-tile escape hatch the
//! You tab's avatar uses) otherwise. Below it: the app name + version, a
//! [`filled_card`] Licenses row (inert), and an [`outlined_card`] "Frust"
//! link-style row that raises a toast (no real browser — a showcase non-goal).

use frust::{
    Align, Alignment, AnyView, Color, CrossAxisAlignment, EdgeInsets, Image, ImageFit, ImageSource,
    Padding, SizedBox, Theme, View, any, column, scroll_view, stack, text, use_context,
};
use frust_material::{app_bar, filled_card, outlined_card};

use crate::HuddleState;
use crate::ui::solid_source::solid_source;

/// The bundled app-icon bytes (the anvil logo), decoded once per build.
const LOGO_PNG: &[u8] = include_bytes!("../../../../../assets/logo.png");

/// The about page.
pub fn about_screen() -> AnyView<HuddleState> {
    let theme = use_context::<Theme>().unwrap_or_else(frust_material::baseline);
    let scheme = theme.scheme();

    let icon_block = match ImageSource::decode(LOGO_PNG) {
        Ok(source) => {
            any(SizedBox(Some(72.0), Some(72.0)).child(Image(source).fit(ImageFit::Contain)))
        }
        // Escape hatch: a solid-primary tile with the app initial.
        Err(_) => any(initials_block("H", scheme.primary, scheme.on_primary)),
    };

    let header = any(column()
        .child(Align(Alignment::CENTER, icon_block))
        .child(SizedBox(None, Some(12.0)))
        .child(Align(Alignment::CENTER, text("Huddle").size(24.0)))
        .child(Align(
            Alignment::CENTER,
            text(format!("Version {}", env!("CARGO_PKG_VERSION"))).size(13.0),
        ))
        .child(Align(
            Alignment::CENTER,
            text("The Frust showcase app").size(13.0),
        )));

    let licenses = any(Padding(
        EdgeInsets::symmetric(0.0, 6.0),
        filled_card(Padding(
            EdgeInsets::all(16.0),
            column()
                .child(text("Open-source licenses").size(15.0))
                .child(text("Material Symbols · Parley · Vello").size(12.0)),
        )),
    ));

    // A link-style card (outlined variant) that raises a toast on tap.
    let frust_link = any(Padding(
        EdgeInsets::symmetric(0.0, 6.0),
        outlined_card(Padding(
            EdgeInsets::all(16.0),
            column()
                .child(text("Frust").size(15.0))
                .child(text("frust.dev").size(12.0)),
        ))
        .on_press(|s: &mut HuddleState| {
            s.toasts.show("Opens frust.dev");
        }),
    ));

    let body = any(scroll_view(Padding(
        EdgeInsets::all(20.0),
        column()
            .child(header)
            .child(SizedBox(None, Some(24.0)))
            .child(licenses)
            .child(frust_link),
    )));

    any(column()
        .child(app_bar::<HuddleState>("About"))
        .flex(1, body)
        .cross_axis(CrossAxisAlignment::Stretch))
}

/// A 72×72 solid-`fill` tile with a centered initial in `on_fill` — the app-icon
/// fallback when the bundled logo fails to decode.
fn initials_block(initial: &str, fill: Color, on_fill: Color) -> impl View<HuddleState> {
    let tile =
        any(SizedBox(Some(72.0), Some(72.0)).child(Image(solid_source(fill)).fit(ImageFit::Fill)));
    // Center the monogram with the SizedBox+Align idiom (a bare `Align` under a
    // `Stack` shrink-wraps to the origin;
    // `profile::presentation::pages::profile`'s `initials_tile` is the
    // precedent).
    let label = any(SizedBox(Some(72.0), Some(72.0)).child(Align(
        Alignment::CENTER,
        text(initial.to_string()).size(32.0).color(on_fill),
    )));
    stack().child(tile).child(label)
}
