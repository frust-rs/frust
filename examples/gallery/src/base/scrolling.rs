//! `Base`-design cases for the website's `widgets/base/scrolling` catalog
//! page set: `list-view`, `scroll-view` (g1-04). See the crate docs and
//! `crate::base` for the pure-`View`/slug-rule contract every case in this
//! registry follows.

use frust_core::{AnyView, View, any};
use frust_widgets::{
    Axis, CrossAxisAlignment, FlexChild, FlexView, container, inflexible, list_view, scroll_view,
    text,
};
use peniko::Color;

use crate::case::{Case, Design};

/// Explicit caption color for row text painted directly onto a fixed
/// `row_color` swatch — self-consistent (both literal, not theme-driven)
/// regardless of which `Variant` the recorder resolves, unlike `framed`'s
/// deliberately un-filled backdrop (see its own doc).
const CAPTION: Color = Color::from_rgb8(0x33, 0x33, 0x37);

/// Wrap `child`, centered, in a fixed 360x240 frame (`Case::DEFAULT_SIZE`) —
/// see `input.rs`'s `framed` for why this deliberately never fills the
/// backdrop itself (the recorder already clears to the active theme's
/// `surface` per `Variant`, which is what keeps a themed label legible in
/// both the light and dark recording pass).
fn framed<V: View<()>>(child: V) -> AnyView<()> {
    any(container(child).size_centered(Case::DEFAULT_SIZE.width, Case::DEFAULT_SIZE.height))
}

/// Alternating row swatch color, so a scrolled/virtualized list reads as a
/// stack of distinct rows rather than one solid fill.
fn row_color(index: usize) -> Color {
    if index.is_multiple_of(2) {
        Color::from_rgb8(0xE5, 0xE7, 0xEB)
    } else {
        Color::from_rgb8(0xD1, 0xD5, 0xDB)
    }
}

/// A row swatch wide enough to show inside the frame with a visible margin.
const ROW_WIDTH: f64 = 320.0;

/// A fresh `ListView` materializes its rebuild-time window before its first
/// layout has ever run, so it starts from a `0`-height cached viewport —
/// producing a 2-row starting window (see `desired_window`'s `BUFFER` doc in
/// `crates/frust-widgets/src/list_view.rs`) regardless of `item_extent`. A
/// single static build (this registry's whole contract — no second rebuild
/// pass runs to re-window against the now-known viewport) always lands on
/// that same 2-row window, so a tall `item_extent` — rather than a short one
/// with many rows — is what fills the frame and shows a clipped second row.
const LIST_ITEM_EXTENT: f64 = 132.0;

fn list_view_case() -> AnyView<()> {
    framed(list_view(6, LIST_ITEM_EXTENT, |i: usize| {
        any(
            container(text(format!("Row {}", i + 1)).size(14.0).color(CAPTION))
                .fill(row_color(i))
                .size_centered(ROW_WIDTH, LIST_ITEM_EXTENT - 12.0),
        )
    }))
}

fn scroll_view_case() -> AnyView<()> {
    let rows: Vec<FlexChild<()>> = (0..8)
        .map(|i| {
            inflexible(
                container(text(format!("Item {}", i + 1)).size(14.0).color(CAPTION))
                    .fill(row_color(i))
                    .size_centered(ROW_WIDTH, 48.0),
            )
        })
        .collect();
    framed(scroll_view(
        FlexView::new(Axis::Vertical, rows).cross_axis(CrossAxisAlignment::Center),
    ))
}

/// This module's slice of the registry [`crate::cases`] concatenates.
pub const CASES: &[Case] = &[
    Case {
        slug: "list-view",
        title: "List View",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Base,
        build: list_view_case,
    },
    Case {
        slug: "scroll-view",
        title: "Scroll View",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Base,
        build: scroll_view_case,
    },
];
