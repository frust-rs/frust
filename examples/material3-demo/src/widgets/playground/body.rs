//! Scrollable playground body: preview(s), optional code snippets, then
//! controls — the reference's `PlaygroundBody`.

use frust::{AnyView, Column, EdgeInsets, Padding, SizedBox, View, any, scroll_view, text};
use frust_material::MaterialSpacing;

use super::code_snippet::play_code_snippet;
use super::{PlaySnippet, ambient_theme};

/// Assemble a playground body from `previews`, optional paste-ready
/// `snippets`, and `controls` — the vertical arrangement every ported
/// playground page returns as its whole body (see the page contract in
/// [`crate::pages::playground`]).
pub fn playground_body<State: 'static>(
    previews: Vec<AnyView<State>>,
    snippets: Vec<PlaySnippet>,
    controls: Vec<AnyView<State>>,
) -> impl View<State> {
    let theme = ambient_theme();
    let mut section = theme.type_scale.title_medium.clone();
    section.color = theme.scheme().on_surface;

    let mut rows: Vec<AnyView<State>> = vec![
        any(text("Preview").style(section.clone())),
        any(SizedBox::<State>(None, Some(MaterialSpacing::MD))),
    ];
    rows.extend(previews);

    if !snippets.is_empty() {
        rows.push(any(SizedBox::<State>(None, Some(MaterialSpacing::XL))));
        rows.push(any(text("Code").style(section.clone())));
        rows.push(any(SizedBox::<State>(None, Some(MaterialSpacing::MD))));
        rows.extend(snippets.iter().map(|snippet| play_code_snippet(snippet)));
    }

    if !controls.is_empty() {
        rows.push(any(SizedBox::<State>(None, Some(MaterialSpacing::XL))));
        rows.push(any(text("Controls").style(section)));
        rows.push(any(SizedBox::<State>(None, Some(MaterialSpacing::MD))));
        rows.extend(controls);
    }

    scroll_view(Padding(
        EdgeInsets {
            left: MaterialSpacing::LG,
            top: MaterialSpacing::SM,
            right: MaterialSpacing::LG,
            bottom: MaterialSpacing::XXL,
        },
        Column(rows),
    ))
}

#[cfg(test)]
mod tests {
    use frust::{any, text};

    use super::playground_body;
    use crate::widgets::playground::play_snippet;

    struct TestState;

    #[test]
    fn builds_with_only_a_preview() {
        let _view = playground_body::<TestState>(vec![any(text("preview"))], vec![], vec![]);
    }

    #[test]
    fn builds_with_snippets_and_controls() {
        let _view = playground_body::<TestState>(
            vec![any(text("preview"))],
            vec![play_snippet("Filled", "filled_button(\"Go\", |_| {})")],
            vec![any(text("control"))],
        );
    }
}
