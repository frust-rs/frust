//! Dialogs: the reference's `DialogsPlayground`.
//!
//! All three variants the reference triggers are ported —
//! [`frust_material::show_dialog`], [`frust_material::show_selection_dialog`],
//! and [`frust_material::show_full_screen_dialog`] — each opened through the
//! documented `show_*` route onto this page's own navigator, so every
//! dismissal (scrim tap, `Escape`, Android back) takes the modal host's
//! staged exit ramp rather than an unstaged pop — and so do the panels' own
//! action buttons, through the [`frust_material::ModalDismiss`] each trigger
//! mints (a raw `nav.pop()` from an action would skip the ramp).
//!
//! # Descoped: `icon`, `topDivider`, `bottomDivider`
//!
//! The reference's `Show icon`, `Top divider`, and `Bottom divider` switches
//! drive `M3EDialog`'s `icon`/`topDivider`/`bottomDivider` parameters.
//! [`mod@frust_material::dialog`]'s own module docs record the icon slot as a
//! deliberate v1 scope cut, and its centered variant composes its panel with
//! both dividers off (the divider pair is reachable only through the
//! selection variant's own fixed layout, not as a builder). This page
//! therefore shows the supported panel and omits all three controls rather
//! than inventing builders the catalog has no seam for.
//!
//! # Every knob is an [`RwSignal`], because this page's body is a nav page
//!
//! A [`frust::navigator`] captures its **root page builder once**, at build,
//! and re-runs *that* closure against live state on every later rebuild
//! (`frust_widgets::nav`'s reconcile loop) — it is never replaced by the
//! closure a later `Component::build` hands it. Values cloned into the
//! closure would therefore freeze at their first-frame reading, so the knobs
//! are signal handles the closure re-reads instead: the sanctioned
//! "something outside the component's own `build` observes this write" case
//! in `docs/CODE_STANDARDS.md`'s State & Reactivity conventions. The reads
//! all happen inside the rebuild pass, so the shell's own `TrackedScope`
//! subscribes to them exactly as it would to a read in `build`.
//!
//! # The selection dialog is controlled, so this page shows its round trip
//!
//! Upstream's `M3EDialog.showSelectionScreen` *returns* the picked options as
//! a `Future<List<String>?>`. [`frust_material::selection_dialog`] is a
//! controlled component instead (that module's own docs): the caller threads
//! `.selected(..)` in and reacts to `.on_toggle(..)`. This page owns that set
//! and prints it under the triggers — the only way a live playground can show
//! a round trip whose result never comes back through the pop.
//!
//! # Why `page`/`Component::build` aren't tested directly
//!
//! `Component::build` mounts a [`frust::navigator`], which auto-wires back
//! handling against the process's running reactive runtime — the same reason
//! [`super::bottom_sheet`] splits its body out. [`content`] below is the part
//! that varies with this page's knob state, built with no navigator touched,
//! and is what this file's tests exercise instead.

use frust::{
    AnyView, Column, Component, CrossAxisAlignment, EdgeInsets, Get, GetUntracked,
    NavigatorController, Padding, PopResult, Row, RwSignal, Set, SizedBox, any, component,
    navigator, text,
};
use frust_material::{
    MaterialSpacing, ModalDismiss, dialog, filled_button, full_screen_dialog, selection_dialog,
    show_dialog, show_full_screen_dialog, show_selection_dialog, text_button, tonal_button,
};

use crate::AppState;
use crate::catalog::DemoEntry;
use crate::widgets::playground::{
    PlaySnippet, ambient_theme, control_panel, play_preview_card, play_snippet, play_switch,
    play_text_field, playground_body,
};

/// Default dialog title — the reference's own `_title` seed.
const DEFAULT_TITLE: &str = "Reset settings?";
/// Default dialog supporting text — the reference's own `_content` seed.
const DEFAULT_CONTENT: &str = "This will restore all settings to their default values.";
/// The selection dialog's options — the reference's own literal list.
const OPTIONS: [&str; 4] = ["Standard", "Pro", "Team", "Enterprise"];

