//! Tooltips: the reference's `TooltipsPlayground`.
//!
//! `frust_material::tooltip`'s trigger and panel are two sibling views over
//! one shared [`TooltipHover`] handle (see that module's own docs), not one
//! nested widget the way `M3ETooltip(child: ...)` reads in the reference. The
//! trigger stays inline with the preview card; the panel — floated through
//! the framework's overlay portal, anchored in window space to the trigger's
//! captured rect — mounts at this page's own outer `Stack`, the same
//! placement [`crate::widgets::playground`]'s [`play_enum_menu_field`]/
//! [`play_enum_menu_panel`] pair documents, so it never gets clipped by
//! [`playground_body`]'s own `scroll_view`.
//!
//! [`play_enum_menu_field`]: crate::widgets::playground::play_enum_menu_field
//! [`play_enum_menu_panel`]: crate::widgets::playground::play_enum_menu_panel
//!
//! Permanent mounting with paint-gated portal registration, no exit fade, and
//! hover with zero added delay are all upstream-faithful per that module's
//! own docs — this page drives no knob for any of the three, since none is
//! exposed as a prop upstream either.

use frust::{AnyView, Component, View, any, component, either, icon, stack};
use frust_material::{
    IconButtonVariant, TooltipHover, icon_button, icons, rich_tooltip, tooltip, tooltip_action,
    tooltip_trigger,
};

use crate::AppState;
use crate::catalog::DemoEntry;
use crate::widgets::playground::{
    control_panel, play_preview_card, play_snippet, play_switch, play_text_field, playground_body,
};

/// This playground's knobs — the reference's `_TooltipsPlaygroundState`.
struct Knobs {
    rich: bool,
    message: String,
    rich_title: String,
    rich_message: String,
    /// Shared by whichever of [`tooltip`]/[`rich_tooltip`] is currently
    /// mounted — see the module docs.
    hover: TooltipHover,
}

impl Default for Knobs {
    fn default() -> Self {
        Self {
            rich: false,
            message: "Compose a new message".to_string(),
            rich_title: "Compose".to_string(),
            rich_message: "Start a new draft with expressive defaults.".to_string(),
            hover: TooltipHover::new(),
        }
    }
}

/// The FRUST snippet text for the current knob state — the reference's
/// `_snippets` getter, ported to real `frust_material` code rather than a
/// Dart string.
fn snippet_code(state: &Knobs) -> String {
    let trigger = "tooltip_trigger(\n    &hover,\n    \
                   icon_button(icon(icons::EDIT), |_: &mut State| {}).variant(IconButtonVariant::Tonal),\n\
                   );";
    let panel = if state.rich {
        format!(
            "rich_tooltip(&hover, {:?})\n    .title({:?})\n    \
             .action(tooltip_action(\"Got it\", |_: &mut State| {{}}));",
            state.rich_message, state.rich_title
        )
    } else {
        format!("tooltip(&hover, {:?});", state.message)
    };
    format!("{trigger}\n{panel}")
}

/// The preview card: the icon-button trigger, wired to the shared hover.
fn preview_trigger(state: &Knobs) -> impl View<Knobs> {
    let button =
        icon_button(icon(icons::EDIT), |_: &mut Knobs| {}).variant(IconButtonVariant::Tonal);
    let label = if state.rich {
        "Rich tooltip"
    } else {
        "Plain tooltip"
    };
    play_preview_card(label, tooltip_trigger(&state.hover, button))
}

/// The portal panel itself — mounted unconditionally beside the trigger (see
/// the module docs); its own portal registration, gated on a live
/// [`TooltipHover::is_open`] read during paint, is what makes it invisible
/// while `hover` reads closed.
fn panel(state: &Knobs) -> impl View<Knobs> {
    either(
        state.rich,
        || {
            rich_tooltip(&state.hover, state.rich_message.clone())
                .title(state.rich_title.clone())
                .action(tooltip_action("Got it", |_: &mut Knobs| {}))
        },
        || tooltip(&state.hover, state.message.clone()),
    )
}

/// The playground body for the current knob state: [`playground_body`]'s
/// scrollable content, plus the portal panel at the outer `Stack` — see
/// the module docs.
fn body(state: &mut Knobs) -> impl View<Knobs> {
    let preview = preview_trigger(state);
    let snippet_label = if state.rich {
        "Rich tooltip"
    } else {
        "Plain tooltip"
    };
    let snippet = play_snippet(snippet_label, snippet_code(state));

    let controls = control_panel(
        "Content",
        (
            play_switch("Rich tooltip", state.rich, |s: &mut Knobs, v| s.rich = v),
            state.rich.then(|| {
                play_text_field(
                    "Rich title",
                    state.rich_title.clone(),
                    |s: &mut Knobs, v| s.rich_title = v,
                )
            }),
            state.rich.then(|| {
                play_text_field(
                    "Rich message",
                    state.rich_message.clone(),
                    |s: &mut Knobs, v| s.rich_message = v,
                )
            }),
            (!state.rich).then(|| {
                play_text_field("Message", state.message.clone(), |s: &mut Knobs, v| {
                    s.message = v
                })
            }),
        ),
    );

    let content = playground_body(preview, vec![snippet], controls);
    stack().child(content).child(panel(state))
}

/// The nested [`Component`] this page owns its knobs in. See the page
/// contract in [`crate::pages::playground`].
#[derive(Default)]
struct TooltipsPlayground;

impl Component for TooltipsPlayground {
    type State = Knobs;

    fn init(&self) -> Knobs {
        Knobs::default()
    }

    fn build(&self, state: &mut Knobs) -> impl View<Knobs> {
        body(state)
    }
}

/// See the page contract in [`crate::pages::playground`].
// erasure: keep stored page builder (a catalog PlaygroundBuilder returns AnyView<AppState>)
pub fn page(_entry: DemoEntry) -> AnyView<AppState> {
    any(component(TooltipsPlayground))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every state this file's own controls can reach still builds a body —
    /// the plain/rich swap never leaves a dangling row or a stale snippet.
    #[test]
    fn the_page_builds_across_every_reachable_control_state() {
        let mut knobs = Knobs::default();
        let _view = body(&mut knobs);
        knobs.rich = true;
        let _view = body(&mut knobs);
        knobs.rich = false;
        let _view = body(&mut knobs);
    }

    #[test]
    fn the_snippet_switches_between_plain_and_rich() {
        let mut knobs = Knobs::default();
        assert!(snippet_code(&knobs).starts_with("tooltip_trigger"));
        assert!(snippet_code(&knobs).contains("tooltip(&hover"));
        knobs.rich = true;
        assert!(snippet_code(&knobs).contains("rich_tooltip(&hover"));
        assert!(snippet_code(&knobs).contains(".title("));
    }
}
