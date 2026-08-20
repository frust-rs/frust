//! Labeled panel wrapping a list of control rows — the reference's
//! `PlayControlPanel`.

use frust::{
    AnyView, Column, CrossAxisAlignment, EdgeInsets, Padding, SizedBox, any, container, text,
};
use frust_material::{MaterialDimensions, MaterialSpacing};

use super::ambient_theme;

/// Wrap `children` (control rows, e.g. [`super::play_slider`]/
/// [`super::play_switch`] output) in a titled, outlined panel — the standard
/// frame every playground's controls sit in.
pub fn control_panel<State: 'static>(
    title: impl Into<String>,
    children: Vec<AnyView<State>>,
) -> AnyView<State> {
    let theme = ambient_theme();
    let scheme = theme.scheme();
    let title_style = theme.type_scale.title_small.clone();
    let fill = scheme.surface_container_lowest;
    let outline = scheme.outline_variant;

    let mut rows: Vec<AnyView<State>> = vec![
        any(text(title.into()).style(title_style)),
        any(SizedBox::<State>(None, Some(MaterialSpacing::MD))),
    ];
    rows.extend(children);

    any(Padding(
        EdgeInsets {
            left: 0.0,
            top: 0.0,
            right: 0.0,
            bottom: MaterialSpacing::MD,
        },
        container(Padding(
            EdgeInsets::all(MaterialSpacing::LG),
            Column(rows).cross_axis(CrossAxisAlignment::Stretch),
        ))
        .fill(fill)
        .radius(MaterialDimensions::RADIUS_LARGE)
        .border(outline, 1.0),
    ))
}

#[cfg(test)]
mod tests {
    use frust::{any, text};

    use super::control_panel;

    struct TestState;

    #[test]
    fn builds_around_control_rows() {
        let _view =
            control_panel::<TestState>("Options", vec![any(text("row one")), any(text("row two"))]);
    }

    #[test]
    fn builds_with_no_rows() {
        let _view = control_panel::<TestState>("Options", vec![]);
    }
}
