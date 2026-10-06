//! Selection: the reference's `SelectionPlayground`.
//!
//! Three knobs on a playground body whose one preview pushes the reference's
//! own `_SelectionDemoHost` — a full page with a
//! [`frust_material::selection_app_bar`] over eight selectable rows, driven
//! entirely through [`frust_material::selection`]'s namespaced controller
//! functions (`is_selected`/`toggle`/`select_all`/`cleared`), which is where
//! that module puts the selection set: in the app's own state, never in the
//! widget.
//!
//! # Every knob is an [`RwSignal`], because this page's body is a nav page
//!
//! A [`frust::navigator`] captures its **root page builder once**, at build,
//! and re-runs *that* closure against live state on every later rebuild
//! (`frust_widgets::nav`'s reconcile loop) — it is never replaced by the
//! closure a later `Component::build` hands it. Values cloned into the
//! closure would therefore freeze at their first-frame reading, so the three
//! knobs are signal handles the closure re-reads instead: the sanctioned
//! "something outside the component's own `build` observes this write" case
//! in `docs/CODE_STANDARDS.md`'s State & Reactivity conventions. The reads
//! all happen inside the rebuild pass, so the shell's own `TrackedScope`
//! subscribes to them exactly as it would to a read in `build`.
//!
//! # The demo is a pushed page, so it owns its own state
//!
//! A navigator page builder is called with no access to the pushing
//! component's state (the page contract in [`crate::pages::playground`]), so
//! the demo host is its own nested [`Component`]: the three knobs are copied
//! into it at push time — exactly the reference's own
//! `_SelectionDemoHost(dismissible: .., showSelectAll: .., customHighlight:
//! ..)` constructor — while the live selection set, search query, and
//! snackbar controller are the pushed page's own retained state. Reopening
//! the demo therefore starts from a cleared selection, as it does upstream.
//!
//! The push lands on *this page's* navigator, so the demo fills the
//! playground pane under the gallery's own route chrome (a page never builds
//! route chrome — the page contract again). Its selection bar therefore reads
//! as an in-pane header rather than replacing the route's, which is also what
//! [`super::bottom_sheet`]'s page-scoped sheets do.
//!
//! # Two gesture compositions, because this catalog splits the reference's list
//!
//! Upstream's `M3EDismissibleList`/`M3ECardList.builder` each own tap,
//! long-press, **and** swipe in one list widget. Here those are two
//! independent pieces, and the nesting order is not free:
//!
//! * **Plain:** one [`frust_material::selection_host`] over all eight rows —
//!   the shape that module documents (it owns Down/Move/Up for the whole row
//!   and never forwards a pointer pass into a captured row's children).
//! * **Dismissible:** a [`frust_material::dismissible()`] wrapping a
//!   single-row host *per row*, never the reverse. The wrapper forwards every
//!   pointer pass to its child until a horizontal drag resolves (its own
//!   module docs), so tap and long-press keep working inside it; a host
//!   wrapping a `dismissible` instead would swallow the drag before the
//!   wrapper ever saw it.
//!
//! Either way the rows are [`frust_material::list_item()`]`.contained()` cards
//! rather than upstream's `M3ECardList` body: the host *is* the row stack, so
//! a card list nested inside it would be one row of it, not eight.
//!
//! # `Custom highlight` is a caller-owned surface
//!
//! [`frust_material::selection_host`] paints no chrome at all — "the item
//! view a caller supplies owns every visual", per its module docs — and
//! [`mod@frust_material::list_item`]'s own `selected` fill is the themed
//! `secondary_container` role with no color override. The knob is honored the
//! way that doc points to: a selected row swaps its themed container for a
//! caller-painted [`frust::container`] in the reference's own
//! `Color(0xFF4CAF50)`.
//!
//! # Not ported: the leading flip and the back-clears-selection arbitration
//!
//! `M3ESelectionLeading`'s 3D flip between the avatar and check faces is
//! unported upstream-side (that module's Not ported section: this framework's
//! paint surface is 2D-affine only), so the leading slot swaps views
//! instead — the substitute that doc names. Upstream's
//! `PopScope(canPop: !isSelectionMode)` has no analogue either: a page's
//! back-press policy here is [`frust::PushOptions`]'s three-way
//! pop/animated-dismiss/veto choice, with no "handle it myself, then stay"
//! rung, so the contextual bar's own close affordance is this port's clear
//! route.
//!
//! # Why `page`/`Component::build` aren't tested directly
//!
//! `Component::build` mounts a [`frust::navigator`], which auto-wires back
//! handling against the process's running reactive runtime — the same reason
//! [`super::bottom_sheet`] splits its body out. [`content`] and
//! [`demo_content`] below are the navigator-free parts, and are what this
//! file's tests exercise.

