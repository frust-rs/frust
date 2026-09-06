//! `Base`-design cases — the framework's own baseline widgets, with no
//! design-system plugin involved (see [`crate::case::Design::Base`]).
//!
//! Starts EMPTY apart from two seed cases (`button`, `container`) so g1-02
//! (the widget-snapshots binary) has something to render; g1-03/04/05 fill in
//! the rest of the website's Base page set.

use frust_core::{AnyView, any};
use frust_widgets::{button, container, text};
use peniko::Color;

use crate::case::{Case, Design};

fn button_case() -> AnyView<()> {
    any(button("Click me", |_state: &mut ()| {}))
}

fn container_case() -> AnyView<()> {
    any(container(text("Hello, Frust"))
        .fill(Color::from_rgb8(0x3B, 0x82, 0xF6))
        .radius(8.0))
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
        slug: "container",
        title: "Container",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Base,
        build: container_case,
    },
];
