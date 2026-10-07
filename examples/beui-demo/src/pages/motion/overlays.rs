//! Motion · Overlays: everything that leaves its parent's box — [`tooltip`],
//! [`popover`] in both morphs, [`context_menu`], [`morphing_modal`],
//! [`center_morph_modal`], [`drawer`] and [`bottom_sheet`].
//!
//! # Every overlay gets its own bounded stage
//!
//! Both hosting seams — `frust_beui::overlay::anchored` and
//! `frust_beui::overlay::modal` — fill the area they are handed and treat that
//! box as the window their content is fitted into, so **bounded constraints
//! are part of the mounting contract** and neither may sit directly inside a
//! [`frust::scroll_view`]. The gallery shell wraps every page in one
//! (`crate::nav::scroll_slot`), so each overlay here mounts inside its own
//! fixed-height [`stage`]: a [`frust::Stack`] under a [`frust::SizedBox`], the
//! trigger below and the host above.
//!
//! For the modal family that also makes the demo legible: a drawer, a sheet
//! and a scrim staged in a 400px box read as one panel each, rather than
//! taking the whole window over.
//!
//! State — the open flags, the anchors and the tooltip latches — lives in a
//! [`frust::component`], as on every Motion page (see
//! `crate::pages::motion::text`).

use frust::{
    AnyView, Column, Component, CrossAxisAlignment, EdgeInsets, Padding, Row, SizedBox, View, any,
    column, component, row, stack, text,
};
use frust_beui::components::bottom_sheet::bottom_sheet;
use frust_beui::components::button::{ButtonSize, ButtonTone, button};
use frust_beui::components::center_morph_modal::center_morph_modal;
use frust_beui::components::context_menu::{
    ContextMenuTone, context_menu, context_menu_item, context_menu_label, context_menu_separator,
    context_menu_trigger,
};
use frust_beui::components::drawer::{DrawerSide, drawer};
use frust_beui::components::morphing_modal::{MorphingBackdrop, MorphingPlacement, morphing_modal};
use frust_beui::components::popover::{PopoverMorph, popover};
use frust_beui::components::tooltip::{TooltipHover, tooltip, tooltip_trigger};
use frust_beui::overlay::{OverlayAnchor, OverlaySide};

use crate::AppState;
use crate::nav::{caption, heading};

/// The context menu's rows, upstream's own shape: a heading, items with
/// shortcuts, a separator, a disabled row and a destructive one.
const MENU_ROWS: [&str; 5] = ["Back", "Forward", "Reload", "Save as\u{2026}", "Delete"];

/// This page's retained flags, anchors and latches.
pub struct State {
    /// One latch per tooltip side, plus the no-arrow/no-delay pair.
    tooltips: [TooltipHover; 6],
    popover_anchor: OverlayAnchor,
    popover_open: bool,
    corner_anchor: OverlayAnchor,
    corner_open: bool,
    side_anchor: OverlayAnchor,
    side_open: bool,
    menu_anchor: OverlayAnchor,
    menu_open: bool,
    menu_status: String,
    modal_anchor: OverlayAnchor,
    modal_open: bool,
    modal_view: usize,
    modal_placement: MorphingPlacement,
    modal_backdrop: MorphingBackdrop,
    center_open: bool,
    drawer_open: bool,
    drawer_side: DrawerSide,
    sheet_open: bool,
    sheet_snap: usize,
}

