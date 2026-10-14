//! Slivers section: one `custom_scroll_view` composing every sliver kind the
//! framework ships, the reference surface for the sliver protocol (see
//! `docs/WIDGETS_ARCHITECTURE.md`'s slivers and scroll sections):
//!
//! 1. a Material [`sliver_app_bar`] left uncontrolled, so the viewport itself
//!    collapses it from the large expanded band to the collapsed one, where it
//!    stays pinned;
//! 2. a floating section header ([`frust::sliver_persistent_header`]) that
//!    shrinks as it scrolls and comes back on any upward drag. Directly after
//!    the pinned bar a floating header reveals underneath it, as in Flutter, so
//!    only the part of its extent beyond the bar's collapsed band shows: its
//!    expanded extent ([`HEADER_MAX`]) is set well above that band;
//! 3. a lazy, keyed [`frust::sliver_list`] of [`ROW_COUNT`] rows, every
//!    [`TALL_ROW_PERIOD`]th one taller than the rest;
//! 4. a four-column [`frust::sliver_grid`];
//! 5. a [`frust::sliver_fill_remaining`] footer filling whatever the slivers
//!    before it leave.
//!
//! # Jumping to a row
//!
//! The buttons drive [`PlaygroundState::slivers_controller`] with
//! [`frust::ScrollController::jump_to`] — item commands are list-only, so a
//! jump to a row goes through [`item_offset`], which adds up the slivers and
//! rows above it and lands the row just under the pinned bar. The list's
//! extent estimate is the rows' exact average over one period
//! ([`ESTIMATED_ROW_EXTENT`]), so a row at a period boundary sits at the same
//! offset whether the rows above it were ever measured or not.
//!
//! The offset readout is published by the scroll surface's own `on_scroll`
//! into the page's state, so it follows every drag, fling and jump.

use frust::{
    Align, Alignment, ButtonStyle, ChildKey, Color, Component, EdgeInsets, Get, Padding, RwSignal,
    ScrollController, ScrollInfo, Set, SizedBox, SliverGridDelegate,
    SliverPersistentHeaderDelegate, Theme, View, button, column, component, container,
    custom_scroll_view, row, sliver_fill_remaining, sliver_grid, sliver_list,
    sliver_persistent_header, text, use_context,
};
use frust_material::{AppBarDensity, AppBarMetrics, AppBarVariant, sliver_app_bar};

use crate::PlaygroundState;

/// How many rows the list holds.
pub const ROW_COUNT: usize = 200;

/// Every row's extent except every [`TALL_ROW_PERIOD`]th one's.
const ROW_EXTENT: f64 = 48.0;

/// The taller extent the last row of every [`TALL_ROW_PERIOD`] rows takes.
const TALL_ROW_EXTENT: f64 = 96.0;

/// Rows `TALL_ROW_PERIOD - 1`, `2 * TALL_ROW_PERIOD - 1`, … are tall.
const TALL_ROW_PERIOD: usize = 4;

/// The list's extent estimate: the rows' exact average over one period (see
/// the module docs' *Jumping to a row*).
const ESTIMATED_ROW_EXTENT: f64 =
    (ROW_EXTENT * (TALL_ROW_PERIOD - 1) as f64 + TALL_ROW_EXTENT) / TALL_ROW_PERIOD as f64;

/// How many cells the grid holds.
const GRID_COUNT: usize = 24;

/// The grid's cross-axis cell count.
const GRID_COLUMNS: usize = 4;

/// The gap between grid cells, both ways.
const GRID_SPACING: f64 = 8.0;

/// The bar's metrics: its variant is large, its density regular.
const BAR_METRICS: AppBarMetrics = AppBarMetrics::for_density(AppBarDensity::Regular);

/// The bar's expanded band — also how much it scrolls.
pub const BAR_EXPANDED: f64 = BAR_METRICS.expanded(AppBarVariant::Large);

/// The band the pinned bar stays at once collapsed.
pub const BAR_COLLAPSED: f64 = BAR_METRICS.collapsed_height;

/// The section header's fully shrunk extent.
const HEADER_MIN: f64 = 40.0;

/// The section header's expanded extent — also how much it scrolls. Well above
/// [`BAR_COLLAPSED`] plus [`HEADER_MIN`], since a reveal under the pinned bar
/// shows only what lies beyond the bar's band.
pub const HEADER_MAX: f64 = 144.0;

