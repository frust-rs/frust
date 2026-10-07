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
//! doesn't reach for).
//!
//! Every case takes its [`Case::DEFAULT_SIZE`] footprint from
//! [`super::framed`], which sizes the frame without filling it so the
//! recorder's per-variant `surface` clear stays visible (see [`super`]'s
//! "The variant has to reach the pixels"). A label sitting on that surface is
//! left at its themed `on_surface` default; the `TextStyle` case is the one
//! that *must* name colours explicitly — `TextStyle` carries its own colour
//! field, so a bulk-styled run can never fall back to the theme — and it
//! picks values that keep contrast on a light and a dark surface alike.

use frust_core::{AnyView, View, any};
use frust_text::{FontWeight, TextStyle};
use frust_widgets::{
    Align, Alignment, CrossAxisAlignment, EdgeInsets, Padding, colored_box, column, container, row,
    stack, text,
};
use peniko::Color;

use super::framed;
use crate::case::{Case, Design};

const CARD_A: Color = Color::from_rgb8(0x3B, 0x82, 0xF6);
const CARD_B: Color = Color::from_rgb8(0xF4, 0x3F, 0x5E);
const CARD_C: Color = Color::from_rgb8(0x10, 0xB9, 0x81);
const CARD_D: Color = Color::from_rgb8(0xF5, 0x9E, 0x0B);

/// [`text_style_case`]'s body gray, and the caption gray under it: mid tones
/// chosen to keep readable contrast against both the light (`#FAFAFA`) and
/// the dark (`#121212`) neutral surface, since a [`TextStyle`] always carries
/// an explicit colour of its own.
const BODY_INK: Color = Color::from_rgb8(0x75, 0x75, 0x75);
const CAPTION_INK: Color = Color::from_rgb8(0x8A, 0x8A, 0x8A);

/// `Alignment`: the five named constants (`CENTER`/`TOP_LEFT`/`TOP_RIGHT`/
/// `BOTTOM_LEFT`/`BOTTOM_RIGHT`) each placed via `Align` inside one shared,
/// bounded box — the page's "component to `0.0..=1.0` fraction of free
/// space" mapping, read off five points at once. The stack's bottom-most
/// child is an *unfilled* expanding box: it establishes the shared bounded
/// box the five `Align`s resolve against without painting over the variant's
/// surface clear.
fn alignment_case() -> AnyView<()> {
    fn badge(label: &'static str) -> impl View<()> {
        container(text(label).color(Color::WHITE).size(11.0))
            .fill(CARD_A)
            .radius(6.0)
            .size_centered(104.0, 32.0)
    }
    let stack = stack()
        .child(colored_box().expand())
        .child(Align(Alignment::TOP_LEFT, badge("TOP_LEFT")))
        .child(Align(Alignment::TOP_RIGHT, badge("TOP_RIGHT")))
        .child(Align(Alignment::CENTER, badge("CENTER")))
        .child(Align(Alignment::BOTTOM_LEFT, badge("BOTTOM_LEFT")))
        .child(Align(Alignment::BOTTOM_RIGHT, badge("BOTTOM_RIGHT")));
    any(framed(stack))
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
/// itself" case the page calls out, as opposed to a themed role lookup. The
/// labels themselves are the counterpoint: they take the themed default.
fn color_case() -> AnyView<()> {
    fn swatch(color: Color, label: &'static str) -> impl View<()> {
        column()
            .child(colored_box().fill(color).radius(8.0).size(64.0, 64.0))
            .child(text(label).size(12.0))
            .cross_axis(CrossAxisAlignment::Center)
    }
    let row = row()
        .child(swatch(CARD_A, "#3B82F6"))
        .child(swatch(CARD_B, "#F43F5E"))
        .child(swatch(CARD_C, "#10B981"))
        .child(swatch(CARD_D, "#F59E0B"))
        .cross_axis(CrossAxisAlignment::Center);
    any(framed(row))
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
    let framed_child = container(Padding(
        insets,
        colored_box().fill(CARD_C).radius(6.0).expand(),
    ))
    .fill(CARD_A)
    .radius(10.0)
    .size_centered(280.0, 180.0);
    any(framed(framed_child))
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
/// distinct [`TextStyle`] value built with [`TextStyle::new`]. Because a
/// `TextStyle` always names a colour, these three runs are the module's one
/// place that cannot defer to the theme; see [`BODY_INK`] for how the values
/// were picked.
fn text_style_case() -> AnyView<()> {
    let heading = TextStyle {
        weight: FontWeight::BOLD,
        ..TextStyle::new(26.0, CARD_A)
    };
    let body = TextStyle::new(16.0, BODY_INK);
    let mut caption = TextStyle::new(13.0, CAPTION_INK);
    caption.letter_spacing = 2.0;

    let column = column()
        .child(text("Heading").style(heading))
        .child(text("Body copy styled in bulk").style(body))
        .child(text("CAPTION").style(caption))
        .cross_axis(CrossAxisAlignment::Center);
    any(framed(column))
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
