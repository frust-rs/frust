//! Loading indicator: the reference's `LoadingIndicatorPlayground`.
//!
//! [`frust_material::loading_indicator`]'s two variants
//! ([`frust_material::LoadingIndicatorVariant::Default`]/`Contained`) map
//! 1:1 onto the reference's `M3ELoadingIndicatorVariant.defaultStyle`/
//! `.contained` — a straight port, no descopes.
//!
//! # `Wrap` becomes a plain `Row`
//!
//! The "Both variants" preview lays its two indicators out in a
//! non-wrapping [`spaced_row`] rather than the reference's `Wrap(spacing:
//! 24, runSpacing: 16)` — this workspace has no flow/wrap layout primitive
//! (see `crate::pages::playground::pick::chips`' module docs for the same
//! divergence).

use frust::{AnyView, Component, CrossAxisAlignment, Row, SizedBox, View, any, component};
use frust_material::{LoadingIndicatorVariant, loading_indicator};

use crate::AppState;
use crate::catalog::DemoEntry;
use crate::widgets::playground::{
    control_panel, play_enum_segmented, play_preview_card, play_snippet, playground_body,
};

/// This playground's knobs — the reference's `_LoadingIndicatorPlaygroundState`.
struct Knobs {
    variant: LoadingIndicatorVariant,
}

impl Default for Knobs {
    fn default() -> Self {
        Self {
            variant: LoadingIndicatorVariant::Default,
        }
    }
}

/// Every [`LoadingIndicatorVariant`], for the segmented control.
const VARIANTS: [LoadingIndicatorVariant; 2] = [
    LoadingIndicatorVariant::Default,
    LoadingIndicatorVariant::Contained,
];

/// [`LoadingIndicatorVariant`]'s display label — the reference's `v.name`.
fn variant_label(variant: LoadingIndicatorVariant) -> &'static str {
    match variant {
        LoadingIndicatorVariant::Default => "Default",
        LoadingIndicatorVariant::Contained => "Contained",
    }
}

/// Build the indicator `variant` selects.
fn indicator_for<State: 'static>(variant: LoadingIndicatorVariant) -> AnyView<State> {
    match variant {
        LoadingIndicatorVariant::Default => any(loading_indicator()),
        LoadingIndicatorVariant::Contained => any(loading_indicator().contained()),
    }
}

/// The FRUST snippet text for the current knob state — the reference's
/// `_snippets` getter, ported to real `frust_material` code rather than a
/// Dart string.
fn snippet_code(state: &Knobs) -> String {
    match state.variant {
        LoadingIndicatorVariant::Default => "loading_indicator();".to_string(),
        LoadingIndicatorVariant::Contained => "loading_indicator().contained();".to_string(),
    }
}

/// Lay `items` out horizontally with `gap`px between each pair — see the
/// module docs' `Wrap` divergence.
fn spaced_row<State: 'static>(items: Vec<AnyView<State>>, gap: f64) -> impl View<State> {
    let mut children: Vec<AnyView<State>> = Vec::with_capacity(items.len() * 2);
    for item in items {
        if !children.is_empty() {
            children.push(any(SizedBox::<State>(Some(gap), None)));
        }
        children.push(item);
    }
    Row(children).cross_axis(CrossAxisAlignment::Center)
}

/// The playground body for the current knob state.
fn body(state: &mut Knobs) -> impl View<Knobs> {
    let primary_preview = play_preview_card("Loading indicator", indicator_for(state.variant));
    let both_preview = play_preview_card(
        "Both variants",
        spaced_row(
            vec![
                indicator_for(LoadingIndicatorVariant::Default),
                indicator_for(LoadingIndicatorVariant::Contained),
            ],
            24.0,
        ),
    );
    let snippet = play_snippet("Loading indicator", snippet_code(state));

    let controls = control_panel(
        "Appearance",
        vec![play_enum_segmented(
            "Variant",
            state.variant,
            &VARIANTS,
            variant_label,
            |s: &mut Knobs, v: LoadingIndicatorVariant| s.variant = v,
        )],
    );

    playground_body(
        vec![primary_preview, both_preview],
        vec![snippet],
        vec![controls],
    )
}

/// The nested [`Component`] this page owns its knobs in. See the page
/// contract in [`crate::pages::playground`].
#[derive(Default)]
struct LoadingIndicatorPlayground;

impl Component for LoadingIndicatorPlayground {
    type State = Knobs;

    fn init(&self) -> Knobs {
        Knobs::default()
    }

    fn build(&self, state: &mut Knobs) -> impl View<Knobs> {
        body(state)
    }
}

/// See the page contract in [`crate::pages::playground`].
pub fn page(_entry: DemoEntry) -> AnyView<AppState> {
    any(component(LoadingIndicatorPlayground))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every state this file's own controls can reach still builds a body.
    #[test]
    fn the_page_builds_across_every_reachable_control_state() {
        let mut knobs = Knobs::default();
        for variant in VARIANTS {
            knobs.variant = variant;
            let _view = body(&mut knobs);
        }
    }

    #[test]
    fn the_snippet_switches_between_variants() {
        let mut knobs = Knobs::default();
        assert_eq!(snippet_code(&knobs), "loading_indicator();");
        knobs.variant = LoadingIndicatorVariant::Contained;
        assert_eq!(snippet_code(&knobs), "loading_indicator().contained();");
    }
}
