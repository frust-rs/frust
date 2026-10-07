//! Card-like surface wrapping a labeled preview child — the reference's
//! `PlayPreviewCard`.
//!
//! # Filling the available width
//!
//! A [`Column`] with no `.cross_axis` override hugs its widest child (the
//! default [`CrossAxisAlignment::Start`]), so a narrow preview (most of
//! them) left the card itself narrow too — Ed's device report ("almost all
//! examples"). Mirrors [`super::code_snippet::play_code_snippet`]'s
//! proven-on-device fix: the label sits in a `FlexView(Axis::Horizontal,
//! [flexible(1, label)])` row (a horizontal flex fills its axis) and the
//! outer [`Column`] switches to [`CrossAxisAlignment::Stretch`], both of
//! which report `bc`'s bounded max width regardless of their content
//! (`frust_widgets::flex`'s stretch/`bc.constrain` combination). Stretch also
//! hands the preview *child* a tight width constraint, which would distort
//! anything narrower than the card into filling it — [`Align`] wraps the
//! child to undo exactly that: it loosens the child's own constraint back to
//! natural width and repositions it at [`Alignment::TOP_LEFT`], the same
//! leading position `CrossAxisAlignment::Start` gave it before this fix.
use frust::{
    Align, Alignment, AnyView, CrossAxisAlignment, EdgeInsets, Padding, SizedBox, View, any,
    column, container, row, text,
};
use frust_material::{MaterialDimensions, MaterialSpacing};

use super::ambient_theme;

/// Wrap `child` in a labeled, tinted surface — the standard frame every
/// playground preview sits in.
// erasure: keep element of a heterogeneous Vec<AnyView> list (playground_body previews)
pub fn play_preview_card<State: 'static>(
    label: impl Into<String>,
    child: impl View<State>,
) -> AnyView<State> {
    let theme = ambient_theme();
    let scheme = theme.scheme();
    let mut label_style = theme.type_scale.label_large.clone();
    label_style.color = scheme.on_surface_variant;
    let fill = scheme.surface_container_low;

    let header = row().flex(1, text(label.into()).style(label_style));

    any(Padding(
        EdgeInsets {
            left: 0.0,
            top: 0.0,
            right: 0.0,
            bottom: MaterialSpacing::MD,
        },
        container(Padding(
            EdgeInsets::all(MaterialSpacing::LG),
            column()
                .child(header)
                .child(SizedBox::<State>(None, Some(MaterialSpacing::MD)))
                .child(Align(Alignment::TOP_LEFT, child))
                .cross_axis(CrossAxisAlignment::Stretch),
        ))
        .fill(fill)
        .radius(MaterialDimensions::RADIUS_LARGE),
    ))
}

#[cfg(test)]
mod tests {
    use frust::{SizedBox, text};

    use super::play_preview_card;

    struct TestState;

    #[test]
    fn builds_around_a_child() {
        let _view = play_preview_card::<TestState>("Filled", text("preview"));
    }

    /// Regression coverage for the width-fill fix (see the module docs):
    /// the outer `Column` now stretches and the child is re-wrapped in
    /// `Align`, so a child far narrower than the card — the common
    /// playground shape (an icon-sized control, not full-width itself) —
    /// must still build cleanly through both wrappers. Actual fill-width
    /// behavior is a paint-time layout fact this crate has no `RenderRoot`
    /// test harness to assert on host-side (this standalone workspace
    /// depends on the `frust` facade only, not `frust-core`/`frust-widgets`);
    /// the material3 gallery's device gate (`docs/DEVELOPMENT.md`'s *Run*
    /// section) is the real verification, the same shape
    /// `play_code_snippet`'s own tests already accept for its proven fix.
    #[test]
    fn builds_around_a_child_narrower_than_the_card() {
        let _view = play_preview_card::<TestState>(
            "Icon-sized",
            SizedBox::<TestState>(Some(24.0), Some(24.0)),
        );
    }
}
