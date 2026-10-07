//! Motion · Selection: the catalog's panel-opening pickers — [`select`] in
//! both variants, [`combobox`], [`multi_select`], and the
//! [`adaptive_stepper`].
//!
//! # Every picker gets its own bounded stage
//!
//! `frust_beui::overlay::anchored`'s mounting contract wants **bounded
//! constraints**: the host fills the area it is handed and treats that box as
//! the window its panel is fitted into, so it must not be handed a
//! [`frust::scroll_view`]'s unbounded axis. The gallery shell wraps every page
//! in exactly such a scroll view (`crate::nav::scroll_slot`), so each picker
//! here mounts inside its own fixed-height [`stage`]: a [`frust::Stack`] under
//! a [`frust::SizedBox`], with the trigger as the bottom child and the panel
//! host as the top one. That is the documented "top child of a full-area
//! `Stack`" mount, scoped to a demo-sized area — the panel is placed against
//! the trigger's captured rect and clipped to the stage rather than to the
//! window.
//!
//! State is a [`frust::component`]'s, as on every Motion page (see
//! `crate::pages::motion::text`). The four [`frust_beui::overlay::OverlayAnchor`]
//! cells live there too, so a trigger and its panel keep sharing one rect
//! across rebuilds.
//!
//! Sample data is upstream's own: the four-framework select, the workspace
//! combobox, the team multi-select, and the guest stepper.

use frust::{
    AnyView, Column, Component, CrossAxisAlignment, SizedBox, View, any, column, component, row,
    stack, text,
};
use frust_beui::components::adaptive_stepper::adaptive_stepper;
use frust_beui::components::combobox::{
    ComboboxOption, combobox, combobox_option, combobox_trigger,
};
use frust_beui::components::multi_select::{
    MultiSelectOption, multi_select, multi_select_option, multi_select_trigger,
};
use frust_beui::components::select::{SelectVariant, select, select_option, select_trigger};
use frust_beui::overlay::OverlayAnchor;

use crate::AppState;
use crate::nav::{caption, heading};

/// The select's options — upstream's own four frameworks.
const FRAMEWORKS: [&str; 4] = ["Next.js", "Remix", "Astro", "Vite"];

/// The combobox's options: a label plus the extra terms its subsequence filter
/// also matches on.
const WORKSPACES: [(&str, &str); 5] = [
    ("Design studio", "recent 12 projects"),
    ("Product team", "recent 8 projects"),
    ("Playground", "workspaces 24 experiments"),
    ("Marketing site", "workspaces 4 projects"),
    ("Archive", "workspaces read only"),
];

/// The multi-select's options — upstream's six teams.
const TEAMS: [&str; 6] = [
    "Design",
    "Engineering",
    "Product",
    "Research",
    "Marketing",
    "Operations",
];

/// This page's retained values, plus the anchor cell each trigger/panel pair
/// shares.
pub struct State {
    select_anchor: OverlayAnchor,
    select_open: bool,
    select_value: Option<usize>,
    morph_anchor: OverlayAnchor,
    morph_open: bool,
    morph_value: Option<usize>,
    combo_anchor: OverlayAnchor,
    combo_open: bool,
    combo_query: String,
    combo_value: Option<usize>,
    multi_anchor: OverlayAnchor,
    multi_open: bool,
    multi_query: String,
    multi_selected: Vec<usize>,
    guests: f64,
    quantity: f64,
}

impl Default for State {
    fn default() -> Self {
        Self {
            select_anchor: OverlayAnchor::new(),
            select_open: false,
            select_value: Some(0),
            morph_anchor: OverlayAnchor::new(),
            morph_open: false,
            morph_value: Some(0),
            combo_anchor: OverlayAnchor::new(),
            combo_open: false,
            combo_query: String::new(),
            combo_value: None,
            multi_anchor: OverlayAnchor::new(),
            multi_open: false,
            multi_query: String::new(),
            multi_selected: vec![0, 1],
            guests: 2.0,
            quantity: 4.0,
        }
    }
}

/// A vertical spacer.
fn gap(height: f64) -> impl View<State> {
    SizedBox(None, Some(height))
}

/// A horizontal spacer.
fn hgap(width: f64) -> impl View<State> {
    SizedBox(Some(width), None)
}

/// One component's block: its name, a one-line note (where a ported
/// degradation is stated), and the live instances.
fn demo(title: &str, note: &str, body: Vec<AnyView<State>>) -> impl View<State> {
    let mut children = vec![
        any(text(title.to_string()).size(16.0)),
        any(gap(4.0)),
        any(caption(note.to_string())),
        any(gap(12.0)),
    ];
    children.extend(body);
    children.push(any(gap(32.0)));
    Column(children).cross_axis(CrossAxisAlignment::Start)
}

