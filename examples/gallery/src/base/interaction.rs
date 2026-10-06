//! `Base`-design cases for the website's `widgets/base/interaction` catalog
//! page set: `button`, `gesture-detector`, `icon-button`. See the crate docs
//! and `crate::base` for the pure-`View`/slug-rule contract every case in
//! this registry follows, and [`super::framed`] for why none of these cases
//! fills its own backdrop.
//!
//! The `button` case here owns the `button` slug outright: the registry's
//! two original placeholder entries (a `button` and a `container` sketch)
//! were dropped from `base/mod.rs` once the real category modules landed, so
//! no second case competes for it.

use frust_core::AnyView;
use frust_widgets::{
    ButtonStyle, CrossAxisAlignment, GestureDetector, SizedBox, button, column, container,
    icon_button, icons, row, text,
};
use peniko::Color;

use super::framed;
use crate::case::{Case, Design};

/// Shared accent used for the gesture-detector's visible target.
const ACCENT: Color = Color::from_rgb8(0x3B, 0x82, 0xF6);

fn button_case() -> AnyView<()> {
    framed(
        column()
            .child(
                row()
                    .child(
                        button("Save", |_: &mut ()| {})
                            .style(ButtonStyle::Primary)
                            .small(),
                    )
                    .child(SizedBox(Some(12.0), None))
                    .child(
                        button("Cancel", |_: &mut ()| {})
                            .style(ButtonStyle::Secondary)
                            .small(),
                    )
                    .child(SizedBox(Some(12.0), None))
                    .child(
                        button("Skip", |_: &mut ()| {})
                            .style(ButtonStyle::Ghost)
                            .small(),
                    )
                    .cross_axis(CrossAxisAlignment::Center),
            )
            .child(SizedBox(None, Some(16.0)))
            .child(
                row()
                    .child(
                        button("Delete", |_: &mut ()| {})
                            .style(ButtonStyle::Danger)
                            .small(),
                    )
                    .child(SizedBox(Some(12.0), None))
                    .child(button("Locked", |_: &mut ()| {}).disabled(true).small())
                    .child(SizedBox(Some(12.0), None))
                    .child(button("Sync", |_: &mut ()| {}).loading(true).small())
                    .cross_axis(CrossAxisAlignment::Center),
            )
            .cross_axis(CrossAxisAlignment::Center),
    )
}

fn gesture_detector_case() -> AnyView<()> {
    framed(
        column()
            .child(
                GestureDetector(
                    container(text("Tap me").size(14.0).color(Color::WHITE))
                        .fill(ACCENT)
                        .radius(12.0)
                        .size_centered(140.0, 64.0),
                )
                .on_tap(|_: &mut ()| {}),
            )
            .child(SizedBox(None, Some(12.0)))
            .child(text("wraps a target; tap or long-press to fire").size(12.0))
            .cross_axis(CrossAxisAlignment::Center),
    )
}

fn icon_button_case() -> AnyView<()> {
    framed(
        row()
            .child(icon_button(icons::CLOSE, "Close", |_: &mut ()| {}))
            .child(SizedBox(Some(24.0), None))
            .child(icon_button(icons::SETTINGS, "Settings", |_: &mut ()| {}))
            .child(SizedBox(Some(24.0), None))
            .child(icon_button(icons::SEND, "Send", |_: &mut ()| {}).ink(ACCENT))
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
