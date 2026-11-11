//! Scroll section: a `ScrollController` driving a keyed, variable-extent
//! `ListView` of ~300 rows — jump/animate to an offset, or scroll straight to
//! a named row by key — demonstrating the programmatic-scroll seam onto
//! `ListView` (see `docs/WIDGETS_ARCHITECTURE.md`'s *Scroll Physics*/
//! *Virtualized ListView* sections for the surface underneath).
//!
//! # Rows
//!
//! [`ROW_COUNT`] rows, keyed by index (`ChildKey::new(usize)`); every
//! [`TALL_ROW_PERIOD`]th row is taller than the rest ([`TALL_ROW_EXTENT`] vs
//! [`ROW_EXTENT`]), so the list runs in `ListView::builder_keyed`'s
//! variable-extent mode (`.estimated_item_extent`) instead of the
//! closed-form uniform path — the same shape a real chat/feed list needs.
//!
//! # Buttons
//!
//! - "Jump to 0" / "Jump to end" — [`frust::ScrollController::jump_to`], an
//!   instant move (`f64::INFINITY` lands on the bottom edge).
//! - "Animate to 50%" — [`frust::ScrollController::animate_to`] to half of
//!   the *current* `max_offset`, read fresh at press time rather than at
//!   build time, so a press always targets the content's current midpoint.
//! - "Item #150 (Start)" / "Item #42 (Center)" / "Animated #280 (End)" —
//!   [`frust::ScrollController::scroll_to_item`], the last one easing there
//!   (`animated: true`) instead of jumping.
//!
//! # Interruption and reduced motion
//!
//! A user drag or wheel input on the list cancels any `animate_to`/animated
//! `scroll_to_item` in flight, leaving the list wherever it had eased to —
//! user input always wins over a programmatic scroll (see
//! `frust::ScrollController::animate_to`'s own doc comment). Under the
//! app-bar's "reduce motion" toggle, or with "animations" switched off (both
//! fold into the same effective `motion.reduce_motion` theme flag — see
//! `crate::effective_reduce_motion`), every animated move on this page
//! collapses to an instant jump instead, same as everywhere else in this app.
//!
//! # Readout
//!
//! The offset/max-offset line below the list is published by
//! [`frust::ScrollController::on_change`] into
//! [`crate::PlaygroundState::scroll_readout`] — a signal write from a
//! listener that runs with no `&mut State` (see that field's own doc
//! comment) — so it live-updates through every drag, fling, and
//! programmatic move, not just on a button press.
//!
//! # Exact extents
//!
//! A second list of [`EXACT_ROW_COUNT`] rows declares every row's height up
//! front through [`frust::ListView::item_extent_of`] ([`exact_row_height`]
//! cycles through [`EXACT_HEIGHTS`]), so nothing is estimated or measured.
//! Its "Row N" buttons `jump_to` [`exact_row_offset`] — the closed-form sum
//! of the heights before row N — and land with that row exactly at the top
//! (the last row stops at the bottom edge, since a jump clamps to
//! `max_offset`). The "visible rows" line under it is fed by
//! [`frust::ListView::on_visible_range`] into
//! [`crate::PlaygroundState::exact_visible_range`]; that delivery is one
//! event late, so the line catches up on the next event after a scroll.

use frust::{
    AnimateTo, ButtonStyle, ChildKey, Color, Curve, EdgeInsets, Get, ItemAlignment, ListView,
    Padding, ScrollbarShow, ScrollbarStyle, Set, SizedBox, Theme, View, button, column, row,
    scroll_view, text, use_context, wrap,
};

use crate::PlaygroundState;

/// How many rows the demo list holds.
const ROW_COUNT: usize = 300;

/// Every row's height — and the keyed list's `estimated_item_extent` — except
/// every [`TALL_ROW_PERIOD`]th one, which uses [`TALL_ROW_EXTENT`] instead.
const ROW_EXTENT: f64 = 56.0;

