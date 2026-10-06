//! `Base`-design cases for the website's `widgets/base/scrolling` catalog
//! page set: `list-view`, `scroll-view`. See the crate docs and
//! `crate::base` for the pure-`View`/slug-rule contract every case in this
//! registry follows, and [`super::framed`] for why neither case fills its
//! own backdrop.

use frust_core::AnyView;
use frust_widgets::{
    Axis, CrossAxisAlignment, FlexChild, FlexView, SizedBox, container, inflexible, list_view,
    scroll_view, text,
};
use peniko::Color;

use super::framed;
use crate::case::{Case, Design};

/// Explicit caption color for row text painted directly onto a fixed
/// `row_color` swatch — self-consistent (both literal, not theme-driven)
/// regardless of which `Variant` the recorder resolves, unlike
/// [`super::framed`]'s deliberately un-filled backdrop.
const CAPTION: Color = Color::from_rgb8(0x33, 0x33, 0x37);

/// The viewport `list_view` is boxed into inside the frame. A scrollable
/// viewport hands its rows TIGHT cross-axis constraints, so a row's own
/// `size_centered` width cannot shrink it — the box is what leaves the
/// variant's cleared surface visible around the list.
const LIST_VIEWPORT: (f64, f64) = (300.0, 196.0);

/// Alternating row swatch color, so a scrolled/virtualized list reads as a
/// stack of distinct rows rather than one solid fill.
fn row_color(index: usize) -> Color {
    if index.is_multiple_of(2) {
        Color::from_rgb8(0xE5, 0xE7, 0xEB)
    } else {
        Color::from_rgb8(0xD1, 0xD5, 0xDB)
    }
}

/// The row swatch width. `scroll_view` measures its content against a LOOSE
/// cross axis, so a `scroll-view` row honours this and leaves a margin
/// either side; `list_view`'s virtualized viewport hands its rows a TIGHT
/// cross axis instead, so a `list-view` row spans the whole
/// [`LIST_VIEWPORT`] width regardless of what is asked for here.
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
    framed(
        SizedBox(Some(LIST_VIEWPORT.0), Some(LIST_VIEWPORT.1)).child(list_view(
            6,
            LIST_ITEM_EXTENT,
            |i: usize| {
                container(text(format!("Row {}", i + 1)).size(14.0).color(CAPTION))
                    .fill(row_color(i))
                    .size_centered(ROW_WIDTH, LIST_ITEM_EXTENT - 12.0)
            },
        )),
    )
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