impl Default for State {
    fn default() -> Self {
        Self {
            tooltips: Default::default(),
            popover_anchor: OverlayAnchor::new(),
            popover_open: false,
            corner_anchor: OverlayAnchor::new(),
            corner_open: false,
            side_anchor: OverlayAnchor::new(),
            side_open: false,
            menu_anchor: OverlayAnchor::new(),
            menu_open: false,
            menu_status: "Right-click the panel below.".to_string(),
            modal_anchor: OverlayAnchor::new(),
            modal_open: false,
            modal_view: 0,
            modal_placement: MorphingPlacement::Center,
            modal_backdrop: MorphingBackdrop::Scrim,
            center_open: false,
            drawer_open: false,
            drawer_side: DrawerSide::Right,
            sheet_open: false,
            sheet_snap: 0,
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

/// One overlay's bounded stage — see the [module docs](self).
///
/// `content` is inset by `(left, top)` inside the stage so a panel placed
/// above or beside it has room; `host` fills the stage and places itself.
fn stage(
    size: (f64, f64),
    inset: (f64, f64),
    content: AnyView<State>,
    host: AnyView<State>,
) -> impl View<State> {
    let (width, height) = size;
    let (left, top) = inset;
    SizedBox(Some(width), Some(height)).child(
        stack()
            .child(Padding(
                EdgeInsets {
                    left,
                    top,
                    right: 0.0,
                    bottom: 0.0,
                },
                column()
                    .child(content)
                    .cross_axis(CrossAxisAlignment::Start),
            ))
            .child(host),
    )
}

/// A labelled stage: the caption above the live overlay.
fn labelled(label: &str, body: AnyView<State>) -> impl View<State> {
    column()
        .child(caption(label.to_string()))
        .child(gap(8.0))
        .child(body)
        .cross_axis(CrossAxisAlignment::Start)
}

/// A small outline button — the trigger most stages here open from.
fn trigger(label: &str, on_press: impl Fn(&mut State) + 'static) -> impl View<State> {
    button(label.to_string(), on_press)
        .tone(ButtonTone::Outline)
        .size(ButtonSize::Sm)
}

/// [`tooltip`] on all four sides, plus the arrow-less and zero-delay forms.
fn tooltips(state: &State) -> impl View<State> {
    let sides = [
        (OverlaySide::Top, "side: top"),
        (OverlaySide::Right, "side: right"),
        (OverlaySide::Bottom, "side: bottom"),
        (OverlaySide::Left, "side: left"),
    ];

    let mut stages: Vec<AnyView<State>> = Vec::new();
    for (index, (side, label)) in sides.into_iter().enumerate() {
        let hover = state.tooltips[index].clone();
        if !stages.is_empty() {
            stages.push(any(hgap(16.0)));
        }
        stages.push(any(labelled(
            label,
            any(stage(
                (160.0, 130.0),
                (44.0, 44.0),
                any(tooltip_trigger(
                    &hover,
                    button("Hover me", |_: &mut State| {})
                        .tone(ButtonTone::Secondary)
                        .size(ButtonSize::Sm),
                )),
                any(tooltip::<State>(&hover, "Copy to clipboard").side(side)),
            )),
        )));
    }

    let no_arrow = state.tooltips[4].clone();
    let instant = state.tooltips[5].clone();
    let extras = any(row()
        .child(labelled(
            "arrow: off",
            any(stage(
                (160.0, 130.0),
                (44.0, 44.0),
                any(tooltip_trigger(
                    &no_arrow,
                    button("Hover me", |_: &mut State| {})
                        .tone(ButtonTone::Secondary)
                        .size(ButtonSize::Sm),
                )),
                any(tooltip::<State>(&no_arrow, "No arrow").arrow(false)),
            )),
        ))
        .child(hgap(24.0))
        .child(labelled(
            "delay: 0",
            any(stage(
                (160.0, 130.0),
                (44.0, 44.0),
                any(tooltip_trigger(
                    &instant,
                    button("Hover me", |_: &mut State| {})
                        .tone(ButtonTone::Secondary)
                        .size(ButtonSize::Sm),
                )
                .delay(std::time::Duration::ZERO)),
                any(tooltip::<State>(&instant, "Instant").side(OverlaySide::Bottom)),
            )),
        ))
        .cross_axis(CrossAxisAlignment::Start));

    demo(
        "tooltip",
        "Hover-opened, so the decision lives in a shared latch the trigger \
         writes during paint rather than in app state \u{2014} a resting pointer \
         sends no events. The staging is the host's scale and fade with no \
         blur and no 8px entrance offset, and the first press after a label \
         has appeared is swallowed by the host's light dismiss: an \
         input-transparent overlay mode would remove that, and it is a seam \
         change rather than a component one.",
        vec![
            any(Row(stages).cross_axis(CrossAxisAlignment::Start)),
            any(gap(16.0)),
            extras,
        ],
    )
}

/// [`popover`]'s two morph sources, plus a side-mounted panel.
fn popovers(state: &State) -> impl View<State> {
    let panel_content = || {
        any(column()
            .child(text("Share this project").size(14.0))
            .child(gap(6.0))
            .child(caption(
                "Anyone with the link can comment. The panel is kept mounted, \
                 so closing it plays the morph backwards.",
            ))
            .cross_axis(CrossAxisAlignment::Start))
    };

    let neck = stage(
        (240.0, 260.0),
        (0.0, 0.0),
        any(frust_beui::overlay::anchor(
            &state.popover_anchor,
            trigger("Neck morph", |s: &mut State| {
                s.popover_open = !s.popover_open
            }),
        )),
        any(popover(panel_content())
            .anchor(&state.popover_anchor)
            .morph(PopoverMorph::Neck)
            .open(state.popover_open)
            .on_open_change(|s: &mut State, open| s.popover_open = open)),
    );

    let corner = stage(
        (240.0, 260.0),
        (0.0, 0.0),
        any(frust_beui::overlay::anchor(
            &state.corner_anchor,
            trigger("Corner morph", |s: &mut State| {
                s.corner_open = !s.corner_open
            }),
        )),
        any(popover(panel_content())
            .anchor(&state.corner_anchor)
            .morph(PopoverMorph::Corner)
            .open(state.corner_open)
            .on_open_change(|s: &mut State, open| s.corner_open = open)),
    );

    let side = stage(
        (260.0, 260.0),
        (0.0, 96.0),
        any(frust_beui::overlay::anchor(
            &state.side_anchor,
            trigger("side: right", |s: &mut State| s.side_open = !s.side_open),
        )),
        any(popover(panel_content())
            .anchor(&state.side_anchor)
            .side(OverlaySide::Right)
            .open(state.side_open)
            .on_open_change(|s: &mut State, open| s.side_open = open)),
    );

    demo(
        "popover",
        "Two source rects, one mechanism: the neck travels the surface out of \
         the trigger's own rect, the corner unfolds it from the panel corner \
         nearest it. Neither melts \u{2014} upstream's neck is an SVG blur-and-\
         contrast goo pair with no PaintScene counterpart, so the surface \
         travels as a hard-edged rounded rect and passes over the trigger \
         instead of merging with it. The hover trigger mode is not ported \
         (see the tooltip for the hover opening this catalog does support).",
        vec![any(row()
            .child(labelled("Neck \u{b7} out of the trigger", any(neck)))
            .child(hgap(16.0))
            .child(labelled(
                "Corner \u{b7} out of the panel corner",
                any(corner),
            ))
            .child(hgap(16.0))
            .child(labelled("Any side is reachable", any(side)))
            .cross_axis(CrossAxisAlignment::Start))],
    )
}

/// [`context_menu`]: a right-click region and the menu that unfolds from the
/// press point.
fn context_menus(state: &State) -> impl View<State> {
    let items = vec![
        context_menu_label("PAGE"),
        context_menu_item(MENU_ROWS[0]).shortcut("\u{2318}["),
        context_menu_item(MENU_ROWS[1])
            .shortcut("\u{2318}]")
            .disabled(true),
        context_menu_item(MENU_ROWS[2]).shortcut("\u{2318}R"),
        context_menu_separator(),
        context_menu_item(MENU_ROWS[3]).shortcut("\u{2318}S"),
        context_menu_item(MENU_ROWS[4])
            .shortcut("\u{2318}\u{232b}")
            .tone(ContextMenuTone::Destructive),
    ];

    let region = any(context_menu_trigger(
        &state.menu_anchor,
        SizedBox(Some(320.0), Some(120.0)).child(Padding(
            EdgeInsets::all(16.0),
            column()
                .child(text("Right-click anywhere in this panel").size(14.0))
                .child(gap(6.0))
                .child(caption(
                    "The menu unfolds from a 16px seed square at the pointer.",
                ))
                .cross_axis(CrossAxisAlignment::Start),
        )),
    )
    .on_open_change(|s: &mut State, open| s.menu_open = open));

    let host = any(context_menu(items, |s: &mut State, index| {
        // The index is into the item vector, separators and labels
        // included — which is why the row labels are matched, not counted.
        s.menu_status = match index {
            1 => "Back".to_string(),
            3 => "Reload".to_string(),
            5 => "Save as\u{2026}".to_string(),
            6 => "Delete".to_string(),
            _ => "\u{2014}".to_string(),
        };
        s.menu_open = false;
    })
    .anchor(&state.menu_anchor)
    .open(state.menu_open)
    .on_open_change(|s: &mut State, open| s.menu_open = open));

    demo(
        "context_menu",
        "A secondary press publishes the point and the panel unfolds out of a \
         seed square there, clamped into the stage \u{2014} upstream clamps too, and \
         never flips. No submenus (upstream ships none either), and no \
         checkbox or radio rows: the catalog has no icon vocabulary to draw \
         their marks from. Arrow keys move a menu-local active row rather \
         than focus; there is no typeahead and no long-press.",
        vec![
            any(stage((360.0, 300.0), (0.0, 0.0), region, host)),
            any(gap(10.0)),
            any(caption(format!("Last activated: {}", state.menu_status))),
        ],
    )
}

/// [`morphing_modal`]: the trigger morph and upstream's own view swap.
fn morphing_modals(state: &State) -> impl View<State> {
    let view_id = if state.modal_view == 0 {
        "details"
    } else {
        "share"
    };

    let content: AnyView<State> = if state.modal_view == 0 {
        any(column()
            .child(text("Project details").size(16.0))
            .child(gap(8.0))
            .child(caption(
                "The panel's surface travelled here from the button's own rect, \
                 and returns to it on close.",
            ))
            .child(gap(16.0))
            .child(
                row()
                    .child(trigger("Share instead", |s: &mut State| s.modal_view = 1))
                    .child(hgap(8.0))
                    .child(trigger("Close", |s: &mut State| s.modal_open = false)),
            )
            .cross_axis(CrossAxisAlignment::Start))
    } else {
        any(column()
            .child(text("Share").size(16.0))
            .child(gap(8.0))
            .child(caption(
                "Changing the view id cross-fades the panel's content with an \
                 8px lift \u{2014} upstream's own morph, and taller content grows \
                 the panel.",
            ))
            .child(gap(8.0))
            .child(caption("\u{b7} anyone with the link may comment"))
            .child(gap(4.0))
            .child(caption("\u{b7} the link expires in 30 days"))
            .child(gap(16.0))
            .child(
                row()
                    .child(trigger("Back to details", |s: &mut State| s.modal_view = 0))
                    .child(hgap(8.0))
                    .child(trigger("Close", |s: &mut State| s.modal_open = false)),
            )
            .cross_axis(CrossAxisAlignment::Start))
    };

    let host = any(morphing_modal(content)
        .anchor(&state.modal_anchor)
        .placement(state.modal_placement)
        .backdrop(state.modal_backdrop)
        .view(view_id)
        .label("Project")
        .open(state.modal_open)
        .on_open_change(|s: &mut State, open| s.modal_open = open));

    let triggers = any(column()
        .child(frust_beui::overlay::anchor(
            &state.modal_anchor,
            button("Open project", |s: &mut State| {
                s.modal_open = true;
                s.modal_view = 0;
            })
            .tone(ButtonTone::Primary)
            .size(ButtonSize::Sm),
        ))
        .child(gap(10.0))
        .child(
            row()
                .child(trigger("Centre", |s: &mut State| {
                    s.modal_placement = MorphingPlacement::Center
                }))
                .child(hgap(8.0))
                .child(trigger("Bottom", |s: &mut State| {
                    s.modal_placement = MorphingPlacement::Bottom
                }))
                .child(hgap(8.0))
                .child(trigger("Scrim", |s: &mut State| {
                    s.modal_backdrop = MorphingBackdrop::Scrim
                }))
                .child(hgap(8.0))
                .child(trigger("Frosted", |s: &mut State| {
                    s.modal_backdrop = MorphingBackdrop::Frosted
                })),
        )
        .cross_axis(CrossAxisAlignment::Start));

    demo(
        "morphing_modal",
        "Two morphs in one component: the entrance travels the panel's surface \
         from the trigger's captured rect (a hero/container-transform is a \
         navigator mechanism and a modal is not a page change, so the surface \
         lerp is the route taken), and upstream's own morph \u{2014} the view swap \
         \u{2014} cross-fades the panel's content with an 8px lift, blur dropped. \
         The frosted backdrop is the wash without its blur, which is why the \
         dimming scrim is the default.",
        vec![any(stage((420.0, 400.0), (0.0, 0.0), triggers, host))],
    )
}

/// [`center_morph_modal`]: the panel that unfolds from its own centre.
fn center_modals(state: &State) -> impl View<State> {
    let content = any(column()
        .child(text("Unfolded from the centre").size(16.0))
        .child(gap(8.0))
        .child(caption(
            "A 4%-of-each-axis sliver at the panel's centre unfolds outward, \
             with the radius held constant so the last frames unfold rather \
             than round off.",
        ))
        .child(gap(16.0))
        .child(trigger("Close", |s: &mut State| s.center_open = false))
        .cross_axis(CrossAxisAlignment::Start));

    let host = any(center_morph_modal(content)
        .label("Centre morph")
        .open(state.center_open)
        .on_open_change(|s: &mut State, open| s.center_open = open));

    demo(
        "center_morph_modal",
        "The sibling morph with a different source rect \u{2014} its own centre \
         rather than a trigger. The close mark is drawn rather than an icon; \
         there is no focus trap, no focus restore and no auto-focus on open \
         (the host's barrier is what keeps interaction inside the panel), and \
         reduced motion fades instead of unfolding, which is upstream's own \
         reduced branch.",
        vec![any(stage(
            (420.0, 400.0),
            (0.0, 0.0),
            any(button("Open modal", |s: &mut State| s.center_open = true)
                .tone(ButtonTone::Primary)
                .size(ButtonSize::Sm)),
            host,
        ))],
    )
}

/// [`drawer`] on either side, and [`bottom_sheet`] with its two snap points.
fn panels(state: &State) -> impl View<State> {
    let drawer_content = any(column()
        .child(text("Filters").size(16.0))
        .child(gap(8.0))
        .child(caption(
            "The scrim, the edge mount, the slide, the barrier, Escape and the \
             backdrop press are the shared modal host's; the drawer is its \
             panel plus two upstream numbers.",
        ))
        .child(gap(16.0))
        .child(trigger("Close", |s: &mut State| s.drawer_open = false))
        .cross_axis(CrossAxisAlignment::Start));

    let drawer_host = any(drawer(drawer_content)
        .side(state.drawer_side)
        .label("Filters")
        .open(state.drawer_open)
        .on_open_change(|s: &mut State, open| s.drawer_open = open));

    let drawer_stage = stage(
        (360.0, 380.0),
        (0.0, 0.0),
        any(row()
            .child(trigger("Open left", |s: &mut State| {
                s.drawer_side = DrawerSide::Left;
                s.drawer_open = true;
            }))
            .child(hgap(8.0))
            .child(trigger("Open right", |s: &mut State| {
                s.drawer_side = DrawerSide::Right;
                s.drawer_open = true;
            }))),
        drawer_host,
    );

    let sheet_content = any(column()
        .child(caption(
            "Drag the handle row: past 120px (or a fast flick) dismisses, an \
             80px step moves between the two snap points.",
        ))
        .child(gap(12.0))
        .child(trigger("Close", |s: &mut State| s.sheet_open = false))
        .cross_axis(CrossAxisAlignment::Start));

    let sheet_host = any(bottom_sheet(sheet_content)
        .title("Trip details")
        .description("Two snap points: half height and almost full.")
        .snap(state.sheet_snap)
        .on_snap_change(|s: &mut State, index| s.sheet_snap = index)
        .label("Trip details")
        .open(state.sheet_open)
        .on_open_change(|s: &mut State, open| s.sheet_open = open));

    let sheet_stage = stage(
        (360.0, 400.0),
        (0.0, 0.0),
        any(column()
            .child(
                button("Open sheet", |s: &mut State| s.sheet_open = true)
                    .tone(ButtonTone::Primary)
                    .size(ButtonSize::Sm),
            )
            .child(gap(10.0))
            .child(caption(format!(
                "Snap point: {}",
                if state.sheet_snap == 0 {
                    "half"
                } else {
                    "almost full"
                }
            )))
            .cross_axis(CrossAxisAlignment::Start)),
        sheet_host,
    );

    demo(
        "drawer \u{b7} bottom_sheet",
        "Both are configured mounts of the shared modal host. Neither backdrop \
         blurs (the scrim is the wash alone) and shadows are the catalog's own \
         glass recipe. The sheet's drag velocity is differentiated from painted \
         frames rather than from pointer timestamps \u{2014} an InputEvent carries \
         no clock \u{2014} its whole handle row is the grip rather than the 40px \
         pill, its snap is reported and handed back (only the host can change \
         a height), and upstream's \"auto\" snap point is not modelled.",
        vec![any(row()
            .child(labelled("drawer \u{b7} left and right", any(drawer_stage)))
            .child(hgap(20.0))
            .child(labelled(
                "bottom_sheet \u{b7} two snap points",
                any(sheet_stage),
            ))
            .cross_axis(CrossAxisAlignment::Start))],
    )
}

/// The page's interactive body.
struct OverlaysPage;

impl Component for OverlaysPage {
    type State = State;

    fn init(&self) -> State {
        State::default()
    }

    fn build(&self, state: &mut State) -> impl View<State> {
        any(column()
            .child(tooltips(state))
            .child(popovers(state))
            .child(context_menus(state))
            .child(morphing_modals(state))
            .child(center_modals(state))
            .child(panels(state))
            .cross_axis(CrossAxisAlignment::Start))
    }
}

pub fn page() -> impl View<AppState> {
    column()
        .child(heading("Motion \u{b7} Overlays"))
        .child(SizedBox(None, Some(8.0)))
        .child(caption(
            "Panels that leave their parent's box: the tooltip, the popover's \
             two morphs, the context menu, both morphing modals, the drawer \
             and the bottom sheet \u{2014} each in its own bounded stage, because \
             an overlay host may not sit directly inside the page's scroll \
             view.",
        ))
        .child(SizedBox(None, Some(24.0)))
        .child(component(OverlaysPage))
        .cross_axis(CrossAxisAlignment::Start)
}
