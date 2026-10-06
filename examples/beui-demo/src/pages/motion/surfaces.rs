//! Motion · Surfaces: the catalog's pointer- and scroll-driven surfaces —
//! [`tilt_card`], [`shared_layout_bg`], the [`scroll_animation`] family
//! (progress, parallax, reveal, scroll-to), [`pull_to_refresh`] and the
//! [`cylinder_carousel`].
//!
//! # Where "scroll progress" comes from here
//!
//! Every `scroll_animation` component is **controlled**: `ScrollView::on_scroll`
//! is the framework's sole delivery route for a scroll position, so the app
//! folds that callback into a [`frust_beui::motion::ScrollFx`] and passes the
//! resulting `0..=1` progress in as a prop. This page therefore owns a small
//! scroll surface of its own — a fixed-height [`frust::scroll_view`] over a
//! long column — and feeds its `ScrollFx` to the progress bar, the ring, the
//! parallax layer and the reveal. The gallery shell's own outer scroll view is
//! not observable from a page, and its travel would answer a different question
//! anyway (the surface's, not this section's).
//!
//! State lives in a [`frust::component`], as on every Motion page (see
//! `crate::pages::motion::text`).

use frust::Row;
use frust::{
    AnyView, Column, Component, CrossAxisAlignment, RwSignal, ScrollInfo, SizedBox, View, any,
    component, scroll_view, text,
};
use frust_beui::components::button::{ButtonSize, ButtonTone, button};
use frust_beui::components::cylinder_carousel::{CylinderCarouselVariant, cylinder_carousel};
use frust_beui::components::pull_to_refresh::pull_to_refresh;
use frust_beui::components::scroll_animation::{
    ParallaxAxis, ScrollProgressPosition, ScrollProgressVariant, parallax, scroll_progress,
    scroll_reveal, scroll_to,
};
use frust_beui::components::shared_layout_bg::{SharedLayoutBgKind, shared_layout_bg};
use frust_beui::components::tilt_card::tilt_card;
use frust_beui::motion::ScrollFx;

use crate::AppState;
use crate::nav::{caption, heading};

/// The shared-layout list's rows.
const ROWS: [(&str, &str); 5] = [
    ("Overview", "The release at a glance"),
    ("Components", "41 motion components, ported"),
    ("Blocks", "Composed product surfaces"),
    ("Playground", "Tune a value, watch it move"),
    ("Changelog", "What landed this week"),
];

/// The carousel's items.
const BALLS: [&str; 8] = [
    "Aurora", "Basalt", "Cinder", "Delta", "Ember", "Fjord", "Gale", "Halo",
];

/// This page's retained state: the scroll feeds, the flags and the knobs.
pub struct State {
    /// The demo scroll surface's folded scroll observations.
    fx: ScrollFx,
    /// The pull-to-refresh list's own scroll observations.
    pull_fx: ScrollFx,
    tilt_max: f64,
    tilt_shadow: bool,
    row_kind: SharedLayoutBgKind,
    scroll_to_offset: RwSignal<f64>,
    scroll_to_status: String,
    refreshing: bool,
    overscroll_refreshing: bool,
    carousel_index: usize,
    carousel_variant: CylinderCarouselVariant,
    carousel_auto: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            fx: ScrollFx::new(),
            pull_fx: ScrollFx::new(),
            tilt_max: 12.0,
            tilt_shadow: false,
            row_kind: SharedLayoutBgKind::Block,
            scroll_to_offset: RwSignal::new(0.0),
            scroll_to_status: "Nothing consumes the eased offset \u{2014} see the note."
                .to_string(),
            refreshing: false,
            overscroll_refreshing: false,
            carousel_index: 0,
            carousel_variant: CylinderCarouselVariant::Concave,
            carousel_auto: false,
        }
    }
}

/// A vertical spacer.
fn gap(height: f64) -> AnyView<State> {
    any(SizedBox(None, Some(height)))
}

/// A horizontal spacer.
fn hgap(width: f64) -> AnyView<State> {
    any(SizedBox(Some(width), None))
}

/// One component's block: its name, a one-line note (where a ported
/// degradation is stated), and the live instances.
fn demo(title: &str, note: &str, body: Vec<AnyView<State>>) -> AnyView<State> {
    let mut children = vec![
        any(text(title.to_string()).size(16.0)),
        gap(4.0),
        any(caption(note.to_string())),
        gap(12.0),
    ];
    children.extend(body);
    children.push(gap(32.0));
    any(Column(children).cross_axis(CrossAxisAlignment::Start))
}

/// A labelled specimen: the caption above the live component.
fn labelled(label: &str, body: AnyView<State>) -> AnyView<State> {
    any(
        Column(vec![any(caption(label.to_string())), gap(8.0), body])
            .cross_axis(CrossAxisAlignment::Start),
    )
}

