//! The chrome a pushed playground route wears: a top app bar over the
//! playground body.
//!
//! Used by the narrow layout's `/playground/:id` route and by the theme
//! settings route — the two pages that cover the gallery shell rather than
//! living inside it. The split layout has chrome of its own (the shell's app
//! bar stays put), so it renders the playground body bare.

use frust::{AnyView, Theme, any, icon, scaffold, use_context};
use frust_material::{app_bar, icon_button, icons};

use crate::AppState;
use crate::pages::brightness_action;

/// `body` under a titled app bar with a back button and the brightness
/// toggle — the reference's `PlaygroundScaffold`.
pub fn playground_scaffold(title: &str, body: AnyView<AppState>) -> AnyView<AppState> {
    let theme = use_context::<Theme>().unwrap_or_else(frust_material::baseline);
    let bar = app_bar::<AppState>(title)
        .leading(
            icon_button(icon(icons::ARROW_BACK), |state: &mut AppState| {
                state.router.pop()
            })
            .semantic_label("Back"),
        )
        .actions(vec![brightness_action(theme.brightness)]);

    // The bar self-insets its top and side edges; no `safe_area` wrapper.
    any(scaffold(body)
        .app_bar(bar)
        .background(theme.scheme().surface))
}
