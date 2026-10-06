//! Base-design cases for the website's `layout` catalog category
//! (`apps/website/widgets/base/layout/`): `Align`, `divider`, `Row`/`Column`,
//! `Padding`, `SizedBox`, `Stack`.
//!
//! These cases record at an overridden 480x320 frame (rather than
//! [`Case::DEFAULT_SIZE`]) — layout widgets read more clearly with a little
//! extra room than the 360x240 default other categories use — so they reach
//! for [`super::framed_in`] rather than [`super::framed`]. Like every frame
//! helper it sizes without filling, which is what leaves the recorder's
//! per-variant `surface` clear visible around each case's content (see
//! [`super`]'s "The variant has to reach the pixels"): the swatches below are
//! all sized smaller than `FRAME` for that reason, and text painted onto the
//! frame itself is left at its themed `on_surface` default.

use frust_core::{AnyView, any};
use frust_widgets::{
    Align, Alignment, CrossAxisAlignment, EdgeInsets, Padding, SizedBox, colored_box, column,
    container, divider, row, stack, text,
};
use kurbo::Size;
use peniko::Color;

use super::framed_in;
use crate::case::{Case, Design};

/// This module's overridden logical viewport — see the module docs.
const FRAME: Size = Size::new(480.0, 320.0);

const BOX_A: Color = Color::from_rgb8(0x3B, 0x82, 0xF6);
const BOX_B: Color = Color::from_rgb8(0xF4, 0x3F, 0x5E);
const BOX_C: Color = Color::from_rgb8(0x10, 0xB9, 0x81);
/// A slate panel dark enough for white text and distinct from both the light
/// and the dark surface the recorder clears to.
const PANEL: Color = Color::from_rgb8(0x33, 0x41, 0x55);
/// The `divider` rule's colour: a mid gray that stays visible on a light
/// (`#FAFAFA`) as well as a dark (`#121212`) surface.
const LINE: Color = Color::from_rgb8(0x8A, 0x8A, 0x8A);

/// `Align`: a small badge pinned to `BOTTOM_RIGHT` inside a full-frame box —
/// `Align` fills a *bounded* axis to `constraints.biggest` (the page's own
/// unbounded-constraint warning), and the frame this case's outer wrapper
/// hands it is bounded on both axes.
fn align_case() -> AnyView<()> {
    let badge = container(text("BOTTOM_RIGHT").color(Color::WHITE).size(14.0))
        .fill(BOX_A)
        .radius(8.0)
        .size_centered(160.0, 56.0);
    framed_in(FRAME, Align(Alignment::BOTTOM_RIGHT, badge))
}

pub(super) const ALIGN: Case = Case {
    slug: "align",
    title: "Align",
    size: FRAME,
    scale: Case::DEFAULT_SCALE,
    time_ms: Case::DEFAULT_TIME_MS,
    design: Design::Base,
    build: align_case,
};

/// `divider`: a hairline rule between two labelled blocks in a `Column` —
/// the divider fills the column's bounded cross (width) axis regardless of
/// the flex pass's own unbounded main-axis probe. The two blocks are
/// unfilled, so the rule reads against the variant's own surface.
fn divider_case() -> AnyView<()> {
    let column = column()
        .child(container(text("Above").size(18.0)).size_centered(FRAME.width, 130.0))
        .child(divider(LINE).thickness(3.0))
        .child(container(text("Below").size(18.0)).size_centered(FRAME.width, 130.0));
    framed_in(FRAME, column)
}

pub(super) const DIVIDER: Case = Case {
    slug: "divider",
    title: "divider",
    size: FRAME,
    scale: Case::DEFAULT_SCALE,
    time_ms: Case::DEFAULT_TIME_MS,
    design: Design::Base,
    build: divider_case,
};