use std::collections::BTreeSet;

use frust::{
    AnyView, Color, ColorScheme, Column, Component, CrossAxisAlignment, EdgeInsets, Get,
    GetUntracked, NavigatorController, Padding, RwSignal, Set, SizedBox, View, any, column,
    component, container, icon, navigator, scroll_view, text,
};
use frust_material::{
    MaterialDimensions, MaterialSpacing, MaterialTokens, SnackbarController, dismiss_background,
    dismissible, icon_button, icons, list_item, search_app_bar, search_bar, selection,
    selection_app_bar, selection_host, snackbar, snackbar_host, tonal_button,
};

use crate::AppState;
use crate::catalog::DemoEntry;
use crate::widgets::playground::{
    PlaySnippet, ambient_theme, control_panel, play_preview_card, play_snippet, play_switch,
    playground_body,
};

/// The demo host's rows — the reference's own `_items` list.
const ITEMS: [(&str, &str); 8] = [
    ("Design review", "Expressive shapes and motion"),
    ("Release checklist", "Ship blockers and owners"),
    ("Weekly sync notes", "Decisions from Monday"),
    ("Accessibility audit", "Contrast and focus order"),
    ("Theme tokens", "Spacing and type scale"),
    ("Demo gallery", "Containment samples"),
    ("Toolbar polish", "Pill spacing and springs"),
    ("Selection patterns", "Multi-select with app bar"),
];

/// The reference's own `selectedColor: Colors.green` (its snippet spells the
/// same value `Color(0xFF4CAF50)`), used as the caller-painted highlight —
/// see the module docs.
const CUSTOM_HIGHLIGHT: Color = Color::from_rgb8(0x4C, 0xAF, 0x50);
/// Leading avatar diameter, in logical px — sized to sit inside
/// `list_item`'s own 72dp two-line row.
const AVATAR_SIZE: f64 = 40.0;
/// Gap between two adjacent rows — mirrors `frust_material`'s own
/// `CARD_LIST_GAP`, since `selection_host` stacks its rows tight.
const ROW_GAP: f64 = 4.0;
/// The idle search bar's placeholder — the reference's `barHintText`.
const SEARCH_HINT: &str = "Search items";

// ---------------------------------------------------------------------------
// The playground page
// ---------------------------------------------------------------------------

/// This page's knob state, plus the navigator the demo host is pushed onto.
/// Every knob is a live handle — see the module docs.
#[derive(Clone)]
struct Knobs {
    nav: NavigatorController<Knobs>,
    dismissible: RwSignal<bool>,
    show_select_all: RwSignal<bool>,
    custom_highlight: RwSignal<bool>,
}

impl Default for Knobs {
    /// The reference's own `_SelectionPlaygroundState` field initializers.
    fn default() -> Self {
        Self {
            nav: NavigatorController::new(),
            dismissible: RwSignal::new(false),
            show_select_all: RwSignal::new(true),
            custom_highlight: RwSignal::new(false),
        }
    }
}

struct SelectionPlayground;

impl Component for SelectionPlayground {
    type State = Knobs;

    fn init(&self) -> Knobs {
        Knobs::default()
    }