/// This page's knob state, plus the navigator every `show_*` route pushes
/// onto. Every field is a live handle — see the module docs.
#[derive(Clone)]
struct Knobs {
    nav: NavigatorController<Knobs>,
    title: RwSignal<String>,
    content: RwSignal<String>,
    barrier_dismissible: RwSignal<bool>,
    multi_select: RwSignal<bool>,
    /// The selection dialog's controlled selection — see the module docs.
    selected: RwSignal<Vec<String>>,
}

impl Default for Knobs {
    /// The reference's own `_DialogsPlaygroundState` field initializers.
    fn default() -> Self {
        Self {
            nav: NavigatorController::new(),
            title: RwSignal::new(DEFAULT_TITLE.to_string()),
            content: RwSignal::new(DEFAULT_CONTENT.to_string()),
            barrier_dismissible: RwSignal::new(true),
            multi_select: RwSignal::new(false),
            selected: RwSignal::new(Vec::new()),
        }
    }
}

struct DialogsPlayground;

impl Component for DialogsPlayground {
    type State = Knobs;

    fn init(&self) -> Knobs {
        Knobs::default()
    }

    fn build(&self, state: &mut Knobs) -> AnyView<Knobs> {
        let snapshot = state.clone();
        any(navigator(&state.nav, move || content(&snapshot)))
    }
}

/// See the page contract in [`crate::pages::playground`].
pub fn page(_entry: DemoEntry) -> AnyView<AppState> {
    any(component(DialogsPlayground))
}

/// The options `current` becomes when `option` is tapped — a single-select
/// tap replaces the selection, a multi-select tap flips that option's
/// membership (order-preserving), mirroring
/// [`frust_material::selection_dialog`]'s controlled `on_toggle` contract.
fn next_selection(current: &[String], option: &str, multi_select: bool) -> Vec<String> {
    if !multi_select {
        return vec![option.to_string()];
    }
    if current.iter().any(|value| value == option) {
        return current
            .iter()
            .filter(|value| value.as_str() != option)
            .cloned()
            .collect();
    }
    let mut next = current.to_vec();
    next.push(option.to_string());
    next
}

/// Open the basic centered dialog — the reference's `_showBasic`. Like the
/// reference's own `onPressed`, it reads each knob once, at press time.
fn open_basic(state: &Knobs) {
    let title = state.title.get_untracked();
    let body = state.content.get_untracked();
    let dismissable = state.barrier_dismissible.get_untracked();
    // Minted once, outside the page builder below — which re-runs on every
    // navigator rebuild.
    let dismiss = ModalDismiss::new();
    show_dialog(
        &state.nav,
        move || {
            let cancel = dismiss.clone();
            let confirm = dismiss.clone();
            dialog()
                .title(title.clone())
                .body(body.clone())
                .dismissable(dismissable)
                .dismiss_handle(dismiss.clone())
                .actions(vec![
                    any(text_button("Cancel", move |_: &mut Knobs| cancel.dismiss())),
                    any(filled_button("Confirm", move |_: &mut Knobs| {
                        confirm.dismiss()
                    })),
                ])
        },
        |_state: &mut Knobs, _result: PopResult| {},
    );
}

/// Open the option-list dialog — the reference's `_showSelection`.
fn open_selection(state: &Knobs) {
    let title = state.title.get_untracked();
    let dismissable = state.barrier_dismissible.get_untracked();
    let multi_select = state.multi_select.get_untracked();
    let selected = state.selected;
    let dismiss = ModalDismiss::new();
    show_selection_dialog(
        &state.nav,
        move || {
            let confirm = dismiss.clone();
            selection_dialog(
                title.clone(),
                OPTIONS.iter().map(|o| (*o).to_string()).collect(),
            )
            .multi_select(multi_select)
            .dismissable(dismissable)
            .selected(selected.get())
            .confirm_label(if multi_select { "Done" } else { "OK" })
            .on_toggle(move |state: &mut Knobs, option: String| {
                let next = next_selection(&state.selected.get_untracked(), &option, multi_select);
                state.selected.set(next);
            })
            .on_confirm(move |_: &mut Knobs| confirm.dismiss())
            .dismiss_handle(dismiss.clone())
        },
        |_state: &mut Knobs, _result: PopResult| {},
    );
}

