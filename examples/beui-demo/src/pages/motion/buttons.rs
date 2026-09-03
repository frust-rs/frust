//! Motion · Buttons: the catalog's press surfaces — [`button`]'s four
//! behaviour variants across every tone, size and state, the three
//! call-to-action buttons of the `expanding-arrow-button` slug, both
//! [`expandable_control`](frust_beui::components::expandable_control) shapes,
//! and [`action_swap`]'s four hand-over treatments.
//!
//! State lives in a [`frust::component`] rather than on `AppState` — see
//! `crate::pages::motion::text`'s module docs for why every Motion page is
//! shaped that way.
//!
//! Sample copy is upstream's own (`components/previews/motion/button-*.tsx`,
//! `hold-action-button`, `slide-action-button`, `expandable-control`,
//! `action-swap*`): "Continue", "Hover me", "Save changes", "Book a demo",
//! "Slide to continue", the Copy/Copied swap pair.

use frust::{AnyView, Column, Component, CrossAxisAlignment, Row, SizedBox, any, component, text};
use frust_beui::components::action_swap::{
    ActionSwapItem, ActionSwapSize, ActionSwapTransition, action_swap,
};
use frust_beui::components::button::{ButtonSize, ButtonState, ButtonTone, ButtonVariant, button};
use frust_beui::components::expandable_control::{expandable_button, expandable_chip};
use frust_beui::components::expanding_arrow_button::{
    HoldActionDirection, expanding_arrow_button, hold_action_button, slide_action_button,
};

use crate::AppState;
use crate::nav::{caption, heading};

/// The four `action_swap` treatments, in the enum's own order.
const SWAP_TRANSITIONS: [(ActionSwapTransition, &str); 4] = [
    (ActionSwapTransition::Fade, "Fade"),
    (ActionSwapTransition::Blur, "Blur"),
    (ActionSwapTransition::Cascade, "Cascade"),
    (ActionSwapTransition::Roll, "Roll"),
];

/// This page's retained knobs.
pub struct State {
    /// The success-path stateful button's current state.
    ok_state: ButtonState,
    /// The failure-path stateful button's current state.
    err_state: ButtonState,
    /// What the CTA row last reported, shown under it.
    cta_status: String,
    /// Whether the expandable button shows its label.
    notifications_expanded: bool,
    /// Whether the expandable chip shows its trailing action.
    chip_expanded: bool,
    /// What the chip's action last reported.
    chip_status: String,
    /// The selected item id per mounted `action_swap`: one slot per specimen,
    /// each holding an id from that specimen's own item list — the four
    /// transition specimens (`copy`/`copied`), the Sm and Lg send specimens
    /// (`send`/`sent`) and the icon specimen (`copy`/`copied`).
    swap_values: [String; 7],
}