    fn build(&self, state: &mut Knobs) -> impl View<Knobs> {
        let snapshot = state.clone();
        any(navigator(&state.nav, move || content(&snapshot)))
    }
}

/// See the page contract in [`crate::pages::playground`].
pub fn page(_entry: DemoEntry) -> AnyView<AppState> {
    any(component(SelectionPlayground))
}

/// The playground body: the demo trigger, the snippet, and the controls —
/// everything that varies with this page's knobs, built without touching the
/// navigator (see the module docs).
fn content(state: &Knobs) -> AnyView<Knobs> {
    playground_body(
        vec![trigger_preview()],
        vec![snippet(state)],
        vec![controls(state)],
    )
}

/// The "Selection demo" card: the reference's explanatory paragraph over the
/// button that pushes the demo host. Knob-independent — the button reads
/// every knob off `&mut Knobs` at press time, the way the reference reads its
/// own `_dismissible`/`_showSelectAll`/`_customHighlight` inside `onPressed`.
fn trigger_preview() -> AnyView<Knobs> {
    let theme = ambient_theme();
    let mut caption = theme.type_scale.body_medium.clone();
    caption.color = theme.scheme().on_surface_variant;

    any(play_preview_card(
        "Selection demo",
        column()
            .child(
                text(
                    "Long-press a row to enter selection mode, then tap rows to \
                 add or remove them. The contextual bar's close button clears \
                 the selection.",
                )
                .style(caption),
            )
            .child(SizedBox::<Knobs>(None, Some(MaterialSpacing::MD)))
            .child(tonal_button("Open selection demo", |state: &mut Knobs| {
                open_demo(state)
            }))
            .cross_axis(CrossAxisAlignment::Start),
    ))
}

/// Push the demo host, copying the knobs into it — the reference's own
/// `Navigator.push(MaterialPageRoute(builder: _SelectionDemoHost(..)))`.
fn open_demo(state: &Knobs) {
    let nav = state.nav.clone();
    let dismissible = state.dismissible.get_untracked();
    let show_select_all = state.show_select_all.get_untracked();
    let custom_highlight = state.custom_highlight.get_untracked();
    state.nav.push(move || {
        component(SelectionDemoHost {
            nav: nav.clone(),
            dismissible,
            show_select_all,
            custom_highlight,
        })
    });
}

fn controls(state: &Knobs) -> AnyView<Knobs> {
    any(control_panel::<Knobs>(
        "Demo options",
        vec![
            play_switch::<Knobs>(
                "Dismissible list",
                state.dismissible.get(),
                |state: &mut Knobs, next: bool| state.dismissible.set(next),
            ),
            play_switch::<Knobs>(
                "Show select all",
                state.show_select_all.get(),
                |state: &mut Knobs, next: bool| state.show_select_all.set(next),
            ),
            play_switch::<Knobs>(
                "Custom highlight",
                state.custom_highlight.get(),
                |state: &mut Knobs, next: bool| state.custom_highlight.set(next),
            ),
        ],
    ))
}

fn snippet(state: &Knobs) -> PlaySnippet {
    let rows = if state.dismissible.get() {
        "// one wrapper per row, host inside (see the page docs)\n\
         dismissible(selection_host(vec![row], &state.selected, on_toggle))\n\
         \u{20}   .background(dismiss_background().icon(icons::CHECK))\n\
         \u{20}   .on_dismissed(|state, _direction| { /* remove the row */ });"
    } else {
        "selection_host(rows, &state.selected, on_toggle)\n\
         \u{20}   .on_activate(|state, index| { /* open the row */ });"
    };
    play_snippet(
        "Selection",
        format!(
            "selection_app_bar(\n\
             \u{20}   any(search_app_bar(search_bar(query, on_query).hint({hint:?}))),\n\
             \u{20}   &state.selected,\n\
             \u{20}   items.len(),\n\
             \u{20}   |state| state.selected = selection::cleared(),\n\
             )\n\
             .safe_area(false) // mid-page, below the page's own bar\n\
             .show_select_all({select_all})\n\
             .actions(vec![any(icon_button(any(icon(icons::DELETE)), |_| {{}}))])\n\
             .on_all_selected(|state, all| {{\n\
             \u{20}   state.selected = if all {{\n\
             \u{20}       selection::select_all(items.len())\n\
             \u{20}   }} else {{\n\
             \u{20}       selection::cleared()\n\
             \u{20}   }};\n\
             }});\n\
             \n\
             // body\n\
             {rows}",
            hint = SEARCH_HINT,
            select_all = state.show_select_all.get(),
        ),
    )
}

