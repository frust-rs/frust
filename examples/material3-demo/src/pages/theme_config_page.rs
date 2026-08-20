//! The theme settings route: seed, brightness, font, and type style.
//!
//! Read-only for now — [`ThemeSettings`] already carries every knob and
//! applies it, so what is missing here is the pickers, not the model. Each row
//! shows what is selected and what else is on offer; the app bar's brightness
//! action is already live.

use frust::{AnyView, Column, EdgeInsets, Padding, SizedBox, Theme, any, text, use_context};
use frust_material::outlined_card;

use crate::AppState;
use crate::pages::playground_scaffold;
use crate::theme::{DemoFont, DemoTypeStyle, SEED_OPTIONS, ThemeSettings};

/// The theme settings screen, pushed over the gallery shell.
pub fn theme_config_page() -> AnyView<AppState> {
    let settings = use_context::<ThemeSettings>();
    let rows = match settings {
        Some(settings) => vec![
            summary(
                "Seed",
                settings.seed_label(),
                options(SEED_OPTIONS.iter().map(|(_, label)| *label)),
            ),
            summary(
                "Brightness",
                brightness_summary(&settings),
                "toggled from the app bar",
            ),
            summary(
                "Font",
                settings.font().label(),
                options(DemoFont::ALL.iter().map(|font| font.label())),
            ),
            summary(
                "Type style",
                settings.type_style().label(),
                options(DemoTypeStyle::ALL.iter().map(|style| style.label())),
            ),
        ],
        // No scope above this page — only reachable in a bare harness, never
        // in the running app.
        None => Vec::new(),
    };

    playground_scaffold("Theme settings", body(rows))
}

/// One settings row: what is selected, and what else this knob offers.
struct Row {
    label: &'static str,
    value: String,
    detail: String,
}

/// Build a row.
fn summary(label: &'static str, value: impl Into<String>, detail: impl Into<String>) -> Row {
    Row {
        label,
        value: value.into(),
        detail: detail.into(),
    }
}

/// A knob's choices, as one middle-dot-separated line.
fn options<'a>(labels: impl Iterator<Item = &'a str>) -> String {
    labels.collect::<Vec<_>>().join(" \u{00B7} ")
}

/// How brightness is currently being resolved, in words.
fn brightness_summary(settings: &ThemeSettings) -> &'static str {
    if settings.auto_theming() {
        if settings.invert_platform() {
            "follows the platform, inverted"
        } else {
            "follows the platform"
        }
    } else {
        match settings.brightness_override() {
            Some(frust::Brightness::Dark) => "pinned dark",
            _ => "pinned light",
        }
    }
}

/// The card the rows sit in.
fn body(rows: Vec<Row>) -> AnyView<AppState> {
    let theme = use_context::<Theme>().unwrap_or_else(frust_material::baseline);
    let scheme = theme.scheme();
    let mut label_style = theme.type_scale.title_medium.clone();
    label_style.color = scheme.on_surface;
    let mut value_style = theme.type_scale.body_large.clone();
    value_style.color = scheme.on_surface;
    let mut detail_style = theme.type_scale.body_medium.clone();
    detail_style.color = scheme.on_surface_variant;

    let mut children: Vec<AnyView<AppState>> = Vec::new();
    for row in rows {
        if !children.is_empty() {
            children.push(any(SizedBox::<AppState>(None, Some(16.0))));
        }
        children.push(any(text(row.label).style(label_style.clone())));
        children.push(any(text(row.value).style(value_style.clone())));
        children.push(any(text(row.detail).style(detail_style.clone())));
    }
    children.push(any(SizedBox::<AppState>(None, Some(16.0))));
    children.push(any(text(
        "Pickers for these land with the theme settings screen.",
    )
    .style(detail_style)));

    any(Padding(
        EdgeInsets::all(16.0),
        outlined_card(Padding(EdgeInsets::all(16.0), Column(children))),
    ))
}
