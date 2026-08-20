//! Monospace snippet with a copy action — the reference's `PlayCodeSnippet` /
//! `PlaySnippet`.
//!
//! `kPlaySnippetImport`/`playDartString` are not ported — see
//! [`super`]'s module docs for why.

use frust::authoring::text::{FontFamily, GenericSlot};
use frust::{
    AnyView, Axis, Column, CrossAxisAlignment, EdgeInsets, FlexView, Padding, SizedBox, any,
    container, flexible, icon, inflexible, text,
};
use frust_material::{IconButtonSize, MaterialDimensions, MaterialSpacing, icon_button, icons};

use super::ambient_theme;

/// The family name `frust_material::install()` registers Roboto Mono under.
/// A literal because the plugin keeps its own family constants private; it
/// must match what the installer registers or the stack falls through to the
/// generic slot — the same precedent `crate::theme::settings` pins for
/// Roboto Flex/Mono.
const ROBOTO_MONO_FAMILY: &str = "Roboto Mono";

/// A paste-ready code sample shown under a playground preview.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaySnippet {
    /// Title above the block.
    pub label: String,
    /// Frust source, shown verbatim and copied verbatim — never Dart, unlike
    /// the reference's own `PlaySnippet.code` (see [`super`]'s module docs).
    pub code: String,
}

/// Build a labeled code sample.
pub fn play_snippet(label: impl Into<String>, code: impl Into<String>) -> PlaySnippet {
    PlaySnippet {
        label: label.into(),
        code: code.into(),
    }
}

/// Copy `text` to the platform clipboard — the snippet card's copy action,
/// factored out of [`play_code_snippet`] so it is unit-testable without a
/// widget tree.
pub fn copy_to_clipboard(text: &str) -> Result<(), frust_clipboard::ClipboardError> {
    frust_clipboard::Clipboard::set_text(text)
}

/// Render `snippet` as a monospace block with a copy button.
pub fn play_code_snippet<State: 'static>(snippet: &PlaySnippet) -> AnyView<State> {
    let theme = ambient_theme();
    let scheme = theme.scheme();
    let mut label_style = theme.type_scale.label_large.clone();
    label_style.color = scheme.on_surface_variant;
    let mut code_style = theme.type_scale.body_small.clone();
    code_style.color = scheme.on_surface;
    code_style.family =
        FontFamily::stack_with_generic([ROBOTO_MONO_FAMILY], GenericSlot::Monospace);
    let fill = scheme.surface_container_low;

    let code = snippet.code.clone();
    let header = FlexView::new(
        Axis::Horizontal,
        vec![
            flexible(1, text(snippet.label.clone()).style(label_style)),
            inflexible(
                icon_button(any(icon(icons::CONTENT_COPY)), move |_state: &mut State| {
                    let _ = copy_to_clipboard(&code);
                })
                .size(IconButtonSize::Sm)
                .semantic_label("Copy snippet"),
            ),
        ],
    )
    .cross_axis(CrossAxisAlignment::Center);

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
                any(header),
                any(SizedBox::<State>(None, Some(MaterialSpacing::MD))),
                any(text(snippet.code.clone()).style(code_style)),
            ])
            .cross_axis(CrossAxisAlignment::Stretch),
        ))
        .fill(fill)
        .radius(MaterialDimensions::RADIUS_LARGE),
    ))
}

#[cfg(test)]
mod tests {
    use super::{copy_to_clipboard, play_code_snippet, play_snippet};

    struct TestState;

    fn clipboard_tests_opted_in() -> bool {
        std::env::var("FRUST_CLIPBOARD_TESTS").as_deref() == Ok("1")
    }

    #[test]
    fn snippet_builder_captures_label_and_code() {
        let snippet = play_snippet("Filled", "filled_button(\"Go\", |_| {})");
        assert_eq!(snippet.label, "Filled");
        assert_eq!(snippet.code, "filled_button(\"Go\", |_| {})");
    }

    #[test]
    fn builds_around_a_snippet() {
        let snippet = play_snippet("Filled", "filled_button(\"Go\", |_| {})");
        let _view = play_code_snippet::<TestState>(&snippet);
    }

    #[test]
    fn copy_round_trips_through_the_platform_clipboard() {
        if !clipboard_tests_opted_in() {
            eprintln!(
                "skipping material3-demo copy_round_trips: destructive \
                 (overwrites the real host clipboard) — opt in with \
                 `FRUST_CLIPBOARD_TESTS=1 cargo test -p material3-demo`"
            );
            return;
        }
        let payload = "frust_material::filled_button(\"Go\", |_| {})";
        if copy_to_clipboard(payload).is_err() {
            // No clipboard mechanism reachable in this environment (e.g. a
            // headless CI runner) — not this test's concern; see
            // `frust_clipboard::Unavailability`.
            return;
        }
        assert_eq!(
            frust_clipboard::Clipboard::get_text().unwrap().as_deref(),
            Some(payload)
        );
    }
}
