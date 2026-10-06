//! Continuous value control using a slider — the reference's `PlaySlider`.

use std::ops::RangeInclusive;

use frust::{AnyView, CrossAxisAlignment, EdgeInsets, Padding, any, column, text};
use frust_material::{MaterialSpacing, slider};

use crate::widgets::playground::ambient_theme;

/// The label suffix showing the current value — two decimals for a
/// continuous slider, none for a stepped one (the reference's
/// `value.toStringAsFixed(divisions == null ? 2 : 0)`).
fn value_label(label: &str, value: f64, divisions: Option<u32>) -> String {
    match divisions {
        Some(_) => format!("{label} ({value:.0})"),
        None => format!("{label} ({value:.2})"),
    }
}

/// A row controlling `value` over `range`, continuous unless `divisions`
/// steps it.
pub fn play_slider<State: 'static>(
    label: impl Into<String>,
    value: f64,
    range: RangeInclusive<f64>,
    divisions: Option<u32>,
    on_changed: impl Fn(&mut State, f64) + 'static,
) -> AnyView<State> {
    let label = label.into();
    let theme = ambient_theme();
    let mut body = theme.type_scale.body_large.clone();
    body.color = theme.scheme().on_surface;
    let display = value_label(&label, value, divisions);

    let mut track = slider(value, on_changed).range(*range.start(), *range.end());
    if let Some(steps) = divisions {
        track = track.divisions(steps);
    }

    any(Padding(
        EdgeInsets {
            left: 0.0,
            top: 0.0,
            right: 0.0,
            bottom: MaterialSpacing::MD,
        },
        column()
            .child(text(display).style(body))
            .child(track)
            .cross_axis(CrossAxisAlignment::Stretch),
    ))
}

#[cfg(test)]
mod tests {
    use super::{play_slider, value_label};

    struct TestState(f64);

    #[test]
    fn continuous_label_shows_two_decimals() {
        assert_eq!(value_label("Size", 0.5, None), "Size (0.50)");
    }

    #[test]
    fn stepped_label_shows_no_decimals() {
        assert_eq!(value_label("Steps", 3.0, Some(5)), "Steps (3)");
    }

    #[test]
    fn builds_a_continuous_slider() {
        let _view = play_slider::<TestState>("Size", 0.5, 0.0..=1.0, None, |state, v| {
            state.0 = v;
        });
    }

    #[test]
    fn builds_a_stepped_slider() {
        let _view = play_slider::<TestState>("Steps", 3.0, 0.0..=5.0, Some(5), |state, v| {
            state.0 = v;
        });
    }
}
