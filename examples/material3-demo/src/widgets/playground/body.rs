//! Scrollable playground body: preview(s), optional code snippets, then
//! controls — the reference's `PlaygroundBody`.

use frust::{
    AnyView, EdgeInsets, Padding, SizedBox, View, ViewSeq, column, scroll_view, text, views,
};
use frust_material::MaterialSpacing;

use super::code_snippet::play_code_snippet;
use super::{PlaySnippet, ambient_theme};

/// Assemble a playground body from `previews`, optional paste-ready
/// `snippets`, and `controls` — the vertical arrangement every ported
/// playground page returns as its whole body (see the page contract in
/// [`crate::pages::playground`]).
///
/// `previews` and `controls` are any [`ViewSeq`]: a `Vec`, a tuple of mixed
/// views, or a single view.
pub fn playground_body<State: 'static, PM, CM>(
    previews: impl ViewSeq<State, PM>,
    snippets: Vec<PlaySnippet>,
    controls: impl ViewSeq<State, CM>,
) -> impl View<State> {
    let theme = ambient_theme();
    let mut section = theme.type_scale.title_medium.clone();
    section.color = theme.scheme().on_surface;

    // Emptiness is a property of the flattened sequence, so erase it once here.
    let mut control_views: Vec<AnyView<State>> = Vec::new();
    controls.extend_views(&mut control_views);

    scroll_view(Padding(
        EdgeInsets {
            left: MaterialSpacing::LG,
            top: MaterialSpacing::SM,
            right: MaterialSpacing::LG,
            bottom: MaterialSpacing::XXL,
        },
        column()
            .child(text("Preview").style(section.clone()))
            .child(SizedBox::<State>(None, Some(MaterialSpacing::MD)))
            .children(previews)
            .when(!snippets.is_empty(), |body| {
                body.child(SizedBox::<State>(None, Some(MaterialSpacing::XL)))
                    .child(text("Code").style(section.clone()))
                    .child(SizedBox::<State>(None, Some(MaterialSpacing::MD)))
                    .children(views(
                        snippets.iter().map(|snippet| play_code_snippet(snippet)),
                    ))
            })
            .when(!control_views.is_empty(), |body| {
                body.child(SizedBox::<State>(None, Some(MaterialSpacing::XL)))
                    .child(text("Controls").style(section))
                    .child(SizedBox::<State>(None, Some(MaterialSpacing::MD)))
                    .children(control_views)
            }),
    ))
}

#[cfg(test)]
mod tests {
    use frust::{AnyView, text};

    use super::playground_body;
    use crate::widgets::playground::play_snippet;

    struct TestState;

    #[test]
    fn builds_with_only_a_preview() {
        let _view = playground_body::<TestState, _, _>(
            text("preview"),
            vec![],
            Vec::<AnyView<TestState>>::new(),
        );
    }

    #[test]
    fn builds_with_snippets_and_controls() {
        let _view = playground_body::<TestState, _, _>(
            text("preview"),
            vec![play_snippet("Filled", "filled_button(\"Go\", |_| {})")],
            text("control"),
        );
    }
}
