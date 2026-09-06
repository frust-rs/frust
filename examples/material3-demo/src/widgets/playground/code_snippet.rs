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
                "skipping material3demo copy_round_trips: destructive \
                 (overwrites the real host clipboard) — opt in with \
                 `FRUST_CLIPBOARD_TESTS=1 cargo test` from this example's \
                 own directory (it is a standalone workspace)"
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

    /// Every `<ident>.dismiss()` / `<ident>.dismiss_with(` receiver
    /// identifier found in a snippet's paste-ready text — the collector
    /// below finds two calls per pattern by construction, `.dismiss()` and
    /// `.dismiss_with(`, never confusing either with `.dismiss_handle(` or a
    /// bare `dismiss.clone()` (no leading dot before the receiver's own
    /// name, so the pattern match starts one character later).
    fn dismiss_receivers(code: &str) -> Vec<String> {
        let mut receivers = Vec::new();
        for pattern in [".dismiss()", ".dismiss_with("] {
            let mut cursor = 0;
            while let Some(offset) = code[cursor..].find(pattern) {
                let call_at = cursor + offset;
                let ident_start = code[..call_at]
                    .rfind(|c: char| !(c.is_alphanumeric() || c == '_'))
                    .map(|i| i + 1)
                    .unwrap_or(0);
                let ident = &code[ident_start..call_at];
                if !ident.is_empty() {
                    receivers.push(ident.to_string());
                }
                cursor = call_at + pattern.len();
            }
        }
        receivers
    }

    /// A snippet's own contract ("shown verbatim and copied verbatim",
    /// [`super::PlaySnippet::code`]'s doc) means every dismiss receiver it
    /// prints must be bound with a `let <ident> =` somewhere in that same
    /// text — otherwise copying the snippet as shown doesn't compile.
    fn assert_dismiss_receivers_are_bound(label: &str, code: &str) {
        for receiver in dismiss_receivers(code) {
            let binding = format!("let {receiver} =");
            assert!(
                code.contains(&binding),
                "playground snippet {label:?} calls \
                 `{receiver}.dismiss()`/`{receiver}.dismiss_with(...)` but never binds it \
                 with `{binding}` anywhere in its own paste-ready text:\n{code}"
            );
        }
    }

    /// Sweeps every playground page whose paste-ready snippet mints a
    /// `ModalDismiss` (`view::dialogs`, `view::side_sheet`,
    /// `pick::time_pickers` — the full set as of this test's writing, a
    /// string search over `src/pages/playground` for a snippet-text
    /// `ModalDismiss::new();` literal) for the binding-sanity rule above.
    /// String-level only, deliberately: this crate carries no `syn`
    /// dev-dependency, and a full parse isn't needed to catch an undeclared
    /// receiver.
    #[test]
    fn every_playground_snippets_dismiss_receiver_is_bound_in_its_own_text() {
        use crate::pages::playground::pick::time_pickers::snippets_for_binding_test as time_picker_snippets;
        use crate::pages::playground::view::dialogs::snippets_for_binding_test as dialog_snippets;
        use crate::pages::playground::view::side_sheet::snippets_for_binding_test as side_sheet_snippets;

        let mut snippets = dialog_snippets();
        snippets.extend(side_sheet_snippets());
        snippets.extend(time_picker_snippets());

        assert!(
            snippets.len() >= 5,
            "expected at least the five known ModalDismiss-minting snippets, got {}",
            snippets.len()
        );
        for snippet in &snippets {
            assert_dismiss_receivers_are_bound(&snippet.label, &snippet.code);
        }
    }
}
