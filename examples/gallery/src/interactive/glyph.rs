//! Stateful constructors for the Glyph design system's cases (`glyph/*`
//! slugs) — see the [module docs](super) for what this table is and why it
//! sits beside the registry rather than inside it.
//!
//! # Seeding
//!
//! Every constructor here seeds [`Component::init`] with the values its
//! recorded twin in [`crate::glyph`] hardcodes, so the live frame opens on the
//! poster it replaces — the same contract [`super::base`] states at length.
//! Two cases carry an affordance the poster has no state for at all
//! (`glyph/dialog` once dismissed, `glyph/badge-and-tag` once emptied); each
//! renders that affordance **only** in a state the recorded frame never shows,
//! so the seed composition is still the poster's, character for character.
//!
//! # The overlay cases are built non-dismissable, deliberately
//!
//! [`glyph_dialog`] and [`command_palette`] both expect to live under a
//! navigator: a scrim tap / `Escape` stages their exit animation and then
//! fires a **state-free** `on_close: Fn()` (`plugins/glyph/src/dialog.rs`'s
//! "Actions, and the dismiss callback is state-free" — app state rides the
//! navigator's `on_result` instead). A case in this registry has no navigator,
//! so that path has nowhere to write, and leaving `dismissable` at its `true`
//! default is actively wrong here: the widget latches its terminal
//! `Dismissed` phase, paints at alpha 0 forever, and `rebuild` does not reset
//! it — the reader is left with a blank frame that this module's state still
//! believes is open, and no way back. Both are therefore built
//! `.dismissable(false)`, which gates scrim/`Escape` only; the explicit close
//! paths (an action button, a row selection) are what carry state here.
//!
//! # Which Glyph cases are absent, and why each one is
//!
//! `glyph/term-block` and `glyph/toast` want motion rather than state, and
//! neither gains anything from a live clock: `term_block`'s per-line reveal is
//! opt-in and the case leaves `staggered` at its `false` default, so the
//! widget builds no `AnimationController` at all, and a bare toast's `paint`
//! reads no frame time whatsoever — its enter/hold/exit timeline belongs to
//! [`toast_host`](frust_glyph::toast_host), which the case does not compose.
//! Measured in a browser rather than argued: both hold byte-identical frames
//! across a live second with no input. `glyph/alert`, `glyph/app-bar` and
//! `glyph/card` are absent for the ordinary reason — they are static
//! compositions.
//!
//! `glyph/list` is absent for **neither** of those reasons, and must not be
//! read as a static composition: it is a *known, still-unfixed* second
//! instance of the exact defect the `accordion` row below exists to correct.
//! Both of its rows paint a trailing chevron straight from their own item
//! data, and the case never wires `glyph_list`'s optional `on_press` — so
//! `plugins/glyph/src/list.rs`'s `event` returns `Ignored` on
//! `on_press.is_none()` before it ever reaches phase dispatch, and a row press
//! cannot become even a no-op. It is left alone here because converting it
//! needs an app-owned selection and somewhere for the chevron to lead, which
//! is a wider change than handing a `None` callback a destination; it is
//! recorded here so a reader deciding what is left to do sees it.
//!
//! Nothing here is a `Case::build`, so none of it reaches the snapshot oracle
//! and no poster moves.

use frust_core::{AnyView, Component, any, component};
use frust_glyph::{
    BadgeVariant, PaletteItem, accordion, badge, command_palette, dots_loader, glyph_dialog,
    glyph_nav_bar, glyph_nav_item, progress, segmented_control, skeleton, tabs, tag, toggle,
};
use frust_widgets::{
    Axis, ButtonStyle, CrossAxisAlignment, FlexView, GestureDetector, SizedBox, button, inflexible,
    text,
};

use super::{Entry, framed};

/// This catalog's slice of the side table [`super::entries`] concatenates.
pub const INTERACTIVE: &[Entry] = &[
    ("glyph/accordion", accordion_case),
    ("glyph/badge-and-tag", badge_and_tag_case),
    ("glyph/command-palette", command_palette_case),
    ("glyph/dialog", dialog_case),
    ("glyph/loaders", loaders_case),
    ("glyph/nav-bar", navbar_case),
    ("glyph/tabs", tabs_case),
    ("glyph/toggle", toggle_case),
];

// ---- accordion --------------------------------------------------------------

