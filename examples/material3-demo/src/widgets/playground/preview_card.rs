//! Card-like surface wrapping a labeled preview child — the reference's
//! `PlayPreviewCard`.

use frust::{
    AnyView, Column, CrossAxisAlignment, EdgeInsets, Padding, SizedBox, View, any, container, text,
};
use frust_material::{MaterialDimensions, MaterialSpacing};

use super::ambient_theme;

/// Wrap `child` in a labeled, tinted surface — the standard frame every
/// playground preview sits in.
pub fn play_preview_card<State: 'static>(
    label: impl Into<String>,
    child: impl View<State>,
) -> AnyView<State> {
    let theme = ambient_theme();
    let scheme = theme.scheme();
    let mut label_style = theme.type_scale.label_large.clone();
    label_style.color = scheme.on_surface_variant;
    let fill = scheme.surface_container_low;

    any(Padding(
        EdgeInsets {
            left: 0.0,
            top: 0.0,
            right: 0.0,
            bottom: MaterialSpacing::MD,
        },
        container(Padding(
            EdgeInsets::all(MaterialSpacing::LG),
            Column(vec![
                any(text(label.into()).style(label_style)),
                any(SizedBox::<State>(None, Some(MaterialSpacing::MD))),
                any(child),
            ])
            .cross_axis(CrossAxisAlignment::Start),
        ))
        .fill(fill)
        .radius(MaterialDimensions::RADIUS_LARGE),
    ))
}

#[cfg(test)]
mod tests {
    use frust::text;

    use super::play_preview_card;

    struct TestState;

    #[test]
    fn builds_around_a_child() {
        let _view = play_preview_card::<TestState>("Filled", text("preview"));
    }
}