/// `Row`/`Column`: one inflexible badge plus two flexible panels sharing the
/// remaining main-axis space 2:1, via `FlexView::new`/`flexible`/`inflexible`.
/// The row is boxed to 400x240 inside the frame so the three shares read as a
/// band on the surface rather than as the whole preview.
fn flex_case() -> AnyView<()> {
    let row = row()
        .child(
            container(text("fixed").color(Color::WHITE).size(13.0))
                .fill(BOX_A)
                .size_centered(100.0, 220.0),
        )
        .flex(
            2,
            container(text("flex 2").color(Color::WHITE).size(13.0)).fill(BOX_B),
        )
        .flex(
            1,
            container(text("flex 1").color(Color::WHITE).size(13.0)).fill(BOX_C),
        )
        .cross_axis(CrossAxisAlignment::Stretch);
    framed_in(FRAME, SizedBox(Some(400.0), Some(240.0)).child(row))
}

pub(super) const FLEX: Case = Case {
    slug: "flex",
    title: "Row and Column",
    size: FRAME,
    scale: Case::DEFAULT_SCALE,
    time_ms: Case::DEFAULT_TIME_MS,
    design: Design::Base,
    build: flex_case,
};

/// `Padding`: a filled outer frame around an inset, differently-colored
/// child — the inset gap between the two fills IS the padding. The outer
/// frame shrink-wraps its padded child, so it stays well inside the recorded
/// frame.
fn padding_case() -> AnyView<()> {
    let framed_child = container(Padding(
        EdgeInsets::all(24.0),
        container(text("Child").color(Color::WHITE))
            .fill(BOX_C)
            .radius(6.0)
            .size_centered(160.0, 100.0),
    ))
    .fill(BOX_A)
    .radius(10.0);
    framed_in(FRAME, framed_child)
}

pub(super) const PADDING: Case = Case {
    slug: "padding",
    title: "Padding",
    size: FRAME,
    scale: Case::DEFAULT_SCALE,
    time_ms: Case::DEFAULT_TIME_MS,
    design: Design::Base,
    build: padding_case,
};

/// `SizedBox`: forces a fixed 220x140 box around a child regardless of its
/// own natural size.
fn sized_box_case() -> AnyView<()> {
    let boxed = SizedBox(Some(220.0), Some(140.0)).child(
        container(text("220x140").color(Color::WHITE))
            .fill(BOX_B)
            .radius(8.0),
    );
    framed_in(FRAME, boxed)
}

pub(super) const SIZED_BOX: Case = Case {
    slug: "sized-box",
    title: "SizedBox",
    size: FRAME,
    scale: Case::DEFAULT_SCALE,
    time_ms: Case::DEFAULT_TIME_MS,
    design: Design::Base,
    build: sized_box_case,
};

/// `Stack`: four overlaid children. The bottom-most one establishes the
/// stack's size (the page's own "needs a bounded box" note) — here an
/// *unfilled* `colored_box().expand()`, so it does that sizing job without
/// painting a fixed backdrop over the variant's surface clear. Above it sit a
/// centered panel and two corner badges that overlap its edges, which is what
/// makes the overlay order visible.
fn stack_case() -> AnyView<()> {
    let overlay = stack()
        .child(colored_box().expand())
        .child(Align(
            Alignment::CENTER,
            container(text("base panel").color(Color::WHITE).size(14.0))
                .fill(PANEL)
                .radius(12.0)
                .size_centered(320.0, 200.0),
        ))
        .child(Align(
            Alignment::TOP_LEFT,
            container(text("TOP_LEFT").color(Color::WHITE).size(13.0))
                .fill(BOX_A)
                .radius(6.0)
                .size_centered(120.0, 48.0),
        ))
        .child(Align(
            Alignment::BOTTOM_RIGHT,
            container(text("BOTTOM_RIGHT").color(Color::WHITE).size(13.0))
                .fill(BOX_C)
                .radius(6.0)
                .size_centered(120.0, 48.0),
        ));
    any(SizedBox(Some(FRAME.width), Some(FRAME.height)).child(overlay))
}

pub(super) const STACK: Case = Case {
    slug: "stack",
    title: "Stack",
    size: FRAME,
    scale: Case::DEFAULT_SCALE,
    time_ms: Case::DEFAULT_TIME_MS,
    design: Design::Base,
    build: stack_case,
};
