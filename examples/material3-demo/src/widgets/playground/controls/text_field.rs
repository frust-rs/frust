//! Text control using an outlined text field — the reference's
//! `PlayTextField`.
//!
//! The reference pairs a `TextEditingController` with `didUpdateWidget` to
//! keep it in sync with `widget.value` — `frust_material::text_field` is
//! already controlled the same way `Checkbox`/`Slider` are
//! (`docs/CODE_STANDARDS.md`'s Interaction Semantics: `rebuild` applies the
//! view's `value` set-if-different), so there is no controller to manage
//! here.

use frust::{AnyView, EdgeInsets, Padding, any};
use frust_material::{MaterialSpacing, TextFieldVariant, text_field};

/// A row editing `value` as plain text.
// erasure: keep element of a heterogeneous Vec<AnyView> list (control_panel children)
pub fn play_text_field<State: 'static>(
    label: impl Into<String>,
    value: impl Into<String>,
    on_changed: impl Fn(&mut State, String) + 'static,
) -> AnyView<State> {
    any(Padding(
        EdgeInsets {
            left: 0.0,
            top: 0.0,
            right: 0.0,
            bottom: MaterialSpacing::MD,
        },
        text_field(value.into(), on_changed)
            .label(label.into())
            .variant(TextFieldVariant::Outlined),
    ))
}

#[cfg(test)]
mod tests {
    use super::play_text_field;

    struct TestState(String);

    #[test]
    fn builds_an_outlined_text_field() {
        let _view = play_text_field::<TestState>("Label", "value", |state, next| {
            state.0 = next;
        });
    }
}
