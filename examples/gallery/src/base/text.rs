//! Base-design case for the website's `text` catalog category
//! (`apps/website/widgets/base/text/text.mdx`) — the single leaf that shapes
//! and paints a run of text.

use frust_core::{AnyView, any};
use frust_text::FontWeight;
use frust_widgets::{Column, CrossAxisAlignment, container, text};
use peniko::Color;

use crate::case::{Case, Design};

const PAGE_BG: Color = Color::from_rgb8(0x0F, 0x17, 0x2A);
const ACCENT: Color = Color::from_rgb8(0xF5, 0x9E, 0x0B);

fn text_case() -> AnyView<()> {
    let column = Column(vec![
        any(text("Hello, Frust")
            .size(28.0)
            .weight(FontWeight::SEMI_BOLD)
            .color(ACCENT)),
        any(text("the text leaf").size(15.0).color(Color::WHITE)),
    ])
    .cross_axis(CrossAxisAlignment::Center);
    any(container(column).fill(PAGE_BG).size_centered(360.0, 240.0))
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