/// One picker's bounded overlay stage — see the [module docs](self).
///
/// `trigger` is laid out at the stage's top-left inside a `trigger_width`-wide
/// column; `panel` is the anchored host, filling the stage and placing itself
/// against the rect the trigger captured. The stage's own `size` is what bounds
/// the host — it is deliberately wider than the trigger, so a panel that grows
/// past the trigger's width still has room, and fixed so two stages sit side by
/// side at a 900px-wide window.
fn stage(
    trigger_width: f64,
    size: (f64, f64),
    trigger: AnyView<State>,
    panel: AnyView<State>,
) -> impl View<State> {
    let (width, height) = size;
    SizedBox(Some(width), Some(height)).child(
        stack()
            .child(
                column()
                    .child(SizedBox(Some(trigger_width), None).child(trigger))
                    .cross_axis(CrossAxisAlignment::Start),
            )
            .child(panel),
    )
}

/// A labelled stage: the caption above the live picker.
fn labelled_stage(label: &str, body: AnyView<State>) -> impl View<State> {
    column()
        .child(caption(label.to_string()))
        .child(gap(8.0))
        .child(body)
        .cross_axis(CrossAxisAlignment::Start)
}

/// [`select`]'s two variants, side by side.
fn selects(state: &State) -> impl View<State> {
    let options = || {
        FRAMEWORKS
            .iter()
            .enumerate()
            .map(|(index, label)| select_option(*label).disabled(index == 3))
            .collect::<Vec<_>>()
    };
    let label_of = |value: Option<usize>| value.map(|index| FRAMEWORKS[index].to_string());

    let default = stage(
        224.0,
        (290.0, 280.0),
        any(
            select_trigger(&state.select_anchor, label_of(state.select_value))
                .placeholder("Pick a framework")
                .open(state.select_open)
                .on_open_change(|s: &mut State, open| s.select_open = open),
        ),
        any(
            select(options(), state.select_value, |s: &mut State, index| {
                s.select_value = Some(index);
                s.select_open = false;
            })
            .anchor(&state.select_anchor)
            .open(state.select_open)
            .on_open_change(|s: &mut State, open| s.select_open = open),
        ),
    );

    let morph = stage(
        224.0,
        (290.0, 280.0),
        any(
            select_trigger(&state.morph_anchor, label_of(state.morph_value))
                .variant(SelectVariant::Morph)
                .placeholder("Pick a framework")
                .open(state.morph_open)
                .on_open_change(|s: &mut State, open| s.morph_open = open),
        ),
        any(
            select(options(), state.morph_value, |s: &mut State, index| {
                s.morph_value = Some(index);
                s.morph_open = false;
            })
            .variant(SelectVariant::Morph)
            .header(label_of(state.morph_value), "Pick a framework")
            .anchor(&state.morph_anchor)
            .open(state.morph_open)
            .on_open_change(|s: &mut State, open| s.morph_open = open),
        ),
    );

    demo(
        "select",
        "Default pinches the panel off the trigger and separates; morph grows \
         the trigger's own surface into the panel. The morph is a surface \
         morph, not a shared-layout one \u{2014} the rows cross-fade against it \
         rather than travelling \u{2014} and it opens flush *below* the trigger \
         rather than over it, because the host places a panel off an anchor \
         edge. The item entrance drops its blur, the gooey separation animates \
         the near edge's two corners rather than all four, there is no \
         type-ahead, and the list height is a constant cap. Vite is disabled.",
        vec![any(row()
            .child(labelled_stage("Default", any(default)))
            .child(hgap(32.0))
            .child(labelled_stage("Morph", any(morph)))
            .cross_axis(CrossAxisAlignment::Start))],
    )
}

/// [`combobox`]: the searchable panel, filtered live.
fn comboboxes(state: &State) -> impl View<State> {
    let options: Vec<ComboboxOption> = WORKSPACES
        .iter()
        .map(|(label, keywords)| {
            combobox_option(*label).keywords(keywords.split(' ').collect::<Vec<_>>())
        })
        .collect();
    let selected_label = state
        .combo_value
        .map(|index| WORKSPACES[index].0.to_string());

    let live = stage(
        260.0,
        (340.0, 320.0),
        any(combobox_trigger(
            &state.combo_anchor,
            state.combo_query.clone(),
            |s: &mut State, q| {
                s.combo_query = q;
            },
        )
        .selected_label(selected_label)
        .placeholder("Search workspaces\u{2026}")
        .open(state.combo_open)
        .on_open_change(|s: &mut State, open| s.combo_open = open)),
        any(combobox(
            options,
            state.combo_value,
            state.combo_query.clone(),
            |s: &mut State, index| {
                s.combo_value = Some(index);
                s.combo_open = false;
                s.combo_query.clear();
            },
        )
        .anchor(&state.combo_anchor)
        .open(state.combo_open)
        .on_open_change(|s: &mut State, open| s.combo_open = open)),
    );

    demo(
        "combobox",
        "The filter is upstream's own: case-folded and a subsequence, so \"dsg\" \
         finds Design studio; a row that leaves the set collapses its height \
         where it stands. The port is a flat list \u{2014} no groups, labels or \
         separators \u{2014} there is no keyboard list navigation or type-ahead \
         (the wrapped field owns the focus path), the hover highlight cuts \
         between rows rather than travelling, and a long list clips at the \
         height cap rather than scrolling.",
        vec![any(live)],
    )
}

