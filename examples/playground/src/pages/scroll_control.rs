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

use frust::{
    AnimateTo, AnyView, ButtonStyle, ChildKey, Color, Curve, EdgeInsets, Get, ItemAlignment,
    ListView, Padding, SizedBox, Theme, any, button, column, row, text, use_context,
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

/// One row's content: its index, a tall row highlighted in the theme's
/// accent color and marked "(tall)" so the variable-extent mix is visible on
/// screen, not just measurable.
fn row_view(index: usize, accent: Color, muted: Color) -> AnyView<PlaygroundState> {
    let tall = index.is_multiple_of(TALL_ROW_PERIOD);
    let label = if tall {
        format!("Row {index} (tall)")
    } else {
        format!("Row {index}")
    };
    any(
        SizedBox::<PlaygroundState>(None, Some(row_height(index))).child(Padding(
            EdgeInsets::symmetric(12.0, 8.0),
            text(label)
                .size(12.0)
                .color(if tall { accent } else { muted }),
        )),
    )
}

/// See the page-fn contract in [`crate::pages`].
pub fn page(state: &PlaygroundState) -> AnyView<PlaygroundState> {
    let theme = use_context::<Theme>().unwrap_or_else(frust_material::baseline);
    let scheme = theme.scheme();
    let accent = scheme.primary;
    let muted = scheme.on_surface_variant;

    let key_of = move |index: usize| ChildKey::new(index);
    let builder = move |index: usize| row_view(index, accent, muted);
    let list = ListView::builder_keyed(ROW_COUNT, ROW_EXTENT, key_of, builder)
        .estimated_item_extent(ROW_EXTENT)
        .controller(state.scroll_controller.clone());
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

    any(Padding(
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
            ),
    ))
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
}
