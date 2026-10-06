//! Text fields: the reference's `TextFieldsPlayground`.
//!
//! One preview ([`frust_material::text_field`]) plus its `Appearance`/
//! `Content` control panels — a straight port, since the wrapped field's
//! builder surface already covers every prop the reference's
//! `_TextFieldsPlaygroundState` drives (`variant`, `enabled`, `obscureText`,
//! a leading icon, and the label/supporting/error trio). See
//! [`frust_material::text_field`]'s own module docs for the wrapper's known
//! limitations (single-line, no per-field alignment) — none of them are
//! reachable from this playground's controls.

use frust::{AnyView, Component, View, any, component, icon};
use frust_material::{TextFieldVariant, icons, text_field};

use crate::AppState;
use crate::catalog::DemoEntry;
use crate::widgets::playground::{
    control_panel, play_enum_segmented, play_preview_card, play_snippet, play_switch,
    play_text_field, playground_body,
};

/// This playground's knobs — the reference's `_TextFieldsPlaygroundState`.
struct Knobs {
    /// The field's own live editable content — the reference's
    /// `TextEditingController`, distinct from the label/supporting/error
    /// props below (each edited through its own [`play_text_field`] row).
    value: String,
    variant: TextFieldVariant,
    enabled: bool,
    obscure: bool,
    show_leading: bool,
    show_error: bool,
    label: String,
    supporting: String,
    error: String,
}

impl Default for Knobs {
    fn default() -> Self {
        Self {
            value: String::new(),
            variant: TextFieldVariant::Filled,
            enabled: true,
            obscure: false,
            show_leading: true,
            show_error: false,
            label: "Full name".to_string(),
            supporting: "As it appears on your ID".to_string(),
            error: "Enter a valid value".to_string(),
        }
    }
}

/// Every [`TextFieldVariant`], for the segmented control.
const VARIANTS: [TextFieldVariant; 2] = [TextFieldVariant::Filled, TextFieldVariant::Outlined];

/// [`TextFieldVariant`]'s display label — the reference's `v.name`.
fn variant_label(variant: TextFieldVariant) -> &'static str {
    match variant {
        TextFieldVariant::Filled => "Filled",
        TextFieldVariant::Outlined => "Outlined",
    }
}

/// The FRUST snippet text for the current knob state — the reference's
/// `_snippets` getter, ported to real `frust_material` code rather than a
/// Dart string.
fn snippet_code(state: &Knobs) -> String {
    let supporting_or_error = if state.show_error {
        format!("\n    .error_text({:?})", state.error)
    } else {
        format!("\n    .supporting_text({:?})", state.supporting)
    };
    let leading = if state.show_leading {
        "\n    .leading(any(icon(icons::EDIT)))".to_string()
    } else {
        String::new()
    };
    format!(
        "text_field(value, on_change)\n    .label({:?}){supporting_or_error}\n    .variant(TextFieldVariant::{:?})\n    .enabled({}){leading}\n    .obscured({});",
        state.label, state.variant, state.enabled, state.obscure
    )
}

/// The playground body for the current knob state.
fn body(state: &mut Knobs) -> AnyView<Knobs> {
    let mut field = text_field(state.value.clone(), |s: &mut Knobs, v: String| s.value = v)
        .label(state.label.clone())
        .variant(state.variant)
        .enabled(state.enabled)
        .obscured(state.obscure);
    field = if state.show_error {
        field.error_text(state.error.clone())
    } else {
        field.supporting_text(state.supporting.clone())
    };
    if state.show_leading {
        field = field.leading(any(icon(icons::EDIT)));
    }
    let preview = play_preview_card("Text field", field);
    let snippet = play_snippet("Text field", snippet_code(state));

    let appearance = control_panel(
        "Appearance",
        vec![
            play_enum_segmented(
                "Variant",
                state.variant,
                &VARIANTS,
                variant_label,
                |s: &mut Knobs, v: TextFieldVariant| s.variant = v,
            ),
            play_switch("Enabled", state.enabled, |s: &mut Knobs, v| s.enabled = v),
            play_switch("Obscure text", state.obscure, |s: &mut Knobs, v| {
                s.obscure = v
            }),
            play_switch("Leading icon", state.show_leading, |s: &mut Knobs, v| {
                s.show_leading = v
            }),
            play_switch("Error", state.show_error, |s: &mut Knobs, v| {
                s.show_error = v
            }),
        ],
    );
    let content = control_panel(
        "Content",
        vec![
            play_text_field("Label", state.label.clone(), |s: &mut Knobs, v| s.label = v),
            play_text_field(
                "Supporting text",
                state.supporting.clone(),
                |s: &mut Knobs, v| s.supporting = v,
            ),
            play_text_field("Error text", state.error.clone(), |s: &mut Knobs, v| {
                s.error = v
            }),
        ],
    );

    playground_body(vec![preview], vec![snippet], vec![appearance, content])
}

/// The nested [`Component`] this page owns its knobs in. See the page
/// contract in [`crate::pages::playground`].
#[derive(Default)]
struct TextFieldsPlayground;

impl Component for TextFieldsPlayground {
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
    any(component(TextFieldsPlayground))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every state this file's own controls can reach still builds a body —
    /// the enum/text-field lookups this module computes from [`Knobs`] never
    /// panic across the reachable combinations.
    #[test]
    fn the_page_builds_across_every_reachable_control_state() {
        let mut knobs = Knobs::default();
        for variant in VARIANTS {
            knobs.variant = variant;
            let _view = body(&mut knobs);
        }
        for enabled in [true, false] {
            knobs.enabled = enabled;
            let _view = body(&mut knobs);
        }
        for obscure in [true, false] {
            knobs.obscure = obscure;
            let _view = body(&mut knobs);
        }
        for show_leading in [true, false] {
            knobs.show_leading = show_leading;
            let _view = body(&mut knobs);
        }
        for show_error in [true, false] {
            knobs.show_error = show_error;
            let _view = body(&mut knobs);
        }
    }

    #[test]
    fn the_snippet_switches_between_supporting_and_error_text() {
        let mut knobs = Knobs::default();
        assert!(snippet_code(&knobs).contains(".supporting_text"));
        knobs.show_error = true;
        assert!(snippet_code(&knobs).contains(".error_text"));
        assert!(!snippet_code(&knobs).contains(".supporting_text"));
    }
}