// ---------------------------------------------------------------------------
// The pushed demo host
// ---------------------------------------------------------------------------

/// The reference's `_SelectionDemoHost`: the knobs it was pushed with, plus
/// the navigator its back affordance pops.
struct SelectionDemoHost {
    nav: NavigatorController<Knobs>,
    dismissible: bool,
    show_select_all: bool,
    custom_highlight: bool,
}

/// The demo host's own retained state — the reference's
/// `_SelectionDemoHostState` (its `M3ESelectionController` restated as an
/// app-owned index set, per [`frust_material::selection`]'s controller
/// contract).
struct DemoState {
    nav: NavigatorController<Knobs>,
    selected: BTreeSet<usize>,
    query: String,
    toasts: SnackbarController<DemoState>,
}

impl Component for SelectionDemoHost {
    type State = DemoState;

    fn init(&self) -> DemoState {
        DemoState {
            nav: self.nav.clone(),
            selected: selection::cleared(),
            query: String::new(),
            toasts: SnackbarController::new(),
        }
    }

    fn build(&self, state: &mut DemoState) -> impl View<DemoState> {
        let controller = state.toasts.clone();
        any(snackbar_host(
            &controller,
            demo_content(
                state,
                self.dismissible,
                self.show_select_all,
                self.custom_highlight,
            ),
        ))
    }
}

/// The demo host's body: the contextual bar over the scrolling row list.
/// Navigator-free (the controller it holds is only ever `pop`ped from a
/// callback), so this file's tests build it directly.
fn demo_content(
    state: &DemoState,
    dismissible: bool,
    show_select_all: bool,
    custom_highlight: bool,
) -> AnyView<DemoState> {
    any(column().child(demo_bar(state, show_select_all)).flex(
        1,
        scroll_view(Padding(
            EdgeInsets {
                left: MaterialSpacing::LG,
                top: MaterialSpacing::SM,
                right: MaterialSpacing::LG,
                bottom: MaterialSpacing::XXL,
            },
            demo_list(state, dismissible, custom_highlight),
        )),
    ))
}

/// The idle search bar, wrapped in the contextual selection bar — the
/// reference's `M3ESelectionAppBar(idle: M3EAppBar.search(..), actions: ..)`.
fn demo_bar(state: &DemoState, show_select_all: bool) -> AnyView<DemoState> {
    let back_nav = state.nav.clone();
    let idle = any(search_app_bar(
        search_bar(
            state.query.clone(),
            |state: &mut DemoState, query: String| {
                state.query = query;
            },
        )
        .hint(SEARCH_HINT),
    )
    // Mid-page, below the page's own app bar: not at the window top.
    .safe_area(false)
    .leading(
        icon_button(icon(icons::ARROW_BACK), move |_: &mut DemoState| {
            back_nav.pop();
        })
        .semantic_label("Back"),
    ));

    any(selection_app_bar(
        idle,
        &state.selected,
        ITEMS.len(),
        |state: &mut DemoState| state.selected = selection::cleared(),
    )
    // Matches the idle bar so the swap stays continuous.
    .safe_area(false)
    .show_select_all(show_select_all)
    .actions(vec![
        any(icon_button(icon(icons::ARCHIVE), |_: &mut DemoState| {}).semantic_label("Archive")),
        any(icon_button(icon(icons::DELETE), |_: &mut DemoState| {}).semantic_label("Delete")),
    ])
    .on_all_selected(|state: &mut DemoState, all: bool| {
        state.selected = if all {
            selection::select_all(ITEMS.len())
        } else {
            selection::cleared()
        };
    }))
}