/// A small outline button — this page's knobs.
fn knob(label: &str, on_press: impl Fn(&mut State) + 'static) -> AnyView<State> {
    any(button(label.to_string(), on_press)
        .tone(ButtonTone::Outline)
        .size(ButtonSize::Sm))
}

/// A card body for the tilt demo.
fn card_body(title: &str, body: &str) -> AnyView<State> {
    any(SizedBox(Some(240.0), Some(150.0)).child(
        Column(vec![
            gap(16.0),
            any(text(title.to_string()).size(16.0)),
            gap(8.0),
            any(caption(body.to_string())),
        ])
        .cross_axis(CrossAxisAlignment::Start),
    ))
}

/// [`tilt_card`]: the cursor-tracked lean and its glare.
fn tilt_cards(state: &State) -> AnyView<State> {
    let default = any(tilt_card(card_body(
        "Glare on",
        "Move the pointer over the card: the lean follows it and the glare \
             tracks the cursor.",
    ))
    .max(state.tilt_max)
    .shadow(state.tilt_shadow));

    let no_glare = any(tilt_card(card_body(
        "Glare off",
        "The same lean with the cursor-tracked highlight suppressed.",
    ))
    .max(state.tilt_max)
    .glare(false)
    .shadow(state.tilt_shadow));

    demo(
        "tilt_card",
        "The scene's only transform primitive is an affine, which cannot make a \
         trapezoid \u{2014} so the tilt is the affine shadow of upstream's two \
         rotations: an exact cos\u{3b8} foreshortening plus one shear per axis. At \
         the default 12\u{b0} the two read alike; at a large max this leans where \
         upstream rotates into depth, and .shadow() is the compensation offered \
         for it (off by default, since upstream paints none). A press keeps the \
         tilt rather than snapping flat.",
        vec![
            any(Row(vec![
                labelled("Default \u{b7} glare on", default),
                hgap(40.0),
                labelled("glare: off", no_glare),
            ])
            .cross_axis(CrossAxisAlignment::Start)),
            gap(14.0),
            any(Row(vec![
                any(caption(format!("max: {:.0}\u{b0}", state.tilt_max))),
                hgap(12.0),
                knob("8\u{b0}", |s: &mut State| s.tilt_max = 8.0),
                hgap(8.0),
                knob("12\u{b0}", |s: &mut State| s.tilt_max = 12.0),
                hgap(8.0),
                knob("24\u{b0}", |s: &mut State| s.tilt_max = 24.0),
                hgap(16.0),
                knob(
                    if state.tilt_shadow {
                        "Shadow: on"
                    } else {
                        "Shadow: off"
                    },
                    |s: &mut State| s.tilt_shadow = !s.tilt_shadow,
                ),
            ])
            .cross_axis(CrossAxisAlignment::Center)),
        ],
    )
}

/// [`shared_layout_bg`]: the pill that glides between hovered rows.
fn shared_layout(state: &State) -> AnyView<State> {
    let rows: Vec<AnyView<State>> = ROWS
        .iter()
        .map(|(title, body)| {
            any(Column(vec![
                any(text(title.to_string()).size(14.0)),
                gap(2.0),
                any(caption(body.to_string())),
            ])
            .cross_axis(CrossAxisAlignment::Start))
        })
        .collect();

    let list =
        any(SizedBox(Some(340.0), None)
            .child(shared_layout_bg(rows).kind(state.row_kind).inset(20.0)));

    demo(
        "shared_layout_bg",
        "Upstream's layoutId pill is restated as one retained rect that springs \
         from where it is toward where it now belongs \u{2014} the same glide \
         without the projection. Its enter/exit blur becomes an opacity fade, \
         which is exactly what upstream itself does under reduced motion. Enter, \
         move and exit stay three different animations, and a press keeps the \
         highlight.",
        vec![
            labelled("Hover the rows", list),
            gap(14.0),
            any(Row(vec![
                any(caption(match state.row_kind {
                    SharedLayoutBgKind::Block => "as: div (group)",
                    SharedLayoutBgKind::List => "as: ul (list)",
                })),
                hgap(12.0),
                knob("Block", |s: &mut State| {
                    s.row_kind = SharedLayoutBgKind::Block
                }),
                hgap(8.0),
                knob("List", |s: &mut State| {
                    s.row_kind = SharedLayoutBgKind::List
                }),
            ])
            .cross_axis(CrossAxisAlignment::Center)),
        ],
    )
}