/// Retained state for the interactive `accordion` case: one bool per panel,
/// seeded to the settled-open/settled-closed pair the recorded case pins.
struct AccordionState {
    deploy: bool,
    rollback: bool,
}

/// The `accordion` case, with a header press that actually discloses.
///
/// This is the one case in the registry whose primary affordance had no
/// handler at all — not a handler writing to `()`, as everywhere else, but no
/// `on_toggle` call in the builder chain whatsoever, so the widget's own
/// `on_toggle` stayed `None` and a header press was dropped before it could
/// even become a no-op. The visible cost was the whole of Glyph's signature
/// disclosure motion: the body is always laid out at its full natural height
/// and revealed through a `0..1` fraction of it, tweened over the theme's
/// 220ms spatial timing (`plugins/glyph/src/accordion.rs`) — a reveal that is
/// content-size independent, and that a pair of pinned `.open(..)` booleans
/// made entirely invisible.
///
/// `on_toggle` is `Fn(&mut State)` and carries no value: the widget reports
/// only *that* the header was pressed and never flips its own `open`, so the
/// handler negates the panel's own bool. Two independent bools rather than one
/// index, because these panels are independent disclosures and not a radio
/// group — both open and both closed are legal states here.
struct AccordionCase;

impl Component for AccordionCase {
    type State = AccordionState;

    fn init(&self) -> Self::State {
        AccordionState {
            deploy: true,
            rollback: false,
        }
    }

    fn build(&self, state: &mut Self::State) -> AnyView<Self::State> {
        framed(FlexView::new(
            Axis::Vertical,
            vec![
                inflexible(
                    accordion(
                        "Deploy pipeline",
                        text(
                            "Builds, tests, and ships the release artifact on every push to main.",
                        ),
                    )
                    .open(state.deploy)
                    .on_toggle(|state: &mut AccordionState| state.deploy = !state.deploy),
                ),
                inflexible(SizedBox(None, Some(12.0))),
                inflexible(
                    accordion("Rollback", text("Revert to the previous release."))
                        .open(state.rollback)
                        .on_toggle(|state: &mut AccordionState| state.rollback = !state.rollback),
                ),
            ],
        ))
    }
}

/// The [`super::Build`] the table registers: a `()`-stated `AnyView` around
/// the stateful component, legal because `ComponentView<C>` implements
/// `View<Outer>` for every `Outer`. Every constructor below repeats this
/// three-line shape.
fn accordion_case() -> AnyView<()> {
    any(component(AccordionCase))
}

// ---- badge-and-tag ----------------------------------------------------------

/// The version tag the recorded case shows, and the one the restore affordance
/// puts back — named once so the seed and the restore cannot drift.
const VERSION_TAG: &str = "v0.44.1";

/// Retained state for the interactive `badge-and-tag` case: the removable tag
/// list, seeded to the single tag the recorded case draws a `×` on. The three
/// status badges and the static `stable` tag hold no state and are rebuilt
/// verbatim.
struct TagState {
    versions: Vec<String>,
}

/// The `badge-and-tag` case, with a `×` that removes the tag it sits on.
///
/// `tag`'s `on_remove` is `Fn(&mut State)` — the widget reports the press and
/// removes nothing itself — so the list has to be app-owned; the handler
/// retains by label rather than by index, since an index into a list that is
/// about to shrink is the classic way to remove the wrong row later.
///
/// Removal is one-way in the real widget, which would leave a gallery reader
/// one press away from a frame they cannot get back. A small ghost **Restore
/// tag** button therefore takes the empty list's place. That is deliberately
/// not a divergence from the poster: the recorded frame is the one-tag state,
/// and the button exists only in the zero-tag state that frame never shows.
struct BadgeAndTagCase;

impl Component for BadgeAndTagCase {
    type State = TagState;

    fn init(&self) -> Self::State {
        TagState {
            versions: vec![VERSION_TAG.to_string()],
        }
    }