/// The taller row height every [`TALL_ROW_PERIOD`]th row uses — the
/// variable-extent mix [`frust::ListView::estimated_item_extent`] exists for.
const TALL_ROW_EXTENT: f64 = 160.0;

/// Every how-many-th row (by index, 0-based) is tall: rows 0, 7, 14, ….
const TALL_ROW_PERIOD: usize = 7;

/// The bounded height the list's own viewport gets inside the page's
/// scrolling column — the same reason `pages::graph_canvas`'s `PanZoomView`
/// needs one: the shell wraps every page body in a `scroll_view`, so an
/// un-sized scrolling child here would read an unbounded height instead of a
/// real window to scroll within.
const VIEWPORT_H: f64 = 420.0;

/// This row's height: [`TALL_ROW_EXTENT`] every [`TALL_ROW_PERIOD`]th row,
/// [`ROW_EXTENT`] otherwise.
fn row_height(index: usize) -> f64 {
    if index.is_multiple_of(TALL_ROW_PERIOD) {
        TALL_ROW_EXTENT
    } else {
        ROW_EXTENT
    }
}

/// How many rows the exact-extent list holds — enough that measuring rows
/// on the way to a far jump target would be hopeless.
const EXACT_ROW_COUNT: usize = 100_000;

/// The heights the exact-extent list cycles through by index, so the offset
/// of any row has a closed form.
const EXACT_HEIGHTS: [f64; 3] = [40.0, 64.0, 96.0];

/// The rows the exact-extent list's jump buttons target.
const EXACT_JUMP_ROWS: [usize; 4] = [0, 12_345, 50_000, 99_999];

/// This exact-extent row's declared height.
fn exact_row_height(index: usize) -> f64 {
    EXACT_HEIGHTS[index % EXACT_HEIGHTS.len()]
}

/// The sum of the heights of every row before `index`: whole cycles of
/// [`EXACT_HEIGHTS`] plus the partial cycle, so a press costs O(1) rather
/// than a walk over up to 100k rows. All heights are small integers, so the
/// sum is exact in `f64` and matches the widget's own prefix sum.
fn exact_row_offset(index: usize) -> f64 {
    let period = EXACT_HEIGHTS.len();
    let cycle: f64 = EXACT_HEIGHTS.iter().sum();
    let partial: f64 = EXACT_HEIGHTS[..index % period].iter().sum();
    (index / period) as f64 * cycle + partial
}

/// One row's content: its index, a tall row highlighted in the theme's
/// accent color and marked "(tall)" so the variable-extent mix is visible on
/// screen, not just measurable.
fn row_view(index: usize, accent: Color, muted: Color) -> impl View<PlaygroundState> {
    let tall = index.is_multiple_of(TALL_ROW_PERIOD);
    let label = if tall {
        format!("Row {index} (tall)")
    } else {
        format!("Row {index}")
    };
    SizedBox::<PlaygroundState>(None, Some(row_height(index))).child(Padding(
        EdgeInsets::symmetric(12.0, 8.0),
        text(label)
            .size(12.0)
            .color(if tall { accent } else { muted }),
    ))
}