/// The row list, in whichever of the two compositions the `dismissible` knob
/// picks — see the module docs.
fn demo_list(state: &DemoState, dismissible: bool, custom_highlight: bool) -> AnyView<DemoState> {
    if dismissible {
        return any(Column(
            (0..ITEMS.len())
                .map(|index| dismissible_row(state, index, custom_highlight))
                .collect::<Vec<_>>(),
        )
        .cross_axis(CrossAxisAlignment::Stretch));
    }
    let rows = (0..ITEMS.len()).map(|index| row_view(state, index, custom_highlight));
    any(
        selection_host(rows, &state.selected, |state: &mut DemoState, index| {
            state.selected = selection::toggle(&state.selected, index);
        })
        .on_activate(|state: &mut DemoState, index| {
            let title = ITEMS[index].0;
            state.toasts.show(snackbar(format!("Open {title}")));
        }),
    )
}

/// One swipe-to-dismiss row: the wrapper *outside*, a single-row selection
/// host inside (the only order that keeps all three gestures — module docs).
///
/// `on_dismissed` fires at the commit, before the exit animation runs
/// ([`mod@frust_material::dismissible`]'s Exit section), so this keeps the row
/// mounted and reports the commit through the page's snackbar instead of
/// removing it — the reference's own handler is a snackbar too.
fn dismissible_row(state: &DemoState, index: usize, custom_highlight: bool) -> AnyView<DemoState> {
    let theme = ambient_theme();
    let success = theme
        .extension::<MaterialTokens>()
        .map(|tokens| tokens.colors(theme.brightness).success);
    let host = selection_host(
        vec![row_view(state, index, custom_highlight)],
        &state.selected,
        move |state: &mut DemoState, _row: usize| {
            state.selected = selection::toggle(&state.selected, index);
        },
    )
    .on_activate(move |state: &mut DemoState, _row: usize| {
        let title = ITEMS[index].0;
        state.toasts.show(snackbar(format!("Open {title}")));
    });

    // The reference's own two bands: `success` behind a rightward swipe,
    // `danger` behind a leftward one — the latter is `dismiss_background`'s
    // own unset default (`error_container`), so only the first needs a color.
    let mut forward = dismiss_background().icon(icons::CHECK);
    if let Some(success) = success {
        forward = forward.color(success);
    }
    any(dismissible(host)
        .background(forward)
        .secondary_background(dismiss_background().icon(icons::CLOSE))
        .on_dismissed(move |state: &mut DemoState, _direction| {
            let title = ITEMS[index].0;
            state.toasts.show(snackbar(format!("Dismissed {title}")));
        }))
}

/// One row: a `list_item` whose leading avatar and container surface both key
/// off its own selected state (see the module docs).
fn row_view(state: &DemoState, index: usize, custom_highlight: bool) -> AnyView<DemoState> {
    let (title, subtitle) = ITEMS[index];
    let selected = selection::is_selected(&state.selected, index);
    let row = list_item(title)
        .supporting(subtitle)
        .leading(leading_avatar(index, selected));

    let surface: AnyView<DemoState> = if custom_highlight && selected {
        any(container(row)
            .fill(CUSTOM_HIGHLIGHT)
            .radius(MaterialDimensions::RADIUS_MEDIUM))
    } else {
        any(row.contained(true).selected(selected))
    };

    any(Padding(
        EdgeInsets {
            left: 0.0,
            top: 0.0,
            right: 0.0,
            bottom: ROW_GAP,
        },
        surface,
    ))
}

