//! Base-design cases for the website's `layout` catalog category
//! (`apps/website/widgets/base/layout/`): `Align`, `divider`, `Row`/`Column`,
//! `Padding`, `SizedBox`, `Stack`.
//!
//! These cases record at an overridden 480x320 frame (rather than
//! [`Case::DEFAULT_SIZE`]) — layout widgets read more clearly with a little
//! extra room than the 360x240 default other categories use.

use frust_core::{AnyView, any};
use frust_widgets::{
    Align, Alignment, Axis, Column, CrossAxisAlignment, EdgeInsets, FlexView, Padding, SizedBox,
    Stack, colored_box, container, divider, flexible, inflexible, text,
};
use kurbo::Size;
use peniko::Color;

use crate::case::{Case, Design};

/// This module's overridden logical viewport — see the module docs.
const FRAME: Size = Size::new(480.0, 320.0);

const PAGE_BG: Color = Color::from_rgb8(0x0B, 0x12, 0x22);
const BOX_A: Color = Color::from_rgb8(0x3B, 0x82, 0xF6);
const BOX_B: Color = Color::from_rgb8(0xF4, 0x3F, 0x5E);
const BOX_C: Color = Color::from_rgb8(0x10, 0xB9, 0x81);
const LINE: Color = Color::from_rgb8(0xE5, 0xE7, 0xEB);

/// `Align`: a small badge pinned to `BOTTOM_RIGHT` inside a full-frame box —
/// `Align` fills a *bounded* axis to `constraints.biggest` (the page's own
/// unbounded-constraint warning), and the frame this case's outer wrapper
/// hands it is bounded on both axes.
fn align_case() -> AnyView<()> {
    let badge = container(text("BOTTOM_RIGHT").color(Color::WHITE).size(14.0))
        .fill(BOX_A)
        .radius(8.0)
        .size_centered(160.0, 56.0);
    any(container(Align(Alignment::BOTTOM_RIGHT, badge))
        .fill(PAGE_BG)
        .size_centered(FRAME.width, FRAME.height))
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
/// the flex pass's own unbounded main-axis probe.
fn divider_case() -> AnyView<()> {
    let column = Column(vec![
        any(container(text("Above").color(Color::WHITE).size(18.0))
            .size_centered(FRAME.width, 130.0)),
        any(divider(LINE).thickness(3.0)),
        any(container(text("Below").color(Color::WHITE).size(18.0))
            .size_centered(FRAME.width, 130.0)),
    ]);
    any(container(column)
        .fill(PAGE_BG)
        .size_centered(FRAME.width, FRAME.height))
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
fn flex_case() -> AnyView<()> {
    let row = FlexView::new(
        Axis::Horizontal,
        vec![
            inflexible(
                container(text("fixed").color(Color::WHITE).size(13.0))
                    .fill(BOX_A)
                    .size_centered(100.0, 220.0),
            ),
            flexible(
                2,
                container(text("flex 2").color(Color::WHITE).size(13.0)).fill(BOX_B),
            ),
            flexible(
                1,
                container(text("flex 1").color(Color::WHITE).size(13.0)).fill(BOX_C),
            ),
        ],
    )
    .cross_axis(CrossAxisAlignment::Stretch);
    any(container(row)
        .fill(PAGE_BG)
        .size_centered(FRAME.width, FRAME.height))
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
/// child — the inset gap between the two fills IS the padding.
fn padding_case() -> AnyView<()> {
    let framed = container(Padding(
        EdgeInsets::all(24.0),
        container(text("Child").color(Color::WHITE))
            .fill(BOX_C)
            .radius(6.0)
            .size_centered(160.0, 100.0),
    ))
    .fill(BOX_A)
    .radius(10.0);
    any(container(framed)
        .fill(PAGE_BG)
        .size_centered(FRAME.width, FRAME.height))
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
    any(container(boxed)
        .fill(PAGE_BG)
        .size_centered(FRAME.width, FRAME.height))
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

/// `Stack`: three overlaid children — a full-bleed background (the
/// bottom-most child establishes the stack's size, per the page's own
/// "needs a bounded box" note) plus two aligned corner badges.
fn stack_case() -> AnyView<()> {
    let overlay = Stack(vec![
        any(colored_box().fill(PAGE_BG).expand()),
        any(Align(
            Alignment::TOP_LEFT,
            container(text("TOP_LEFT").color(Color::WHITE).size(13.0))
                .fill(BOX_A)
                .radius(6.0)
                .size_centered(120.0, 48.0),
        )),
        any(Align(
            Alignment::BOTTOM_RIGHT,
            container(text("BOTTOM_RIGHT").color(Color::WHITE).size(13.0))
                .fill(BOX_C)
                .radius(6.0)
                .size_centered(120.0, 48.0),
        )),
    ]);
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