/// The [`scroll_animation`] family, driven by this page's own scroll surface.
fn scroll_effects(state: &State) -> AnyView<State> {
    let progress = state.fx.progress();

    let mut rows: Vec<AnyView<State>> = vec![
        any(text("Scroll this surface").size(15.0)),
        gap(6.0),
        any(caption(
            "Its ScrollFx feeds the bar, the ring, the parallax layer and the \
             reveal below.",
        )),
        gap(12.0),
        any(parallax(
            caption("parallax \u{b7} speed 0.4, axis y \u{2014} drifts against the travel"),
            progress,
        )
        .speed(0.4)),
        gap(10.0),
        any(
            parallax(caption("parallax \u{b7} speed 0.8, axis x"), progress)
                .speed(0.8)
                .axis(ParallaxAxis::X),
        ),
        gap(16.0),
    ];
    for index in 0..10 {
        rows.push(any(caption(format!(
            "Row {:02} \u{b7} the surface's own travel is what \"progress\" means here.",
            index + 1
        ))));
        rows.push(gap(10.0));
    }
    rows.push(any(scroll_reveal(
        Column(vec![
            any(text("Revealed").size(15.0)),
            gap(6.0),
            any(caption(
                "scroll_reveal fires once past 30% of the surface's travel, \
                 rising 24px into place with its blur dropped.",
            )),
        ])
        .cross_axis(CrossAxisAlignment::Start),
        progress,
    )
    .threshold(0.3)
    .slide(24.0)));
    rows.push(gap(20.0));

    let surface = any(SizedBox(Some(420.0), Some(240.0)).child(
        scroll_view(Column(rows).cross_axis(CrossAxisAlignment::Start)).on_scroll(
            |s: &mut State, info: ScrollInfo| {
                s.fx.observe(info);
            },
        ),
    ));

    let bar = any(SizedBox(Some(420.0), Some(6.0)).child(scroll_progress::<State>(progress)));
    let bottom_bar = any(SizedBox(Some(420.0), Some(6.0))
        .child(scroll_progress::<State>(progress).position(ScrollProgressPosition::Bottom)));
    let ring = any(SizedBox(Some(56.0), Some(56.0))
        .child(scroll_progress::<State>(progress).variant(ScrollProgressVariant::Ring)));

    let jump = any(scroll_to(
        button("Ease the offset to 480", |_: &mut State| {})
            .tone(ButtonTone::Outline)
            .size(ButtonSize::Sm),
        480.0,
        state.scroll_to_offset,
    )
    .on_activate(|s: &mut State| {
        s.scroll_to_status =
            "Eased 0 \u{2192} 480 into the signal; no surface consumed it.".to_string();
    }));

    demo(
        "scroll_animation",
        "Four of the family's five files are here. SmoothScroll is deliberately \
         not ported \u{2014} frust owns its scroll physics, and a component tier \
         re-integrating the wheel would be a second, competing one. scroll_to \
         eases an offset into a signal it cannot apply, because ScrollView \
         publishes no programmatic-scroll seam: the ease is real, the page \
         movement is the missing half. Progress is the surface's travel rather \
         than one element's viewport crossing.",
        vec![any(Row(vec![
            any(Column(vec![
                labelled("scroll_progress \u{b7} bar, top", bar),
                gap(12.0),
                labelled("The surface", surface),
                gap(8.0),
                labelled("scroll_progress \u{b7} bar, bottom", bottom_bar),
            ])
            .cross_axis(CrossAxisAlignment::Start)),
            hgap(40.0),
            any(Column(vec![
                labelled("scroll_progress \u{b7} ring", ring),
                gap(16.0),
                any(caption(format!("progress: {:.0}%", progress * 100.0))),
                gap(16.0),
                labelled("scroll_to", jump),
                gap(8.0),
                any(caption(state.scroll_to_status.clone())),
            ])
            .cross_axis(CrossAxisAlignment::Start)),
        ])
        .cross_axis(CrossAxisAlignment::Start))],
    )
}