impl Default for State {
    fn default() -> Self {
        Self {
            ok_state: ButtonState::Idle,
            err_state: ButtonState::Idle,
            cta_status: "Release early to cancel; drag the arrow to the end.".to_string(),
            notifications_expanded: false,
            chip_expanded: false,
            chip_status: "The chip's action is revealed while it is expanded.".to_string(),
            swap_values: [
                "copy".to_string(),
                "copy".to_string(),
                "copy".to_string(),
                "copy".to_string(),
                "send".to_string(),
                "send".to_string(),
                "copy".to_string(),
            ],
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

/// A row of live instances, `spacing`px apart.
fn row(items: Vec<AnyView<State>>, spacing: f64) -> AnyView<State> {
    let mut children: Vec<AnyView<State>> = Vec::with_capacity(items.len() * 2);
    for item in items {
        if !children.is_empty() {
            children.push(hgap(spacing));
        }
        children.push(item);
    }
    any(Row(children).cross_axis(CrossAxisAlignment::Center))
}

/// A labelled specimen: the live component over its caption.
fn specimen(label: &str, body: AnyView<State>) -> AnyView<State> {
    any(
        Column(vec![body, gap(6.0), any(caption(label.to_string()))])
            .cross_axis(CrossAxisAlignment::Start),
    )
}

/// The next state in the demo's own idle → loading → outcome → idle cycle.
///
/// Upstream drives this with two `setTimeout`s; a page fn has no timer, so the
/// press *is* the clock here and every stage of the swap is watchable.
fn advance(state: ButtonState, outcome: ButtonState) -> ButtonState {
    match state {
        ButtonState::Idle => ButtonState::Loading,
        ButtonState::Loading => outcome,
        _ => ButtonState::Idle,
    }
}

/// [`button`]'s two axes: the base's tones and sizes, then the three
/// behavioural variants.
fn buttons(state: &State) -> AnyView<State> {
    let tones = row(
        vec![
            any(button("Continue", |_: &mut State| {}).tone(ButtonTone::Primary)),
            any(button("Download", |_: &mut State| {}).tone(ButtonTone::Secondary)),
            any(button("Outline", |_: &mut State| {}).tone(ButtonTone::Outline)),
            any(button("Ghost", |_: &mut State| {}).tone(ButtonTone::Ghost)),
            any(button("Disabled", |_: &mut State| {}).disabled(true)),
        ],
        12.0,
    );

    let sizes = row(
        vec![
            any(button("Small", |_: &mut State| {}).size(ButtonSize::Sm)),
            any(button("Medium", |_: &mut State| {}).size(ButtonSize::Md)),
            any(button("Large", |_: &mut State| {}).size(ButtonSize::Lg)),
            any(button("\u{2715}", |_: &mut State| {})
                .size(ButtonSize::Icon)
                .tone(ButtonTone::Secondary)),
        ],
        12.0,
    );

    let metallic = row(
        vec![
            any(button("Continue", |_: &mut State| {}).variant(ButtonVariant::Metallic)),
            any(button("Generate", |_: &mut State| {})
                .variant(ButtonVariant::Metallic)
                .size(ButtonSize::Sm)),
            any(button("Paused sheen", |_: &mut State| {})
                .variant(ButtonVariant::Metallic)
                .paused(true)),
        ],
        12.0,
    );

    let magnetic = row(
        vec![
            any(button("Hover me", |_: &mut State| {})
                .variant(ButtonVariant::Magnetic)
                .strength(0.35)),
            any(button("Subtle pull", |_: &mut State| {})
                .variant(ButtonVariant::Magnetic)
                .tone(ButtonTone::Secondary)
                .strength(0.25)),
            any(button("Strong pull", |_: &mut State| {})
                .variant(ButtonVariant::Magnetic)
                .tone(ButtonTone::Outline)
                .strength(0.5)),
        ],
        16.0,
    );

    let stateful = row(
        vec![
            any(button("Save changes", |s: &mut State| {
                s.ok_state = advance(s.ok_state, ButtonState::Success);
            })
            .variant(ButtonVariant::Stateful)
            .state(state.ok_state)
            .loading_label("Saving")
            .success_label("Saved")),
            any(button("Submit", |s: &mut State| {
                s.err_state = advance(s.err_state, ButtonState::Error);
            })
            .variant(ButtonVariant::Stateful)
            .tone(ButtonTone::Secondary)
            .state(state.err_state)
            .loading_label("Submitting")
            .error_label("Failed")),
        ],
        12.0,
    );

    demo(
        "button",
        "Two axes: the behaviour variant (base, metallic, magnetic, stateful) \
         and the base's own tone. No ripple is ported, the metallic sheen does \
         not skew, and no swap blurs \u{2014} every hand-over is opacity, offset \
         and scale. The stateful button reserves its widest label instead of \
         morphing its width; press it repeatedly to walk idle \u{2192} loading \u{2192} \
         outcome \u{2192} idle (upstream uses a timer, this page uses the press).",
        vec![
            specimen(
                "Tones \u{b7} primary, secondary, outline, ghost, disabled",
                tones,
            ),
            gap(18.0),
            specimen("Sizes \u{b7} sm, md, lg, icon", sizes),
            gap(18.0),
            specimen("Metallic \u{b7} md, sm, sheen paused", metallic),
            gap(18.0),
            specimen("Magnetic \u{b7} strength 0.35, 0.25, 0.5", magnetic),
            gap(18.0),
            specimen("Stateful \u{b7} success path, error path", stateful),
        ],
    )
}

/// The three call-to-action buttons the `expanding-arrow-button` slug installs.
fn call_to_action(state: &State) -> AnyView<State> {
    let arrow = row(
        vec![
            any(expanding_arrow_button("Book a demo", |s: &mut State| {
                s.cta_status = "Demo booked.".to_string();
            })),
            any(expanding_arrow_button("Disabled", |_: &mut State| {}).disabled(true)),
        ],
        16.0,
    );

    let hold = row(
        vec![
            any(hold_action_button(
                "Hold for vertical fill",
                |s: &mut State| {
                    s.cta_status = "Hold confirmed (vertical fill).".to_string();
                },
            )),
            any(
                hold_action_button("Hold for horizontal fill", |s: &mut State| {
                    s.cta_status = "Hold confirmed (horizontal fill).".to_string();
                })
                .direction(HoldActionDirection::Horizontal)
                .holding_label("Keep holding")
                .complete_label("Done"),
            ),
        ],
        16.0,
    );

    let slide = any(slide_action_button("Slide to continue", |s: &mut State| {
        s.cta_status = "Slide completed.".to_string();
    })
    .complete_label("Ready"));

    demo(
        "expanding_arrow_button \u{b7} hold_action_button \u{b7} slide_action_button",
        "One registry slug, three CTAs. The near-black track and lime accent \
         upstream hardcodes are token pairs here (inverse surface, primary \
         container), so two hues differ; the hold button's liquid fill edge is \
         painted straight, its completion callback lands on the next event \
         pass rather than the instant the fill does, and neither the hold nor \
         the slide accepts keyboard activation.",
        vec![
            specimen("Hover or focus to expand the arrow trail", arrow),
            gap(18.0),
            specimen("Hold to the end; release early to cancel", hold),
            gap(18.0),
            specimen("Drag the thumb past 82% of the track", slide),
            gap(14.0),
            any(caption(state.cta_status.clone())),
        ],
    )
}

/// Both `expandable_control` shapes, controlled from this page's state.
fn expandables(state: &State) -> AnyView<State> {
    let controls = row(
        vec![
            any(
                expandable_button("\u{1f514}", "Notifications", |s: &mut State, open| {
                    s.notifications_expanded = open;
                })
                .expanded(state.notifications_expanded),
            ),
            any(expandable_chip("React", "\u{2715}", "Remove React")
                .expanded(state.chip_expanded)
                .on_expanded_change(|s: &mut State, open| s.chip_expanded = open)
                .on_action(|s: &mut State, _| {
                    s.chip_status = "Removed React.".to_string();
                })),
            any(
                expandable_button("\u{2699}", "Settings", |_: &mut State, _| {})
                    .expanded(true)
                    .disabled(true),
            ),
        ],
        16.0,
    );

    demo(
        "expandable_control",
        "A width morph, not a swap: the control keeps its identity while its \
         footprint changes. Both shapes are controlled (frust controls never \
         mutate the value they are handed), the morph runs on the catalog's \
         shared SPRING_LAYOUT rather than upstream's inline spring so it is a \
         touch quicker, the label reveal is opacity with the blur dropped, and \
         the icon slot takes a short string because this catalog has no icon \
         vocabulary.",
        vec![
            specimen(
                "Press to expand \u{b7} chip action \u{b7} disabled",
                controls,
            ),
            gap(14.0),
            any(caption(state.chip_status.clone())),
        ],
    )
}

/// [`action_swap`]'s four treatments, each cycling a Copy/Copied pair.
fn action_swaps(state: &State) -> AnyView<State> {
    let items = || {
        vec![
            ActionSwapItem::new("copy", "Copy link"),
            ActionSwapItem::new("copied", "Copied"),
        ]
    };

    let swaps: Vec<AnyView<State>> = SWAP_TRANSITIONS
        .iter()
        .enumerate()
        .map(|(index, (transition, label))| {
            specimen(
                label,
                any(action_swap(items())
                    .transition(*transition)
                    .value(state.swap_values[index].clone())
                    .on_change(move |s: &mut State, id: String| s.swap_values[index] = id)),
            )
        })
        .collect();

    let sizes = row(
        vec![
            any(action_swap(vec![
                ActionSwapItem::new("send", "Send"),
                ActionSwapItem::new("sent", "Sent"),
            ])
            .size(ActionSwapSize::Sm)
            .value(state.swap_values[4].clone())
            .on_change(|s: &mut State, id: String| s.swap_values[4] = id)),
            any(action_swap(vec![
                ActionSwapItem::new("send", "Send invite"),
                ActionSwapItem::new("sent", "Invite sent"),
            ])
            .size(ActionSwapSize::Lg)
            .tone(ButtonTone::Secondary)
            .value(state.swap_values[5].clone())
            .on_change(|s: &mut State, id: String| s.swap_values[5] = id)),
            any(action_swap(vec![
                ActionSwapItem::new("copy", "\u{2398}"),
                ActionSwapItem::new("copied", "\u{2713}"),
            ])
            .size(ActionSwapSize::Icon)
            .tone(ButtonTone::Outline)
            .value(state.swap_values[6].clone())
            .on_change(|s: &mut State, id: String| s.swap_values[6] = id)),
            any(action_swap(items()).disabled(true)),
        ],
        16.0,
    );

    demo(
        "action_swap",
        "One button cycling a list of actions, with four hand-over treatments. \
         No arm blurs \u{2014} upstream leans on a CSS blur in all three of its own \
         treatments, so Blur here is the fade plus its 0.94 \u{2192} 1 scale pop, and \
         Fade is the quiet crossfade upstream never names. There is no icon \
         slot (an item is an id and a label), and the button reserves its \
         widest item rather than morphing its width.",
        vec![
            row(swaps, 24.0),
            gap(18.0),
            specimen(
                "Sizes \u{b7} sm, lg (secondary), icon (outline), disabled",
                sizes,
            ),
        ],
    )
}

/// The page's interactive body.
struct ButtonsPage;

impl Component for ButtonsPage {
    type State = State;

    fn init(&self) -> State {
        State::default()
    }

    fn build(&self, state: &mut State) -> AnyView<State> {
        any(Column(vec![
            buttons(state),
            call_to_action(state),
            expandables(state),
            action_swaps(state),
        ])
        .cross_axis(CrossAxisAlignment::Start))
    }
}

pub fn page() -> AnyView<AppState> {
    any(Column(vec![
        any(heading("Motion \u{b7} Buttons")),
        any(SizedBox(None, Some(8.0))),
        any(caption(
            "Press surfaces: the four button behaviours across every tone and \
             size, the three call-to-action buttons, the two expandable \
             controls, and the four action-swap treatments.",
        )),
        any(SizedBox(None, Some(24.0))),
        any(component(ButtonsPage)),
    ])
    .cross_axis(CrossAxisAlignment::Start))
}
