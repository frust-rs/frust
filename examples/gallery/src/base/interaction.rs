//! `Base`-design cases for the website's `widgets/base/interaction` catalog
//! page set: `button`, `gesture-detector`, `icon-button` (g1-04). See the
//! crate docs and `crate::base` for the pure-`View`/slug-rule contract every
//! case in this registry follows.
//!
//! `button`/`container` replace the two seed cases `base::CASES` shipped with
//! from g1-01 — the conductor drops those two seed entries from
//! `base/mod.rs` at merge (see the module docs there and this crate's task
//! summary) so `button` stays a unique slug.

use frust_core::{AnyView, View, any};
use frust_widgets::{
    Axis, ButtonStyle, CrossAxisAlignment, FlexView, GestureDetector, SizedBox, button, container,
    icon_button, icons, inflexible, text,
};
use peniko::Color;

use crate::case::{Case, Design};

/// Shared accent used for the gesture-detector's visible target.
const ACCENT: Color = Color::from_rgb8(0x3B, 0x82, 0xF6);

/// Wrap `child`, centered, in a fixed 360x240 frame (`Case::DEFAULT_SIZE`) —
/// see `input.rs`'s `framed` for why this deliberately never fills the
/// backdrop itself (the recorder already clears to the active theme's
/// `surface` per `Variant`, which is what keeps a themed label legible in
/// both the light and dark recording pass).
fn framed<V: View<()>>(child: V) -> AnyView<()> {
    any(container(child).size_centered(Case::DEFAULT_SIZE.width, Case::DEFAULT_SIZE.height))
}

fn button_case() -> AnyView<()> {
    framed(
        FlexView::new(
            Axis::Vertical,
            vec![
                inflexible(
                    FlexView::new(
                        Axis::Horizontal,
                        vec![
                            inflexible(
                                button("Save", |_: &mut ()| {})
                                    .style(ButtonStyle::Primary)
                                    .small(),
                            ),
                            inflexible(SizedBox(Some(12.0), None)),
                            inflexible(
                                button("Cancel", |_: &mut ()| {})
                                    .style(ButtonStyle::Secondary)
                                    .small(),
                            ),
                            inflexible(SizedBox(Some(12.0), None)),
                            inflexible(
                                button("Skip", |_: &mut ()| {})
                                    .style(ButtonStyle::Ghost)
                                    .small(),
                            ),
                        ],
                    )
                    .cross_axis(CrossAxisAlignment::Center),
                ),
                inflexible(SizedBox(None, Some(16.0))),
                inflexible(
                    FlexView::new(
                        Axis::Horizontal,
                        vec![
                            inflexible(
                                button("Delete", |_: &mut ()| {})
                                    .style(ButtonStyle::Danger)
                                    .small(),
                            ),
                            inflexible(SizedBox(Some(12.0), None)),
                            inflexible(button("Locked", |_: &mut ()| {}).disabled(true).small()),
                            inflexible(SizedBox(Some(12.0), None)),
                            inflexible(button("Sync", |_: &mut ()| {}).loading(true).small()),
                        ],
                    )
                    .cross_axis(CrossAxisAlignment::Center),
                ),
            ],
        )
        .cross_axis(CrossAxisAlignment::Center),
    )
}

fn gesture_detector_case() -> AnyView<()> {
    framed(
        FlexView::new(
            Axis::Vertical,
            vec![
                inflexible(
                    GestureDetector(
                        container(text("Tap me").size(14.0).color(Color::WHITE))
                            .fill(ACCENT)
                            .radius(12.0)
                            .size_centered(140.0, 64.0),
                    )
                    .on_tap(|_: &mut ()| {}),
                ),
                inflexible(SizedBox(None, Some(12.0))),
                inflexible(text("wraps a target; tap or long-press to fire").size(12.0)),
            ],
        )
        .cross_axis(CrossAxisAlignment::Center),
    )
}

fn icon_button_case() -> AnyView<()> {
    framed(
        FlexView::new(
            Axis::Horizontal,
            vec![
                inflexible(icon_button(icons::CLOSE, "Close", |_: &mut ()| {})),
                inflexible(SizedBox(Some(24.0), None)),
                inflexible(icon_button(icons::SETTINGS, "Settings", |_: &mut ()| {})),
                inflexible(SizedBox(Some(24.0), None)),
                inflexible(icon_button(icons::SEND, "Send", |_: &mut ()| {}).ink(ACCENT)),
            ],
        )
        .cross_axis(CrossAxisAlignment::Center),
    )
}

/// This module's slice of the registry [`crate::cases`] concatenates.
pub const CASES: &[Case] = &[
    Case {
        slug: "button",
        title: "Button",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Base,
        build: button_case,
    },
    Case {
        slug: "gesture-detector",
        title: "GestureDetector",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Base,
        build: gesture_detector_case,
    },
    Case {
        slug: "icon-button",
        title: "Icon Button",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Base,
        build: icon_button_case,
    },
];