/// [`pull_to_refresh`], on both of its routes.
fn pull_to_refreshes(state: &State) -> AnyView<State> {
    let list = |title: &str| {
        let mut rows: Vec<AnyView<State>> = vec![any(text(title.to_string()).size(15.0)), gap(8.0)];
        for index in 0..8 {
            rows.push(any(caption(format!("Item {:02}", index + 1))));
            rows.push(gap(8.0));
        }
        Column(rows).cross_axis(CrossAxisAlignment::Start)
    };

    let drag_route = any(SizedBox(Some(320.0), Some(240.0)).child(
        pull_to_refresh(list("Press and drag down"), |s: &mut State| {
            s.refreshing = true;
        })
        .refreshing(state.refreshing),
    ));

    let overscroll_route = any(SizedBox(Some(320.0), Some(240.0)).child(
        pull_to_refresh(
            scroll_view(list("Overscroll past the top")).on_scroll(
                |s: &mut State, info: ScrollInfo| {
                    s.pull_fx.observe(info);
                },
            ),
            |s: &mut State| s.overscroll_refreshing = true,
        )
        .refreshing(state.overscroll_refreshing)
        .overscroll(state.pull_fx.overscroll()),
    ));

    demo(
        "pull_to_refresh",
        "frust's scroll surface is a separate widget that owns its physics and \
         cannot have a gesture taken off it, so the container accepts a pull \
         from either side: a direct drag (what a mouse exercises), or the \
         signed past-edge overscroll piped out of ScrollView::on_scroll (the \
         faithful route, and the one that feels native on touch). Upstream \
         awaits an async onRefresh; a callback here is synchronous, so the \
         refreshing flag is the app's and this page clears it by hand.",
        vec![
            any(Row(vec![
                labelled("Drag route", drag_route),
                hgap(40.0),
                labelled("Overscroll route", overscroll_route),
            ])
            .cross_axis(CrossAxisAlignment::Start)),
            gap(14.0),
            any(Row(vec![
                knob("Finish drag refresh", |s: &mut State| s.refreshing = false),
                hgap(8.0),
                knob("Finish overscroll refresh", |s: &mut State| {
                    s.overscroll_refreshing = false
                }),
            ])),
            gap(8.0),
            any(caption(format!(
                "refreshing: drag {} \u{b7} overscroll {}",
                state.refreshing, state.overscroll_refreshing
            ))),
        ],
    )
}

/// The [`cylinder_carousel`].
fn carousels(state: &State) -> AnyView<State> {
    let items: Vec<AnyView<State>> = BALLS
        .iter()
        .map(|label| {
            any(Column(vec![any(text(label.to_string()).size(15.0))])
                .cross_axis(CrossAxisAlignment::Center))
        })
        .collect();

    let stage = any(SizedBox(Some(520.0), None).child(
        cylinder_carousel(items)
            .variant(state.carousel_variant)
            .item_size(120.0)
            .visible_items(5)
            .height(180.0)
            .snap(true)
            .auto_rotate(state.carousel_auto)
            .default_index(0)
            .on_index_change(|s: &mut State, index| s.carousel_index = index),
    ));

    demo(
        "cylinder_carousel",
        "Upstream's cylinder is already a hand-rolled projection rather than a \
         CSS 3D transform, so its arithmetic ports exactly and only the \
         composite differs (a scene affine instead of a CSS scale). What is \
         lost is the velocity hand-off: the catalog's spring solves a unit \
         displacement released from rest, so a flick keeps upstream's reach but \
         restarts the roll from zero speed. Drag, wheel or arrow-key it; the \
         items are not pointer targets, the stage owns the gesture.",
        vec![
            stage,
            gap(14.0),
            any(Row(vec![
                any(caption(format!(
                    "index: {} \u{b7} {}",
                    state.carousel_index,
                    BALLS[state.carousel_index % BALLS.len()]
                ))),
                hgap(16.0),
                knob("Concave", |s: &mut State| {
                    s.carousel_variant = CylinderCarouselVariant::Concave
                }),
                hgap(8.0),
                knob("Convex", |s: &mut State| {
                    s.carousel_variant = CylinderCarouselVariant::Convex
                }),
                hgap(16.0),
                knob(
                    if state.carousel_auto {
                        "Auto-rotate: on"
                    } else {
                        "Auto-rotate: off"
                    },
                    |s: &mut State| s.carousel_auto = !s.carousel_auto,
                ),
            ])
            .cross_axis(CrossAxisAlignment::Center)),
        ],
    )
}

/// The page's interactive body.
struct SurfacesPage;

impl Component for SurfacesPage {
    type State = State;

    fn init(&self) -> State {
        State::default()
    }

    fn build(&self, state: &mut State) -> impl View<State> {
        any(Column(vec![
            tilt_cards(state),
            shared_layout(state),
            scroll_effects(state),
            pull_to_refreshes(state),
            carousels(state),
        ])
        .cross_axis(CrossAxisAlignment::Start))
    }
}

pub fn page() -> AnyView<AppState> {
    any(Column(vec![
        any(heading("Motion \u{b7} Surfaces")),
        any(SizedBox(None, Some(8.0))),
        any(caption(
            "Pointer- and scroll-driven surfaces: the tilting card, the gliding \
             row pill, the scroll-progress family over this page's own scroll \
             surface, pull-to-refresh on both of its routes, and the cylinder \
             carousel.",
        )),
        any(SizedBox(None, Some(24.0))),
        any(component(SurfacesPage)),
    ])
    .cross_axis(CrossAxisAlignment::Start))
}
