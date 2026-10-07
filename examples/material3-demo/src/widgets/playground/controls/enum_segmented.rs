//! Discrete enum control via a segmented button — the reference's
//! `PlayEnumSegmented`.

use frust::{AnyView, CrossAxisAlignment, EdgeInsets, Padding, SizedBox, any, column, text};
use frust_material::{MaterialSpacing, segment, segmented_button};

use crate::widgets::playground::ambient_theme;

/// A row picking one of `values` via a segmented button (2-5 options — see
/// `frust_material::segmented_button`'s own bound).
// erasure: keep element of a heterogeneous Vec<AnyView> list (control_panel children)
pub fn play_enum_segmented<State: 'static, T: PartialEq + Copy + 'static>(
    label: impl Into<String>,
    value: T,
    values: &[T],
    label_of: fn(T) -> &'static str,
    on_changed: impl Fn(&mut State, T) + 'static,
) -> AnyView<State> {
    let theme = ambient_theme();
    let mut body = theme.type_scale.body_large.clone();
    body.color = theme.scheme().on_surface;

    let segments = values
        .iter()
        .map(|v| segment(*v).label(label_of(*v)))
        .collect::<Vec<_>>();

    any(Padding(
        EdgeInsets {
            left: 0.0,
            top: 0.0,
            right: 0.0,
            bottom: MaterialSpacing::MD,
        },
        column()
            .child(text(label.into()).style(body))
            .child(SizedBox::<State>(None, Some(MaterialSpacing::SM)))
            .child(segmented_button(
                segments,
                vec![value],
                move |state: &mut State, next: Vec<T>| {
                    if let Some(v) = next.into_iter().next() {
                        on_changed(state, v);
                    }
                },
            ))
            .cross_axis(CrossAxisAlignment::Stretch),
    ))
}

#[cfg(test)]
mod tests {
    use super::play_enum_segmented;

    struct TestState(Choice);

    #[derive(Clone, Copy, PartialEq, Debug)]
    enum Choice {
        A,
        B,
        C,
    }

    impl Choice {
        fn label(self) -> &'static str {
            match self {
                Choice::A => "A",
                Choice::B => "B",
                Choice::C => "C",
            }
        }
    }

    #[test]
    fn builds_with_two_to_five_segments() {
        let _view = play_enum_segmented::<TestState, Choice>(
            "Choice",
            Choice::A,
            &[Choice::A, Choice::B, Choice::C],
            Choice::label,
            |state, next| state.0 = next,
        );
    }
}