    fn build(&self, state: &mut Self::State) -> AnyView<Self::State> {
        let mut tags = vec![inflexible(tag::<TagState>("stable"))];
        for version in &state.versions {
            let label = version.clone();
            tags.push(inflexible(SizedBox(Some(8.0), None)));
            tags.push(inflexible(tag(version.clone()).on_remove(
                move |state: &mut TagState| state.versions.retain(|held| held != &label),
            )));
        }
        if state.versions.is_empty() {
            tags.push(inflexible(SizedBox(Some(8.0), None)));
            tags.push(inflexible(
                button("Restore tag", |state: &mut TagState| {
                    state.versions.push(VERSION_TAG.to_string())
                })
                .style(ButtonStyle::Ghost)
                .small(),
            ));
        }

        framed(FlexView::new(
            Axis::Vertical,
            vec![
                inflexible(
                    FlexView::new(
                        Axis::Horizontal,
                        vec![
                            inflexible(badge("connected", BadgeVariant::Success).dot(true)),
                            inflexible(SizedBox(Some(8.0), None)),
                            inflexible(badge("degraded", BadgeVariant::Warning).dot(true)),
                            inflexible(SizedBox(Some(8.0), None)),
                            inflexible(badge("offline", BadgeVariant::Error).dot(true)),
                        ],
                    )
                    .cross_axis(CrossAxisAlignment::Center),
                ),
                inflexible(SizedBox(None, Some(16.0))),
                inflexible(
                    FlexView::new(Axis::Horizontal, tags).cross_axis(CrossAxisAlignment::Center),
                ),
            ],
        ))
    }
}

fn badge_and_tag_case() -> AnyView<()> {
    any(component(BadgeAndTagCase))
}

// ---- command-palette --------------------------------------------------------

/// The palette's command catalog, `(label, hint)`. The recorded case supplies
/// an already-filtered list directly — a palette owns no fuzzy engine, and
/// filtering is the app's job (`plugins/glyph/src/command_palette.rs`) — so a
/// live query needs an unfiltered source to filter *from*. Sized and worded so
/// that the seeded query `dep` matches exactly the one row the poster shows.
///
/// The hints spell `Cmd+…` rather than `⌘`: neither bundled Glyph mono face
/// carries U+2318, so the literal symbol paints as a `.notdef` box. See the
/// recorded case's own note in [`crate::glyph`] for why a symbol-covering
/// fallback face was rejected.
const COMMANDS: &[(&str, &str)] = &[
    ("Deploy", "Cmd+D"),
    ("Rollback", "Cmd+R"),
    ("Open logs", "Cmd+L"),
    ("Restart worker", "Cmd+K"),
];

/// The rows [`COMMANDS`] contributes for `query`: a case-insensitive substring
/// match on the label, with an empty query matching everything.
fn matching(query: &str) -> Vec<PaletteItem> {
    let needle = query.trim().to_lowercase();
    COMMANDS
        .iter()
        .filter(|(label, _)| needle.is_empty() || label.to_lowercase().contains(&needle))
        .map(|(label, hint)| PaletteItem::new(*label).hint(*hint))
        .collect()
}

/// Retained state for the interactive `command-palette` case: the query
/// string, seeded to the `dep` the recorded case pins. The visible row list is
/// derived from it rather than stored, so the two cannot disagree.
struct PaletteState {
    query: String,
}

/// The `command-palette` case with a query that filters and a row that lands.
///
/// Both of the recorded case's callbacks wrote to `()`, which froze the two
/// halves of a palette at once: typing could not change the query (the
/// embedded text field is controlled and re-adopts the view's value on every
/// rebuild) and a row press did nothing. Storing the query fixes the first and
/// makes the second expressible — `on_select` carries an index into the
/// *currently supplied* rows, so the handler re-derives the same filtered list
/// to name the row that was pressed.
///
/// Selecting a row writes its label into the query. A real palette would run
/// the command and pop itself, and this case has no navigator to pop; putting
/// the chosen command in the field is the honest local stand-in — it is
/// visible, it narrows the list to the row that was pressed, and editing the
/// field undoes it.
struct CommandPaletteCase;

impl Component for CommandPaletteCase {
    type State = PaletteState;

    fn init(&self) -> Self::State {
        PaletteState {
            // Renders exactly the recorded case's single `Deploy` row.
            query: "dep".to_string(),
        }
    }

    fn build(&self, state: &mut Self::State) -> AnyView<Self::State> {
        framed(
            command_palette(
                matching(&state.query),
                |state: &mut PaletteState, query: String| state.query = query,
                |state: &mut PaletteState, index: usize| {
                    let chosen = matching(&state.query)
                        .get(index)
                        .map(|item| item.label.clone());
                    if let Some(label) = chosen {
                        state.query = label;
                    }
                },
            )
            .query(state.query.clone())
            .placeholder("Type a command or search…")
            .dismissable(false),
        )
    }
}

fn command_palette_case() -> AnyView<()> {
    any(component(CommandPaletteCase))
}

