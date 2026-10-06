//! Boolean control row using a switch — the reference's `PlaySwitch`.

use frust::{AnyView, CrossAxisAlignment, EdgeInsets, Padding, any, row, text};
use frust_material::{MaterialSpacing, switch};

use crate::widgets::playground::ambient_theme;

/// A row toggling `value`.
pub fn play_switch<State: 'static>(
    label: impl Into<String>,
    value: bool,
    on_changed: impl Fn(&mut State, bool) + 'static,
) -> AnyView<State> {
    let theme = ambient_theme();
    let mut body = theme.type_scale.body_large.clone();
    body.color = theme.scheme().on_surface;

    any(Padding(
        EdgeInsets {
            left: 0.0,
            top: 0.0,
            right: 0.0,
            bottom: MaterialSpacing::SM,
        },
        row()
            .flex(1, text(label.into()).style(body))
            .child(switch(value, on_changed))
            .cross_axis(CrossAxisAlignment::Center),
    ))
}

#[cfg(test)]
mod tests {
    use super::play_switch;

    struct TestState(bool);

    #[test]
    fn builds_a_switch_row() {
        let _view = play_switch::<TestState>("Enabled", true, |state, next| state.0 = next);
    }
}