/// The bounded height the scroll surface gets inside the page: the shell
/// wraps every page in a `scroll_view`, so an unsized viewport would
/// shrink-wrap and lay out every row.
const VIEWPORT_H: f64 = 480.0;

/// Row `index`'s extent: [`TALL_ROW_EXTENT`] for the last row of each
/// [`TALL_ROW_PERIOD`], [`ROW_EXTENT`] otherwise.
pub fn row_extent(index: usize) -> f64 {
    if index % TALL_ROW_PERIOD == TALL_ROW_PERIOD - 1 {
        TALL_ROW_EXTENT
    } else {
        ROW_EXTENT
    }
}

/// The scroll offset that puts row `index` (clamped to [`ROW_COUNT`], which
/// is the grid's start) just under the pinned, collapsed bar: the bar's and
/// the header's scroll extents plus every row above it, less the band the
/// bar keeps on screen.
pub fn item_offset(index: usize) -> f64 {
    let rows_above: f64 = (0..index.min(ROW_COUNT)).map(row_extent).sum();
    BAR_EXPANDED + HEADER_MAX + rows_above - BAR_COLLAPSED
}

/// See the page-fn contract in [`crate::pages`].
pub fn page(state: &PlaygroundState) -> impl View<PlaygroundState> {
    component(SliversPage {
        controller: state.slivers_controller.clone(),
    })
}

/// The page component: the controller its jump buttons and scroll surface
/// share.
struct SliversPage {
    controller: ScrollController,
}

/// The page's own state: the `(offset, max_offset)` its scroll surface last
/// reported.
struct SliversState {
    readout: RwSignal<(f64, f64)>,
}

impl Default for SliversState {
    fn default() -> Self {
        Self {
            readout: RwSignal::new((0.0, 0.0)),
        }
    }
}

impl Component for SliversPage {
    type State = SliversState;

    fn init(&self) -> SliversState {
        SliversState::default()
    }

    fn build(&self, state: &mut SliversState) -> impl View<SliversState> {
        let (offset, max_offset) = state.readout.get();
        let theme = Self::theme();
        let scheme = theme.scheme();
        let (accent, muted) = (scheme.primary, scheme.on_surface_variant);

        Padding(
            EdgeInsets::all(16.0),
            column()
                .child(text("Slivers").size(13.0).color(accent))
                .child(
                    text(
                        "One custom_scroll_view: a Material sliver_app_bar collapsing with the \
                         scroll and pinned at its collapsed band, a floating section header, a \
                         lazy variable-extent list, a four-column grid and a fill-remaining \
                         footer. The buttons jump the shared ScrollController.",
                    )
                    .size(11.0)
                    .color(muted),
                )
                .child(SizedBox(None, Some(12.0)))
                .child(
                    row()
                        .child(self.jump("Top", 0.0, ButtonStyle::Secondary))
                        .child(SizedBox(Some(8.0), None))
                        .child(self.jump("Row #40", item_offset(40), ButtonStyle::Primary))
                        .child(SizedBox(Some(8.0), None))
                        .child(self.jump("Row #120", item_offset(120), ButtonStyle::Primary)),
                )
                .child(SizedBox(None, Some(8.0)))
                .child(
                    row()
                        .child(self.jump("Grid", item_offset(ROW_COUNT), ButtonStyle::Secondary))
                        .child(SizedBox(Some(8.0), None))
                        .child(self.jump("End", f64::INFINITY, ButtonStyle::Secondary)),
                )
                .child(SizedBox(None, Some(8.0)))
                .child(text(format!("offset {offset:.0} / max {max_offset:.0}")).size(12.0))
                .child(SizedBox(None, Some(8.0)))
                .child(SizedBox(None, Some(VIEWPORT_H)).child(self.surface(&theme))),
        )
    }
}

impl SliversPage {
    fn theme() -> Theme {
        use_context::<Theme>().unwrap_or_else(frust_material::baseline)
    }

    /// A small button jumping the controller to `offset`.
    fn jump(&self, label: &str, offset: f64, style: ButtonStyle) -> impl View<SliversState> {
        let controller = self.controller.clone();
        button(label, move |_state: &mut SliversState| {
            controller.jump_to(offset)
        })
        .style(style)
        .small()
    }