/// Open the edge-to-edge dialog — the reference's `_showFullScreen`.
fn open_full_screen(state: &Knobs) {
    let title = state.title.get_untracked();
    let body = state.content.get_untracked();
    let dismiss = ModalDismiss::new();
    show_full_screen_dialog(
        &state.nav,
        move || {
            let save = dismiss.clone();
            full_screen_dialog(
                title.clone(),
                Padding(
                    EdgeInsets::all(MaterialSpacing::XL),
                    body_text::<Knobs>(&body),
                ),
            )
            .action(any(text_button("Save", move |_: &mut Knobs| {
                save.dismiss()
            })))
            .dismiss_handle(dismiss.clone())
        },
        |_state: &mut Knobs, _result: PopResult| {},
    );
}

/// A body paragraph in the ambient theme's `bodyLarge`/`onSurface` — the
/// same shape [`super::bottom_sheet`]'s sheet content takes (a bare
/// [`frust::text`] would keep its unthemed default ink inside a panel).
fn body_text<State: 'static>(body: &str) -> AnyView<State> {
    let theme = ambient_theme();
    let mut style = theme.type_scale.body_large.clone();
    style.color = theme.scheme().on_surface;
    any(text(body.to_string()).style(style))
}

/// The playground body: the three triggers, the three snippets, and the
/// controls — everything that varies with this page's knobs, built without
/// touching the navigator (see the module docs).
fn content(state: &Knobs) -> AnyView<Knobs> {
    playground_body(
        vec![triggers_preview(state)],
        vec![
            dialog_snippet(state),
            selection_snippet(state),
            full_screen_snippet(state),
        ],
        vec![controls(state)],
    )
}

/// The "Triggers" card: the reference's three `Wrap`ped buttons (this catalog
/// has no reflowing `Wrap`, so a plain `Row` with interleaved spacers stands
/// in, the same substitution [`super::cards`] documents), plus the live
/// selection caption the controlled selection dialog needs (module docs).
fn triggers_preview(state: &Knobs) -> AnyView<Knobs> {
    let theme = ambient_theme();
    let mut caption = theme.type_scale.body_medium.clone();
    caption.color = theme.scheme().on_surface_variant;
    let selected = state.selected.get();
    let summary = if selected.is_empty() {
        "Selection: (none picked yet)".to_string()
    } else {
        format!("Selection: {}", selected.join(", "))
    };

    any(play_preview_card(
        "Triggers",
        Column(vec![
            any(Row(vec![
                any(tonal_button("Dialog", |state: &mut Knobs| {
                    open_basic(state)
                })),
                any(SizedBox::<Knobs>(Some(MaterialSpacing::SM), None)),
                any(tonal_button("Selection", |state: &mut Knobs| {
                    open_selection(state)
                })),
                any(SizedBox::<Knobs>(Some(MaterialSpacing::SM), None)),
                any(tonal_button("Full screen", |state: &mut Knobs| {
                    open_full_screen(state)
                })),
            ])),
            any(SizedBox::<Knobs>(None, Some(MaterialSpacing::MD))),
            any(text(summary).style(caption)),
        ])
        .cross_axis(CrossAxisAlignment::Start),
    ))
}

fn controls(state: &Knobs) -> AnyView<Knobs> {
    any(control_panel::<Knobs>(
        "Content",
        vec![
            play_text_field::<Knobs>(
                "Title",
                state.title.get(),
                |state: &mut Knobs, next: String| state.title.set(next),
            ),
            play_text_field::<Knobs>(
                "Content",
                state.content.get(),
                |state: &mut Knobs, next: String| state.content.set(next),
            ),
            play_switch::<Knobs>(
                "Barrier dismissible",
                state.barrier_dismissible.get(),
                |state: &mut Knobs, next: bool| state.barrier_dismissible.set(next),
            ),
            play_switch::<Knobs>(
                "Multi select (selection)",
                state.multi_select.get(),
                |state: &mut Knobs, next: bool| state.multi_select.set(next),
            ),
        ],
    ))
}

