//! `Base`-design cases for the website's `widgets/base/scrolling` catalog
//! page set: `list-view`, `scroll-view`, `custom-scroll-view`, `slivers`. See the crate docs and
//! `crate::base` for the pure-`View`/slug-rule contract every case in this
//! registry follows, and [`super::framed`] for why neither case fills its
//! own backdrop.

use frust_core::View;
use frust_widgets::{
    Axis, CrossAxisAlignment, EdgeInsets, FlexChild, FlexView, SizedBox, container,
    custom_scroll_view, inflexible, list_view, scroll_view, sliver_box, sliver_fill_remaining,
    sliver_padding, text,
};
use peniko::Color;

use super::framed;
use crate::case::{Case, Design, case_build};

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

fn list_view_case() -> impl View<()> {
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

fn scroll_view_case() -> impl View<()> {
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

/// Viewport for the two sliver cases: smaller than `custom-scroll-view`'s
/// content so the clip is visible, leaving the variant's surface around it.
const SLIVER_VIEWPORT: (f64, f64) = (300.0, 180.0);

/// A fixed-height text swatch used by the sliver cases.
fn swatch(label: String, fill: Color, height: f64) -> impl View<()> {
    container(text(label).size(14.0).color(CAPTION))
        .fill(fill)
        .size_centered(SLIVER_VIEWPORT.0, height)
}

/// `custom_scroll_view`: a box-adapter header plus alternating box rows
/// (plain views mix with slivers as boxes). Ten 44px rows overflow the
/// viewport, so the clip is visible; the offset is the default zero.
///
/// Rows are `sliver_box` children rather than a `sliver_list`: the lazy
/// sliver lists materialise their rows in `View::rebuild` after a first
/// layout, and the static recorder runs a single build, so a `sliver_list`
/// would record blank.
fn custom_scroll_view_case() -> impl View<()> {
    let rows: Vec<_> = (0..10)
        .map(|i| sliver_box(swatch(format!("Row {}", i + 1), row_color(i), 44.0)))
        .collect();
    framed(
        SizedBox(Some(SLIVER_VIEWPORT.0), Some(SLIVER_VIEWPORT.1)).child(custom_scroll_view((
            sliver_box(swatch(
                "custom_scroll_view header".to_string(),
                Color::from_rgb8(0xBF, 0xDB, 0xFE),
                40.0,
            )),
            rows,
        ))),
    )
}

/// The sliver family that builds statically, in one viewport: a `sliver_box`
/// header, a `sliver_padding` around a `sliver_box`, and a
/// `sliver_fill_remaining` footer taking the leftover space. The content is
/// shorter than the viewport so the footer is visible. (The lazy
/// `sliver_list` / `sliver_fixed_extent_list` materialise at rebuild and so
/// cannot appear in a single static frame.)
fn slivers_case() -> impl View<()> {
    framed(
        SizedBox(Some(SLIVER_VIEWPORT.0), Some(SLIVER_VIEWPORT.1)).child(custom_scroll_view((
            sliver_box(swatch(
                "sliver_box".to_string(),
                Color::from_rgb8(0xBF, 0xDB, 0xFE),
                36.0,
            )),
            sliver_padding(
                EdgeInsets::symmetric(0.0, 8.0),
                sliver_box(swatch(
                    "sliver_padding".to_string(),
                    Color::from_rgb8(0xFE, 0xF3, 0xC7),
                    36.0,
                )),
            ),
            sliver_fill_remaining(
                container(text("fill_remaining").size(14.0).color(CAPTION))
                    .fill(Color::from_rgb8(0xBB, 0xF7, 0xD0))
                    .size_centered(SLIVER_VIEWPORT.0, 60.0),
            ),
        ))),
    )
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
        build: case_build!(list_view_case),
    },
    Case {
        slug: "scroll-view",
        title: "Scroll View",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Base,
        build: case_build!(scroll_view_case),
    },
    Case {
        slug: "custom-scroll-view",
        title: "Custom Scroll View",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Base,
        build: case_build!(custom_scroll_view_case),
    },
    Case {
        slug: "slivers",
        title: "Slivers",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Base,
        build: case_build!(slivers_case),
    },
];