    /// The scroll surface: bar, section header, list, grid, footer.
    fn surface(&self, theme: &Theme) -> impl View<SliversState> {
        let scheme = theme.scheme();
        let (accent, muted) = (scheme.primary, scheme.on_surface_variant);
        let (header_fill, header_ink) = (scheme.surface_container_high, scheme.on_surface);
        let (cell_fill, cell_ink) = (scheme.secondary_container, scheme.on_secondary_container);

        custom_scroll_view((
            sliver_app_bar("Collapsing bar")
                .variant(AppBarVariant::Large)
                .pinned(true)
                .corner_shift(false),
            sliver_persistent_header(SliverPersistentHeaderDelegate::new(
                HEADER_MIN,
                HEADER_MAX,
                move |shrink: f64, overlaps: bool| {
                    section_header(shrink, overlaps, header_fill, header_ink)
                },
            ))
            .floating(true),
            sliver_list(ROW_COUNT, ChildKey::new, move |index| {
                list_row(index, accent, muted)
            })
            .estimated_item_extent(ESTIMATED_ROW_EXTENT),
            sliver_grid(GRID_COUNT, move |index| {
                grid_cell(index, cell_fill, cell_ink)
            })
            .delegate(SliverGridDelegate::FixedCrossAxisCount {
                count: GRID_COLUMNS,
                main_spacing: GRID_SPACING,
                cross_spacing: GRID_SPACING,
                child_aspect_ratio: 1.0,
            }),
            sliver_fill_remaining(footer(muted)).has_scroll_body(false),
        ))
        .controller(self.controller.clone())
        .on_scroll(|state: &mut SliversState, info: ScrollInfo| {
            state.readout.set((info.offset, info.max_offset));
        })
    }
}

/// The floating section header's content for how far it has shrunk — a pure
/// function of its arguments, as the header's delegate requires.
fn section_header(shrink: f64, overlaps: bool, fill: Color, ink: Color) -> impl View<SliversState> {
    let note = if overlaps { "  (over the content)" } else { "" };
    container(Padding(
        EdgeInsets::symmetric(16.0, 8.0),
        text(format!("Floating header, shrunk {shrink:.0} px{note}"))
            .size(12.0)
            .color(ink),
    ))
    .fill(fill)
}

/// One list row, its tall rows marked so the variable extents show.
fn list_row(index: usize, accent: Color, muted: Color) -> impl View<SliversState> {
    let tall = row_extent(index) > ROW_EXTENT;
    let label = if tall {
        format!("Row {index} (tall)")
    } else {
        format!("Row {index}")
    };
    SizedBox::<SliversState>(None, Some(row_extent(index))).child(Padding(
        EdgeInsets::symmetric(12.0, 8.0),
        text(label)
            .size(12.0)
            .color(if tall { accent } else { muted }),
    ))
}

/// One grid cell, numbered.
fn grid_cell(index: usize, fill: Color, ink: Color) -> impl View<SliversState> {
    container(Align(
        Alignment::CENTER,
        text(format!("{index}")).size(12.0).color(ink),
    ))
    .fill(fill)
    .radius(8.0)
}

/// The footer `sliver_fill_remaining` stretches over what is left.
fn footer(muted: Color) -> impl View<SliversState> {
    Padding(
        EdgeInsets::all(24.0),
        text("End of the surface: sliver_fill_remaining fills what the slivers above leave.")
            .size(11.0)
            .color(muted),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_estimate_is_the_exact_period_average() {
        let period: f64 = (0..TALL_ROW_PERIOD).map(row_extent).sum();
        assert_eq!(ESTIMATED_ROW_EXTENT * TALL_ROW_PERIOD as f64, period);
        assert_eq!(row_extent(0), ROW_EXTENT);
        assert_eq!(row_extent(TALL_ROW_PERIOD - 1), TALL_ROW_EXTENT);
    }

    #[test]
    fn a_row_offset_sums_the_slivers_and_rows_above_it() {
        assert_eq!(item_offset(0), BAR_EXPANDED + HEADER_MAX - BAR_COLLAPSED);
        assert_eq!(
            item_offset(40) - item_offset(0),
            40.0 * ESTIMATED_ROW_EXTENT,
            "a period-boundary row sits where the estimate puts it"
        );
        assert_eq!(item_offset(ROW_COUNT + 10), item_offset(ROW_COUNT));
    }
}