/// The leading slot: the row's initial on a rotating accent circle, swapped
/// for a check on `primary` while selected — the reference's
/// `M3ESelectionLeading` faces, minus its 3D flip (module docs).
fn leading_avatar(index: usize, selected: bool) -> AnyView<DemoState> {
    let theme = ambient_theme();
    let scheme = theme.scheme();
    let radius = AVATAR_SIZE / 2.0;

    if selected {
        return any(
            container(icon(icons::CHECK).size(20.0).color(scheme.on_primary))
                .size_centered(AVATAR_SIZE, AVATAR_SIZE)
                .fill(scheme.primary)
                .radius(radius),
        );
    }

    let mut style = theme.type_scale.title_medium.clone();
    style.color = scheme.on_primary;
    let initial = ITEMS[index]
        .0
        .chars()
        .next()
        .map(String::from)
        .unwrap_or_default();
    any(container(text(initial).style(style))
        .size_centered(AVATAR_SIZE, AVATAR_SIZE)
        .fill(avatar_color(index, theme.scheme()))
        .radius(radius))
}

/// The unselected avatar's fill — the reference's own `_avatarColor`
/// (`index % 4` over primary/secondary/tertiary/error).
fn avatar_color(index: usize, scheme: &ColorScheme) -> Color {
    match index % 4 {
        0 => scheme.primary,
        1 => scheme.secondary,
        2 => scheme.tertiary,
        _ => scheme.error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demo_state(selected: impl IntoIterator<Item = usize>) -> DemoState {
        DemoState {
            nav: NavigatorController::new(),
            selected: selected.into_iter().collect(),
            query: String::new(),
            toasts: SnackbarController::new(),
        }
    }

    #[test]
    fn the_page_builds_across_every_reachable_control_state() {
        let knobs = Knobs::default();
        for dismissible in [true, false] {
            for show_select_all in [true, false] {
                for custom_highlight in [true, false] {
                    knobs.dismissible.set(dismissible);
                    knobs.show_select_all.set(show_select_all);
                    knobs.custom_highlight.set(custom_highlight);
                    let _view = content(&knobs);
                }
            }
        }
    }

    #[test]
    fn the_demo_host_builds_in_and_out_of_selection_mode() {
        for selected in [vec![], vec![0], (0..ITEMS.len()).collect::<Vec<_>>()] {
            let state = demo_state(selected);
            for dismissible in [true, false] {
                for show_select_all in [true, false] {
                    for custom_highlight in [true, false] {
                        let _view =
                            demo_content(&state, dismissible, show_select_all, custom_highlight);
                    }
                }
            }
        }
    }

    /// The set the demo's own callbacks resolve through
    /// [`frust_material::selection`] round-trips select → toggle → clear.
    #[test]
    fn the_selection_set_round_trips_through_the_controller_functions() {
        let empty = selection::cleared();
        assert!(!selection::is_selection_mode(&empty));

        let one = selection::select(&empty, 2);
        assert!(selection::is_selection_mode(&one));
        assert!(selection::is_selected(&one, 2));

        let two = selection::toggle(&one, 5);
        assert_eq!(two.len(), 2);
        assert_eq!(selection::all_selected_for(&two, ITEMS.len()), None);

        let all = selection::select_all(ITEMS.len());
        assert_eq!(selection::all_selected_for(&all, ITEMS.len()), Some(true));

        let cleared = selection::cleared();
        assert!(!selection::is_selection_mode(&cleared));
        assert_eq!(
            selection::all_selected_for(&cleared, ITEMS.len()),
            Some(false)
        );
    }

    #[test]
    fn the_snippet_tracks_the_body_and_select_all_knobs() {
        let knobs = Knobs::default();
        let plain = snippet(&knobs).code;
        assert!(plain.contains("selection_host(rows"));
        assert!(plain.contains(".show_select_all(true)"));

        knobs.dismissible.set(true);
        knobs.show_select_all.set(false);
        let swiped = snippet(&knobs).code;
        assert!(swiped.contains("dismissible(selection_host("));
        assert!(swiped.contains(".show_select_all(false)"));
    }
}