/// [`multi_select`]: the token field and its toggling panel.
fn multi_selects(state: &State) -> impl View<State> {
    let options: Vec<MultiSelectOption> = TEAMS.iter().map(|t| multi_select_option(*t)).collect();
    let panel_options: Vec<MultiSelectOption> =
        TEAMS.iter().map(|t| multi_select_option(*t)).collect();

    let live = stage(
        280.0,
        (360.0, 340.0),
        any(multi_select_trigger(
            &state.multi_anchor,
            options,
            state.multi_selected.clone(),
            state.multi_query.clone(),
            |s: &mut State, q| s.multi_query = q,
        )
        .placeholder("Add a team\u{2026}")
        .open(state.multi_open)
        .on_remove(|s: &mut State, index| s.multi_selected.retain(|i| *i != index))
        .on_open_change(|s: &mut State, open| s.multi_open = open)),
        any(multi_select(
            panel_options,
            state.multi_selected.clone(),
            state.multi_query.clone(),
            |s: &mut State, index| {
                if let Some(at) = s.multi_selected.iter().position(|i| *i == index) {
                    s.multi_selected.remove(at);
                } else {
                    s.multi_selected.push(index);
                }
            },
        )
        .anchor(&state.multi_anchor)
        .open(state.multi_open)
        .on_open_change(|s: &mut State, open| s.multi_open = open)),
    );

    demo(
        "multi_select",
        "Chips pop in and wipe out; the panel's rows toggle, so it stays open \
         across a change. A removed chip leaves the flow the frame it goes, \
         but its neighbours cut to their new slots instead of springing, the \
         wipe's animated edge is a straight clip rather than a rounded one, \
         and the hosted search field paints its own surface fill (the baseline \
         field offers no seam to suppress it). Backspace does not remove the \
         last chip \u{2014} that key belongs to the field's own focus path.",
        vec![any(live)],
    )
}

/// [`adaptive_stepper`]: the fixed-footprint quantity control.
fn steppers(state: &State) -> impl View<State> {
    let guests = any(
        adaptive_stepper(state.guests, |s: &mut State, v| s.guests = v)
            .min(0.0)
            .max(3.0)
            .label("Guests"),
    );

    let quantity = any(
        adaptive_stepper(state.quantity, |s: &mut State, v| s.quantity = v)
            .min(0.0)
            .max(10.0)
            .step(2.0)
            .label("Quantity"),
    );

    let disabled = any(adaptive_stepper(5.0, |_: &mut State, _| {})
        .disabled(true)
        .label("Disabled"));

    demo(
        "adaptive_stepper",
        "A quantity selector, not a step-flow container: at an end the button \
         slides out and the value pill grows over the space it left. There is \
         no liquid metaball merge (the scene has no such filter, so each pill \
         is painted on its own with the catalog hairline), the hidden button \
         and the rolling digits drop their blur, and the value change is \
         reported as a plain numeric update rather than an announced one. The \
         arrow keys step \u{2014} an addition, since one widget cannot host \
         upstream's two focusable buttons.",
        vec![any(row()
            .child(
                column()
                    .child(guests)
                    .child(gap(8.0))
                    .child(caption(format!(
                        "Guests \u{b7} 0\u{2013}3 \u{b7} {:.0}",
                        state.guests
                    )))
                    .cross_axis(CrossAxisAlignment::Start),
            )
            .child(hgap(32.0))
            .child(
                column()
                    .child(quantity)
                    .child(gap(8.0))
                    .child(caption(format!(
                        "Quantity \u{b7} 0\u{2013}10 by 2 \u{b7} {:.0}",
                        state.quantity
                    )))
                    .cross_axis(CrossAxisAlignment::Start),
            )
            .child(hgap(32.0))
            .child(
                column()
                    .child(disabled)
                    .child(gap(8.0))
                    .child(caption("Disabled"))
                    .cross_axis(CrossAxisAlignment::Start),
            )
            .cross_axis(CrossAxisAlignment::Start))],
    )
}

/// The page's interactive body.
struct SelectionPage;

impl Component for SelectionPage {
    type State = State;

    fn init(&self) -> State {
        State::default()
    }

    fn build(&self, state: &mut State) -> impl View<State> {
        any(column()
            .child(selects(state))
            .child(comboboxes(state))
            .child(multi_selects(state))
            .child(steppers(state))
            .cross_axis(CrossAxisAlignment::Start))
    }
}

pub fn page() -> impl View<AppState> {
    column()
        .child(heading("Motion \u{b7} Selection"))
        .child(SizedBox(None, Some(8.0)))
        .child(caption(
            "Pickers that open a panel: the select in both variants, the \
             searchable combobox, the token multi-select, and the adaptive \
             stepper. Each panel is hosted in its own bounded stage \u{2014} an \
             overlay host may not sit directly inside the page's scroll view.",
        ))
        .child(SizedBox(None, Some(24.0)))
        .child(component(SelectionPage))
        .cross_axis(CrossAxisAlignment::Start)
}