// ---- dialog -----------------------------------------------------------------

/// Retained state for the interactive `dialog` case: whether the modal is up,
/// seeded open so the live frame opens on the poster.
struct DialogState {
    open: bool,
}

/// The `dialog` case, with both actions closing it and a way back in.
///
/// The recorded case has no dismissal path of any kind: its two action buttons
/// press into `()`, and the scrim/`Escape` route cannot reach app state here
/// at all (see this module's header). Giving the actions an `open` bool to
/// clear is the whole conversion — and it needs the second half, because a
/// dismissed modal in a single-case frame would otherwise leave an empty frame
/// and no affordance at all. The closed state therefore shows the trigger the
/// dialog would have had on a real screen.
///
/// Reopening rebuilds the dialog rather than un-hiding it (the two branches
/// are different view types under one `AnyView`, so the rebuild tears the old
/// widget down), which is what replays the scale-and-fade entrance instead of
/// snapping the panel back.
struct DialogCase;

impl Component for DialogCase {
    type State = DialogState;

    fn init(&self) -> Self::State {
        DialogState { open: true }
    }

    fn build(&self, state: &mut Self::State) -> AnyView<Self::State> {
        if !state.open {
            return framed(
                button("Revoke observer-token", |state: &mut DialogState| {
                    state.open = true
                })
                .style(ButtonStyle::Danger)
                .small(),
            );
        }
        framed(
            glyph_dialog::<DialogState>()
                .title("Revoke observer-token?")
                .body("Any device using this token loses access immediately. This can't be undone.")
                .dismissable(false)
                .action(any(button("Cancel", |state: &mut DialogState| {
                    state.open = false
                })
                .style(ButtonStyle::Ghost)
                .small()))
                .action(any(button("Revoke token", |state: &mut DialogState| {
                    state.open = false
                })
                .style(ButtonStyle::Danger)
                .small())),
        )
    }
}

fn dialog_case() -> AnyView<()> {
    any(component(DialogCase))
}

// ---- loaders ----------------------------------------------------------------

/// How far one press advances the determinate ramp.
const RAMP_STEP: f64 = 0.1;

/// The ramp's next value: up by [`RAMP_STEP`], stopping *on* a full bar before
/// wrapping to empty, so both ends of the range are reachable.
fn advance_ramp(value: f64) -> f64 {
    if value >= 1.0 {
        0.0
    } else {
        (value + RAMP_STEP).min(1.0)
    }
}

/// Retained state for the interactive `loaders` case: the determinate ramp's
/// position, seeded to the value the recorded case hardcodes.
struct LoadersState {
    value: f64,
}

/// The `loaders` case, with the one part of it that was actually frozen.
///
/// Two of these three loaders were never static: the skeleton's shimmer sweep
/// and the dot cycle each advance their own `AnimationController` from
/// `PaintCtx::frame_time` and re-arm the frame on every paint
/// (`plugins/glyph/src/skeleton.rs`, `plugins/glyph/src/dots.rs`), and a
/// browser check confirms they run — the recorded poster shows them stopped
/// only because the static recorder paints once. The determinate bar was the
/// exception, and only because its `value` is a caller-owned constant.
///
/// So the bar itself is the affordance: a press on it steps the ramp, and no
/// chrome is added to say so. That is a real cost — the target is the bar's
/// own 8px-tall band and nothing advertises it — and it is the deliberate
/// trade: a caption or a slider would be content the recorded frame does not
/// have, and this case's geometry would stop matching its poster.
struct LoadersCase;

impl Component for LoadersCase {
    type State = LoadersState;

    fn init(&self) -> Self::State {
        LoadersState { value: 0.65 }
    }

    fn build(&self, state: &mut Self::State) -> AnyView<Self::State> {
        framed(
            FlexView::new(
                Axis::Vertical,
                vec![
                    inflexible(GestureDetector(progress(state.value)).on_tap(
                        |state: &mut LoadersState| state.value = advance_ramp(state.value),
                    )),
                    inflexible(SizedBox(None, Some(18.0))),
                    inflexible(skeleton(220.0, 16.0)),
                    inflexible(SizedBox(None, Some(18.0))),
                    inflexible(dots_loader()),
                ],
            )
            .cross_axis(CrossAxisAlignment::Start),
        )
    }
}

fn loaders_case() -> AnyView<()> {
    any(component(LoadersCase))
}

