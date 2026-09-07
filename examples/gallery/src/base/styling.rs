//! Base-design cases for the website's `styling` catalog category
//! (`apps/website/widgets/base/styling/`): `Alignment`, `Color`,
//! `EdgeInsets`, `TextStyle`.
//!
//! These are plain value types, not widgets, so each case builds a small
//! scene that demonstrates the type's *effect* rather than the type itself —
//! `Alignment` via `Align`, `Color` via `.fill`, `EdgeInsets` via `Padding`
//! (a different shape than [`super::layout`]'s own `padding` case: an
//! asymmetric struct literal rather than a uniform inset), `TextStyle` via
//! `text(..).style(..)` (the bulk-styling knob `text.rs`'s `text` case
//! doesn't reach for). Every case forces its own full [`Case::DEFAULT_SIZE`]
//! footprint via `container(..).size_centered(..)`, for the same reason
//! [`super::basics`] does (see that module's docs).

use frust_core::{AnyView, any};
use frust_text::{FontWeight, TextStyle};
use frust_widgets::{
    Align, Alignment, Column, CrossAxisAlignment, EdgeInsets, Padding, Row, Stack, colored_box,
    container, text,
};
use peniko::Color;

use crate::case::{Case, Design};

const PAGE_BG: Color = Color::from_rgb8(0x0D, 0x14, 0x24);
const CARD_A: Color = Color::from_rgb8(0x3B, 0x82, 0xF6);
const CARD_B: Color = Color::from_rgb8(0xF4, 0x3F, 0x5E);
const CARD_C: Color = Color::from_rgb8(0x10, 0xB9, 0x81);
const CARD_D: Color = Color::from_rgb8(0xF5, 0x9E, 0x0B);

/// `Alignment`: the five named constants (`CENTER`/`TOP_LEFT`/`TOP_RIGHT`/
/// `BOTTOM_LEFT`/`BOTTOM_RIGHT`) each placed via `Align` inside one shared,
/// bounded box — the page's "component to `0.0..=1.0` fraction of free
/// space" mapping, read off five points at once.
fn alignment_case() -> AnyView<()> {
    fn badge(label: &'static str) -> AnyView<()> {
        any(container(text(label).color(Color::WHITE).size(11.0))
            .fill(CARD_A)
            .radius(6.0)
            .size_centered(104.0, 32.0))
    }
    let stack = Stack(vec![
        any(colored_box().fill(PAGE_BG).expand()),
        any(Align(Alignment::TOP_LEFT, badge("TOP_LEFT"))),
        any(Align(Alignment::TOP_RIGHT, badge("TOP_RIGHT"))),
        any(Align(Alignment::CENTER, badge("CENTER"))),
        any(Align(Alignment::BOTTOM_LEFT, badge("BOTTOM_LEFT"))),
        any(Align(Alignment::BOTTOM_RIGHT, badge("BOTTOM_RIGHT"))),
    ]);
    any(container(stack).size_centered(360.0, 240.0))
}

pub(super) const ALIGNMENT: Case = Case {
    slug: "alignment",
    title: "Alignment",
    size: Case::DEFAULT_SIZE,
    scale: Case::DEFAULT_SCALE,
    time_ms: Case::DEFAULT_TIME_MS,
    design: Design::Base,
    build: alignment_case,
};

/// `Color`: a row of `Color::from_rgb8` swatches, each labelled with its own
/// hex literal — the "literal is appropriate ... for authoring the palette
/// itself" case the page calls out, as opposed to a themed role lookup.
fn color_case() -> AnyView<()> {
    fn swatch(color: Color, label: &'static str) -> AnyView<()> {
        any(Column(vec![
            any(colored_box().fill(color).radius(8.0).size(64.0, 64.0)),
            any(text(label).color(Color::WHITE).size(12.0)),
        ])
        .cross_axis(CrossAxisAlignment::Center))
    }
    let row = Row(vec![
        swatch(CARD_A, "#3B82F6"),
        swatch(CARD_B, "#F43F5E"),
        swatch(CARD_C, "#10B981"),
        swatch(CARD_D, "#F59E0B"),
    ])
    .cross_axis(CrossAxisAlignment::Center);
    any(container(row).fill(PAGE_BG).size_centered(360.0, 240.0))
}

pub(super) const COLOR: Case = Case {
    slug: "color",
    title: "Color",
    size: Case::DEFAULT_SIZE,
    scale: Case::DEFAULT_SCALE,
    time_ms: Case::DEFAULT_TIME_MS,
    design: Design::Base,
    build: color_case,
};

/// `EdgeInsets`: the struct-literal form for an asymmetric inset (the page's
/// own example uses `EdgeInsets::all`, which [`super::layout`]'s `padding`
/// case already shows) — a wide top inset and a narrow inset on the other
/// three edges, so the gap itself reads as uneven per edge.
fn edge_insets_case() -> AnyView<()> {
    let insets = EdgeInsets {
        left: 12.0,
        top: 48.0,
        right: 12.0,
        bottom: 12.0,
    };
    let framed = container(Padding(
        insets,
        colored_box().fill(CARD_C).radius(6.0).expand(),
    ))
    .fill(CARD_A)
    .radius(10.0)
    .size_centered(280.0, 180.0);
    any(container(framed).fill(PAGE_BG).size_centered(360.0, 240.0))
}

pub(super) const EDGE_INSETS: Case = Case {
    slug: "edge-insets",
    title: "EdgeInsets",
    size: Case::DEFAULT_SIZE,
    scale: Case::DEFAULT_SCALE,
    time_ms: Case::DEFAULT_TIME_MS,
    design: Design::Base,
    build: edge_insets_case,
};

/// `TextStyle`: three runs bulk-styled with `text(..).style(..)` rather than
/// the leaf's individual `.size`/`.weight`/`.color` knobs — a heavier
/// heading, a regular body line, and a wide-tracked caption, each carrying a
/// distinct [`TextStyle`] value built with [`TextStyle::new`].
fn text_style_case() -> AnyView<()> {
    let heading = TextStyle {
        weight: FontWeight::BOLD,
        ..TextStyle::new(26.0, CARD_D)
    };
    let body = TextStyle::new(16.0, Color::WHITE);
    let mut caption = TextStyle::new(13.0, Color::from_rgb8(0x9C, 0xA3, 0xAF));
    caption.letter_spacing = 2.0;

    let column = Column(vec![
        any(text("Heading").style(heading)),
        any(text("Body copy styled in bulk").style(body)),
        any(text("CAPTION").style(caption)),
    ])
    .cross_axis(CrossAxisAlignment::Center);
    any(container(column).fill(PAGE_BG).size_centered(360.0, 240.0))
}

pub(super) const TEXT_STYLE: Case = Case {
    slug: "text-style",
    title: "TextStyle",
    size: Case::DEFAULT_SIZE,
    scale: Case::DEFAULT_SCALE,
    time_ms: Case::DEFAULT_TIME_MS,
    design: Design::Base,
    build: text_style_case,
};
