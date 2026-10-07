//! Base-design case for the website's `text` catalog category
//! (`apps/website/widgets/base/text/text.mdx`) — the single leaf that shapes
//! and paints a run of text.
//!
//! The frame is [`super::framed`] (sized, never filled), so both runs sit on
//! the recorder's per-variant `surface` clear: the body line takes the
//! themed `on_surface` default, and the heading's one explicit colour is an
//! accent picked to stay legible against a light *and* a dark surface.

use frust_core::{AnyView, any};
use frust_text::FontWeight;
use frust_widgets::{CrossAxisAlignment, column, text};
use peniko::Color;

use super::framed;
use crate::case::{Case, Design};

/// The heading's explicit glyph colour: a mid-tone blue that keeps usable
/// contrast against both the light (`#FAFAFA`) and the dark (`#121212`)
/// neutral surface the recorder clears to.
const ACCENT: Color = Color::from_rgb8(0x3B, 0x82, 0xF6);

fn text_case() -> AnyView<()> {
    any(framed(
        column()
            .child(
                text("Hello, Frust")
                    .size(28.0)
                    .weight(FontWeight::SEMI_BOLD)
                    .color(ACCENT),
            )
            .child(text("the text leaf").size(15.0))
            .cross_axis(CrossAxisAlignment::Center),
    ))
}

pub(super) const TEXT: Case = Case {
    slug: "text",
    title: "text",
    size: Case::DEFAULT_SIZE,
    scale: Case::DEFAULT_SCALE,
    time_ms: Case::DEFAULT_TIME_MS,
    design: Design::Base,
    build: text_case,
};