// ---- nav-bar ----------------------------------------------------------------

/// Retained state for the interactive `nav-bar` case: the chosen destination,
/// seeded to the index the recorded case lights.
struct NavBarState {
    selected: usize,
}

/// The `nav-bar` case as a real destination picker.
///
/// `glyph_nav_bar` is controlled — it reports the pressed index and never sets
/// its own `selected` — so the lit item, its glyph tint and its label weight
/// all follow one app-owned index. One index rather than a bool per item, for
/// the reason [`super::base`]'s radio case gives: independent bools admit the
/// none-lit and several-lit states a nav bar must not have.
struct NavBarCase;

impl Component for NavBarCase {
    type State = NavBarState;

    fn init(&self) -> Self::State {
        NavBarState { selected: 0 }
    }

    fn build(&self, state: &mut Self::State) -> AnyView<Self::State> {
        framed(glyph_nav_bar(
            vec![
                glyph_nav_item("┌", "frame"),
                glyph_nav_item("─", "stream"),
                glyph_nav_item("╳", "close"),
            ],
            state.selected,
            |state: &mut NavBarState, index: usize| state.selected = index,
        ))
    }
}

fn navbar_case() -> AnyView<()> {
    any(component(NavBarCase))
}

// ---- tabs -------------------------------------------------------------------

/// Retained state for the interactive `tabs` case: one index per control,
/// seeded to the pair the recorded case pins (tab 0, segment 1).
struct TabsState {
    tab: usize,
    segment: usize,
}

/// The `tabs` case with its two controls selecting independently.
///
/// The recorded frame stacks a tab strip pinned to `0` over a segmented
/// control pinned to `1`, which reads as one composite control in two halves.
/// Keeping the two indices separate is what shows they are not: pressing a tab
/// moves the tab strip's underline and leaves the pill where it was, and vice
/// versa. Both widgets are controlled and never move their own `selected`.
struct TabsCase;

impl Component for TabsCase {
    type State = TabsState;

    fn init(&self) -> Self::State {
        TabsState { tab: 0, segment: 1 }
    }

    fn build(&self, state: &mut Self::State) -> AnyView<Self::State> {
        framed(FlexView::new(
            Axis::Vertical,
            vec![
                inflexible(tabs(
                    vec![
                        "Overview".to_string(),
                        "Activity".to_string(),
                        "Settings".to_string(),
                    ],
                    state.tab,
                    |state: &mut TabsState, index: usize| state.tab = index,
                )),
                inflexible(SizedBox(None, Some(20.0))),
                inflexible(segmented_control(
                    vec![
                        "List".to_string(),
                        "Grid".to_string(),
                        "Compact".to_string(),
                    ],
                    state.segment,
                    |state: &mut TabsState, index: usize| state.segment = index,
                )),
            ],
        ))
    }
}

fn tabs_case() -> AnyView<()> {
    any(component(TabsCase))
}

// ---- toggle -----------------------------------------------------------------

/// Retained state for the interactive `toggle` case: one bool per switch,
/// seeded to the checked/unchecked pair the recorded case pins.
struct ToggleState {
    notifications: bool,
    auto_sync: bool,
}

/// The `toggle` case with switches that stay where they are put.
///
/// `toggle` fires `on_toggle(state, !checked)` and never flips its own
/// `checked`, so with nowhere to write the knob's press animation played and
/// then snapped straight back. Feeding the reported value into per-switch
/// state is the whole conversion; the knob slide and the track cross-fade are
/// already widget-owned and needed nothing.
struct ToggleCase;

impl Component for ToggleCase {
    type State = ToggleState;

    fn init(&self) -> Self::State {
        ToggleState {
            notifications: true,
            auto_sync: false,
        }
    }

    fn build(&self, state: &mut Self::State) -> AnyView<Self::State> {
        framed(FlexView::new(
            Axis::Vertical,
            vec![
                inflexible(
                    toggle(state.notifications, |state: &mut ToggleState, checked| {
                        state.notifications = checked;
                    })
                    .label("Notifications"),
                ),
                inflexible(SizedBox(None, Some(16.0))),
                inflexible(
                    toggle(state.auto_sync, |state: &mut ToggleState, checked| {
                        state.auto_sync = checked;
                    })
                    .label("Auto-sync"),
                ),
            ],
        ))
    }
}

fn toggle_case() -> AnyView<()> {
    any(component(ToggleCase))
}