fn dialog_snippet(state: &Knobs) -> PlaySnippet {
    play_snippet(
        "Dialog",
        format!(
            "let dismiss = ModalDismiss::new();\n\
             show_dialog(\n\
             \u{20}   &state.nav,\n\
             \u{20}   move || dialog()\n\
             \u{20}       .title({title:?})\n\
             \u{20}       .body({body:?})\n\
             \u{20}       .dismissable({dismissable})\n\
             \u{20}       .dismiss_handle(dismiss.clone())\n\
             \u{20}       .actions(vec![\n\
             \u{20}           any(text_button(\"Cancel\", move |_| cancel.dismiss())),\n\
             \u{20}           any(filled_button(\"Confirm\", move |_| confirm.dismiss())),\n\
             \u{20}       ]),\n\
             \u{20}   |_state, _result| {{}},\n\
             );",
            title = state.title.get(),
            body = state.content.get(),
            dismissable = state.barrier_dismissible.get(),
        ),
    )
}

fn selection_snippet(state: &Knobs) -> PlaySnippet {
    let multi_select = state.multi_select.get();
    let confirm = if multi_select { "Done" } else { "OK" };
    play_snippet(
        "Selection",
        format!(
            "let dismiss = ModalDismiss::new();\n\
             show_selection_dialog(\n\
             \u{20}   &state.nav,\n\
             \u{20}   move || selection_dialog({title:?}, options())\n\
             \u{20}       .multi_select({multi})\n\
             \u{20}       .dismissable({dismissable})\n\
             \u{20}       .selected(selected.get())\n\
             \u{20}       .confirm_label({confirm:?})\n\
             \u{20}       .on_toggle(|state, option| state.selected.set(next(option)))\n\
             \u{20}       .on_confirm(move |_| confirm.dismiss())\n\
             \u{20}       .dismiss_handle(dismiss.clone()),\n\
             \u{20}   |_state, _result| {{}},\n\
             );",
            title = state.title.get(),
            multi = multi_select,
            dismissable = state.barrier_dismissible.get(),
        ),
    )
}

fn full_screen_snippet(state: &Knobs) -> PlaySnippet {
    play_snippet(
        "Full screen",
        format!(
            "let dismiss = ModalDismiss::new();\n\
             show_full_screen_dialog(\n\
             \u{20}   &state.nav,\n\
             \u{20}   move || full_screen_dialog(\n\
             \u{20}       {title:?},\n\
             \u{20}       Padding(EdgeInsets::all(24.0), text({body:?})),\n\
             \u{20}   )\n\
             \u{20}   .action(any(text_button(\"Save\", move |_| save.dismiss())))\n\
             \u{20}   .dismiss_handle(dismiss.clone()),\n\
             \u{20}   |_state, _result| {{}},\n\
             );",
            title = state.title.get(),
            body = state.content.get(),
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_builds_across_every_reachable_control_state() {
        let knobs = Knobs::default();
        for barrier_dismissible in [true, false] {
            for multi_select in [true, false] {
                knobs.barrier_dismissible.set(barrier_dismissible);
                knobs.multi_select.set(multi_select);
                let _view = content(&knobs);
            }
        }
        knobs.title.set(String::new());
        knobs.content.set(String::new());
        let _view = content(&knobs);
        knobs
            .selected
            .set(vec!["Pro".to_string(), "Team".to_string()]);
        let _view = content(&knobs);
    }

    #[test]
    fn a_single_select_tap_replaces_the_selection() {
        let current = vec!["Standard".to_string()];
        assert_eq!(next_selection(&current, "Pro", false), vec!["Pro"]);
    }

    #[test]
    fn a_multi_select_tap_flips_membership_in_order() {
        let current = vec!["Standard".to_string(), "Pro".to_string()];
        assert_eq!(
            next_selection(&current, "Team", true),
            vec!["Standard", "Pro", "Team"]
        );
        assert_eq!(next_selection(&current, "Standard", true), vec!["Pro"]);
    }

    #[test]
    fn the_selection_snippet_tracks_the_confirm_label() {
        let knobs = Knobs::default();
        assert!(selection_snippet(&knobs).code.contains("\"OK\""));
        knobs.multi_select.set(true);
        assert!(selection_snippet(&knobs).code.contains("\"Done\""));
    }
}
