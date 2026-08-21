//! Labeled panel wrapping a list of control rows — the reference's
//! `PlayControlPanel`.
//!
//! # Section-title contrast (dark-on-dark fix)
//!
//! The title used to take `theme.type_scale.title_small` unstyled. That
//! reads fine on paper but resolves to solid black in *every* theme: a
//! `TypeScale` token's `color` field is carried straight through from
//! whatever `base: &TextStyle` the design system built its scale from
//! (`TypeScale`'s own doc comment — "its `family`/`style`/`color` are
//! kept"), and `frust_material::baseline` builds its scale from
//! `TextStyle::default()`, whose color is `Color::BLACK`
//! (`crates/frust-text/src/style.rs`). `text(..).style(..)` then marks that
//! color *explicit* (`TextView::style`'s own doc comment), which permanently
//! opts the run out of `frust-widgets`' themed `on_surface` fallback
//! (`ThemeTextColor`) — the mechanism that would otherwise have saved an
//! unstyled title. On a `surface_container_lowest` panel fill (dark theme:
//! `~0x0F0D13`, near black) that leaves black-on-near-black: unreadable, and
//! invisible to a host-side test since paint-time color resolution needs a
//! real render pass to observe.
//!
//! This is **not** a framework defect — every other file in this kit
//! (`body.rs`, `code_snippet.rs`, `preview_card.rs`) already overrides
//! `.color` from the scheme before calling `.style(..)`, exactly as
//! `TypeScale`'s doc comment expects every consumer to. `control_panel.rs`
//! was the one call site in this directory that skipped it — a kit-local
//! bug, fixed the same way its siblings already are: [`title_style`] sets an
//! explicit `on_surface_variant` (the M3 convention for a de-emphasized
//! section header on a surface), which both `light`/`dark`
//! `frust_material::baseline` schemes resolve to real contrast against
//! `surface_container_lowest` (see this module's tests).

use frust::authoring::text::TextStyle;
use frust::{
    AnyView, Column, CrossAxisAlignment, EdgeInsets, Padding, SizedBox, Theme, any, container, text,
};
use frust_material::{MaterialDimensions, MaterialSpacing};

use super::ambient_theme;

/// Resolve the section title's style: `title_small` with an explicit
/// `on_surface_variant` color (see this module's doc comment for why the
/// override can't be left to the themed default). Factored out so the
/// resolved color is unit-testable without a widget tree — the
/// `code_snippet.rs`/`copy_to_clipboard` precedent.
fn title_style(theme: &Theme) -> TextStyle {
    let mut style = theme.type_scale.title_small.clone();
    style.color = theme.scheme().on_surface_variant;
    style
}

/// Wrap `children` (control rows, e.g. [`super::play_slider`]/
/// [`super::play_switch`] output) in a titled, outlined panel — the standard
/// frame every playground's controls sit in.
pub fn control_panel<State: 'static>(
    title: impl Into<String>,
    children: Vec<AnyView<State>>,
) -> AnyView<State> {
    let theme = ambient_theme();
    let scheme = theme.scheme();
    let title_style = title_style(&theme);
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
    use frust::{Brightness, any, text};

    use super::{control_panel, title_style};

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

    /// Regression coverage for the dark-on-dark title fix (see this module's
    /// doc comment): the resolved title color must be the M3
    /// `on_surface_variant` token — not the type scale's carried-through
    /// black default — and must differ from the panel's own
    /// `surface_container_lowest` fill, in both themes but especially dark
    /// (where the fill is near black). Reverting the `title_style` override
    /// collapses `style.color` back to `Color::BLACK`
    /// (`theme.type_scale.title_small`'s carried-through default), which
    /// fails the `on_surface_variant` equality below regardless of
    /// brightness.
    #[test]
    fn title_style_resolves_on_surface_variant_and_differs_from_panel_fill() {
        for brightness in [Brightness::Light, Brightness::Dark] {
            let theme = frust_material::baseline().with_brightness(brightness);
            let scheme = theme.scheme();
            let style = title_style(&theme);

            assert_eq!(
                style.color, scheme.on_surface_variant,
                "{brightness:?} title color should resolve to on_surface_variant"
            );
            assert_ne!(
                style.color, scheme.surface_container_lowest,
                "{brightness:?} title color must stay legible against the panel fill"
            );
        }
    }
}