/// See the page-fn contract in [`crate::pages`].
pub fn page(state: &PlaygroundState) -> impl View<PlaygroundState> {
    let theme = use_context::<Theme>().unwrap_or_else(frust_material::baseline);
    let scheme = theme.scheme();
    let accent = scheme.primary;
    let muted = scheme.on_surface_variant;

    let key_of = move |index: usize| ChildKey::new(index);
    let builder = move |index: usize| row_view(index, accent, muted);
    let list = ListView::builder_keyed(ROW_COUNT, ROW_EXTENT, key_of, builder)
        .estimated_item_extent(ROW_EXTENT)
        .controller(state.scroll_controller.clone())
        .scrollbar(ScrollbarStyle::default());
    let list_viewport = SizedBox::<PlaygroundState>(None, Some(VIEWPORT_H)).child(list);

    let (offset, max_offset) = state.scroll_readout.get();
    let readout = format!(
        "offset {offset:.0} / max {max_offset:.0}  animating: {}",
        state.scroll_controller.is_animating(),
    );

    let jump_start = {
        let controller = state.scroll_controller.clone();
        button("Jump to 0", move |_state: &mut PlaygroundState| {
            controller.jump_to(0.0);
        })
        .style(ButtonStyle::Secondary)
        .small()
    };
    let jump_end = {
        let controller = state.scroll_controller.clone();
        button("Jump to end", move |_state: &mut PlaygroundState| {
            controller.jump_to(f64::INFINITY);
        })
        .style(ButtonStyle::Secondary)
        .small()
    };
    let animate_half = {
        let controller = state.scroll_controller.clone();
        button("Animate to 50%", move |_state: &mut PlaygroundState| {
            let target = controller.max_offset() * 0.5;
            controller.animate_to(
                target,
                AnimateTo {
                    duration_ms: 400.0,
                    curve: Curve::EaseInOut,
                },
            );
        })
        .style(ButtonStyle::Primary)
        .small()
    };
    let item_150 = {
        let controller = state.scroll_controller.clone();
        button("Item #150 (Start)", move |_state: &mut PlaygroundState| {
            controller.scroll_to_item(ChildKey::new(150usize), ItemAlignment::Start, false);
        })
        .style(ButtonStyle::Secondary)
        .small()
    };
    let item_42 = {
        let controller = state.scroll_controller.clone();
        button("Item #42 (Center)", move |_state: &mut PlaygroundState| {
            controller.scroll_to_item(ChildKey::new(42usize), ItemAlignment::Center, false);
        })
        .style(ButtonStyle::Secondary)
        .small()
    };
    let item_280_animated = {
        let controller = state.scroll_controller.clone();
        button(
            "Animated #280 (End)",
            move |_state: &mut PlaygroundState| {
                controller.scroll_to_item(ChildKey::new(280usize), ItemAlignment::End, true);
            },
        )
        .style(ButtonStyle::Primary)
        .small()
    };

    let exact_list = ListView::builder(EXACT_ROW_COUNT, EXACT_HEIGHTS[0], move |index: usize| {
        let height = exact_row_height(index);
        SizedBox::<PlaygroundState>(None, Some(height)).child(Padding(
            EdgeInsets::symmetric(12.0, 8.0),
            text(format!("Row {index} · {height:.0} px"))
                .size(12.0)
                .color(muted),
        ))
    })
    .item_extent_of(exact_row_height)
    .on_visible_range(|state: &mut PlaygroundState, first, last| {
        state.exact_visible_range.set(Some((first, last)));
    })
    .controller(state.exact_scroll_controller.clone())
    .scrollbar(ScrollbarStyle::default());
    let exact_viewport = SizedBox::<PlaygroundState>(None, Some(VIEWPORT_H)).child(exact_list);

    let visible_label = match state.exact_visible_range.get() {
        Some((first, last)) => format!("visible rows {first}–{last}"),
        None => "visible rows —".to_string(),
    };

    // A wrap, not a row: four buttons do not fit one line on a phone width.
    let mut exact_jumps = wrap().spacing(8.0).run_spacing(8.0);
    for target in EXACT_JUMP_ROWS {
        let controller = state.exact_scroll_controller.clone();
        let label = if target >= 1_000 {
            format!("Row {},{:03}", target / 1_000, target % 1_000)
        } else {
            format!("Row {target}")
        };
        exact_jumps = exact_jumps.child(
            button(label, move |_state: &mut PlaygroundState| {
                controller.jump_to(exact_row_offset(target));
            })
            .style(ButtonStyle::Secondary)
            .small(),
        );
    }

    // Scrollbar showcase: a plain `ScrollView` with the default
    // (`WhileScrolling`) bar and one with the `Always` variant. The list above
    // carries the default bar too.
    let sample_rows = |prefix: &'static str| {
        let mut col = column();
        for i in 0..24 {
            col = col.child(text(format!("{prefix} row {i}")).size(12.0));
        }
        col
    };
    let default_bar = SizedBox::<PlaygroundState>(None, Some(96.0))
        .child(scroll_view(sample_rows("Default bar")).scrollbar(ScrollbarStyle::default()));
    let always_bar = SizedBox::<PlaygroundState>(None, Some(96.0)).child(
        scroll_view(sample_rows("Always bar"))
            .scrollbar(ScrollbarStyle::new(ScrollbarShow::Always)),
    );

    Padding(
        EdgeInsets::all(16.0),
        column()
            .child(text("Scroll").size(13.0).color(accent))
            .child(
                text(
                    "A keyed, variable-extent ListView bound to a ScrollController: \
                         jump or animate to an offset, or scroll straight to a named row. \
                         Dragging the list while an animated move is in flight interrupts \
                         it — user input always wins. Under reduce-motion (or with \
                         animations off) every animated move here collapses to an instant \
                         jump instead.",
                )
                .size(11.0)
                .color(muted),
            )
            .child(SizedBox(None, Some(12.0)))
            .child(list_viewport)
            .child(SizedBox(None, Some(8.0)))
            .child(text(readout).size(12.0))
            .child(SizedBox(None, Some(8.0)))
            .child(
                row()
                    .child(jump_start)
                    .child(SizedBox(Some(8.0), None))
                    .child(jump_end)
                    .child(SizedBox(Some(8.0), None))
                    .child(animate_half),
            )
            .child(SizedBox(None, Some(8.0)))
            .child(
                row()
                    .child(item_150)
                    .child(SizedBox(Some(8.0), None))
                    .child(item_42)
                    .child(SizedBox(Some(8.0), None))
                    .child(item_280_animated),
            )
            .child(SizedBox(None, Some(16.0)))
            .child(text("Exact extents").size(13.0).color(accent))
            .child(
                text(
                    "A 100,000-row ListView that declares every row's height up front \
                     (40, 64 or 96 px by index), so a jump lands exactly on the named row \
                     even this far down a very long list — nothing is estimated or \
                     measured. The label below shows the rows currently on screen.",
                )
                .size(11.0)
                .color(muted),
            )
            .child(SizedBox(None, Some(8.0)))
            .child(exact_viewport)
            .child(SizedBox(None, Some(8.0)))
            .child(text(visible_label).size(12.0))
            .child(SizedBox(None, Some(8.0)))
            .child(exact_jumps)
            .child(SizedBox(None, Some(16.0)))
            .child(text("Scrollbars").size(13.0).color(accent))
            .child(
                text(
                    "Opt-in baseline scrollbar: the list above and the first box show it \
                     while scrolling (fades out); the second box keeps it always visible. \
                     Hover widens the thumb; drag the thumb or click the track.",
                )
                .size(11.0)
                .color(muted),
            )
            .child(SizedBox(None, Some(8.0)))
            .child(default_bar)
            .child(SizedBox(None, Some(8.0)))
            .child(always_bar),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_height_is_tall_every_seventh_row() {
        assert_eq!(row_height(0), TALL_ROW_EXTENT);
        assert_eq!(row_height(7), TALL_ROW_EXTENT);
        assert_eq!(row_height(14), TALL_ROW_EXTENT);
        assert_eq!(row_height(1), ROW_EXTENT);
        assert_eq!(row_height(6), ROW_EXTENT);
        assert_eq!(row_height(8), ROW_EXTENT);
    }

    #[test]
    fn exact_heights_have_three_distinct_values() {
        let mut seen: Vec<f64> = (0..EXACT_HEIGHTS.len()).map(exact_row_height).collect();
        seen.dedup();
        assert_eq!(seen.len(), 3);
    }

    #[test]
    fn exact_row_offset_matches_a_brute_force_sum() {
        let brute = |index: usize| (0..index).map(exact_row_height).sum::<f64>();
        for index in [0, 1, 2, 3, 4, 12_345, 50_000, EXACT_ROW_COUNT - 1] {
            assert_eq!(exact_row_offset(index), brute(index), "row {index}");
        }
    }
}
