//! **Sandwich** — frust content on both sides of a native view: painted
//! under it, and painted over it.
//!
//! Two native slots, each with frust paint beneath and above:
//!
//! - a **native field** (a native label standing in for a text input — the
//!   plugin ships no native text field) sitting in a frust frame, with a
//!   frust **selection toolbar** across its top edge and a frust **popover**
//!   over its lower half;
//! - a **native slider** inside a frust **card**, with a frust **badge** over
//!   the right end of its track.
//!
//! # Surface modes
//!
//! On a translucent surface (Mode B, native views composited *below* frust)
//! each slot's hole punch erases the frust paint under the view at the view's
//! own paint position, and everything painted after it lands on top — the
//! sandwich holds with one surface. On an opaque surface (Mode A — Android's
//! default, macOS always) the native views sit *above* frust, so the toolbar,
//! popover and badge hide behind them unless the shell draws the content
//! painted after a view onto an overlay surface stacked above it
//! (`FRUST_OVERLAYS=1` on Android).
//!
//! # Paint order
//!
//! By default everything under the views is painted first, then both views,
//! then everything over them: one cut, so one overlay, which every host can
//! stack correctly. "Interleaved paint order" paints the slider's card
//! *between* the two views instead — two overlays, the first carrying the
//! card. An Android overlay `SurfaceView` stacks above every native view
//! (`setZOrderOnTop`), so there the card's overlay covers the slider: the
//! documented limit of on-top overlays, shown on purpose.
//!
//! # At rest
//!
//! Two native slots on every target, whichever paint order is on.

use frust::{
    AnyView, ButtonStyle, EdgeInsets, FlexChild, Get, Padding, Set, SizedBox, Stack, View, either,
    button, checkbox, container, inflexible, text,
};
use frust_native_widgets::{live_slot_count, native_label, native_slider};

use super::common::{
    PAIR_CELL_W, PLATFORM, S, accent, block, bump, caption, chip, gap, label, local_sig,
    page_column, readout, row, theme,
};

/// This page's one-line summary, the header's caption.
pub const SANDWICH_SUMMARY: &str = "Frust paint under and over native views: a popover and a \
                                    selection toolbar over a native field, a badge over a native \
                                    slider in a frust card.";

/// Native slots this page publishes at rest, on every target.
pub const SANDWICH_AT_REST_SLOTS: usize = 2;

local_sig!(slider_value_sig, i32, 40);
local_sig!(interleaved_sig, bool, false);
local_sig!(live_refresh_sig, u32, 0);

/// The native field's size — a single-line text field's.
const FIELD_W: f64 = 2.0 * PAIR_CELL_W;
const FIELD_H: f64 = 44.0;
/// The frust frame showing around the field (painted under it).
const FRAME: f64 = 4.0;
/// The selection toolbar's height; it straddles the field's top edge.
const TOOLBAR_H: f64 = 32.0;
/// Where the field's frame starts: half the toolbar above it.
const FIELD_TOP: f64 = TOOLBAR_H / 2.0;
/// Where the popover starts: over the field's lower third.
const POPOVER_TOP: f64 = FIELD_TOP + FRAME + FIELD_H * 2.0 / 3.0;
/// The slider card's position and size.
const CARD_TOP: f64 = 140.0;
const CARD_W: f64 = FIELD_W + 2.0 * FRAME;
const CARD_H: f64 = 96.0;
/// The native slider inside the card.
const SLIDER_LEFT: f64 = 16.0;
const SLIDER_TOP: f64 = CARD_TOP + 40.0;
const SLIDER_H: f64 = 40.0;

/// `child` placed at `(x, y)` inside the page's sandwich stack.
fn at(x: f64, y: f64, child: impl View<S>) -> impl View<S> {
    Padding(
        EdgeInsets {
            left: x,
            top: y,
            right: 0.0,
            bottom: 0.0,
        },
        child,
    )
}

/// The frust frame the native field sits in — painted under the field, so a
/// ring of it shows around the field's edge.
fn field_frame() -> impl View<S> {
    container(SizedBox(
        Some(FIELD_W + 2.0 * FRAME),
        Some(FIELD_H + 2.0 * FRAME),
    ))
    .fill(accent())
    .radius(8.0)
}

/// The native field.
fn field() -> impl View<S> {
    native_label("Native field \u{2014} frust paints under and over me")
        .content_description("Native field between frust layers (sandwich)")
        .size(FIELD_W, FIELD_H)
}

/// A frust selection toolbar across the field's top edge: real frust buttons,
/// so a tap on one over the native view is a frust tap.
fn selection_toolbar() -> impl View<S> {
    let action = |name: &'static str| {
        inflexible(
            button(name, move |state: &mut S| {
                crate::push_toast(state.toasts, format!("Sandwich: {name}"));
            })
            .small()
            .style(ButtonStyle::Ghost),
        )
    };
    container(SizedBox(None, Some(TOOLBAR_H)).child(Padding(
        EdgeInsets::symmetric(4.0, 0.0),
        row(vec![action("Cut"), action("Copy"), action("Paste")]),
    )))
    .fill(theme().scheme().inverse_surface)
    .radius(6.0)
}

/// A frust popover over the field's lower third, running past it.
fn popover() -> impl View<S> {
    let theme = theme();
    let scheme = theme.scheme();
    container(Padding(
        EdgeInsets::symmetric(12.0, 8.0),
        text("frust popover \u{2014} painted after the field, so over it")
            .size(12.0)
            .color(scheme.inverse_on_surface),
    ))
    .fill(scheme.inverse_surface.with_alpha(0.92))
    .radius(10.0)
}

/// The frust card the native slider sits in — painted under the slider.
fn slider_card() -> impl View<S> {
    container(SizedBox(Some(CARD_W), Some(CARD_H)))
        .fill(theme().scheme().surface_container_high)
        .radius(12.0)
}

/// The card's title, frust text inside the card beside nothing native.
fn card_title(value: i32) -> impl View<S> {
    text(format!("Volume (frust card) \u{2014} {value}"))
        .size(13.0)
        .color(theme().scheme().on_surface)
}

/// The native slider, written back through the page's signal.
fn slider(value: i32) -> impl View<S> {
    native_slider(value, 0, 100)
        .content_description("Native slider inside a frust card (sandwich)")
        .size(CARD_W - 2.0 * SLIDER_LEFT, SLIDER_H)
        .on_change(|requested| slider_value_sig().set(requested.clamp(0, 100)))
}

/// A frust badge over the slider track's right end.
fn badge(value: i32) -> impl View<S> {
    let theme = theme();
    let scheme = theme.scheme();
    container(Padding(
        EdgeInsets::symmetric(8.0, 3.0),
        text(format!("{value}%"))
            .size(11.0)
            .color(scheme.on_primary),
    ))
    .fill(scheme.primary)
    .radius(10.0)
}

/// The sandwich: one stack, absolutely placed, whose child ORDER is the paint
/// order — see the module docs' *Paint order*.
fn sandwich(value: i32, interleaved: bool) -> impl View<S> {
    let badge_at = |child| at(CARD_W - 72.0, SLIDER_TOP - 8.0, child);
    either(
        interleaved,
        // Field group, then card group: the card is painted between the views.
        || Stack((
            at(0.0, FIELD_TOP, field_frame()),
            at(FRAME, FIELD_TOP + FRAME, field()),
            at(24.0, 0.0, selection_toolbar()),
            at(40.0, POPOVER_TOP, popover()),
            at(0.0, CARD_TOP, slider_card()),
            at(SLIDER_LEFT, CARD_TOP + 12.0, card_title(value)),
            at(SLIDER_LEFT, SLIDER_TOP, slider(value)),
            badge_at(badge(value)),
        )),
        // Everything under, both views, everything over.
        || Stack((
            at(0.0, FIELD_TOP, field_frame()),
            at(0.0, CARD_TOP, slider_card()),
            at(SLIDER_LEFT, CARD_TOP + 12.0, card_title(value)),
            at(FRAME, FIELD_TOP + FRAME, field()),
            at(SLIDER_LEFT, SLIDER_TOP, slider(value)),
            at(24.0, 0.0, selection_toolbar()),
            at(40.0, POPOVER_TOP, popover()),
            badge_at(badge(value)),
        )),
    )
}

/// The live slot count against [`SANDWICH_AT_REST_SLOTS`] — only on a target
/// with a native host; a desktop preview has no native runtime and reads `0`.
fn live_rows() -> Vec<FlexChild<S>> {
    let _ = live_refresh_sig().get();
    let expected = if cfg!(any(
        target_os = "android",
        target_os = "ios",
        target_os = "macos"
    )) {
        SANDWICH_AT_REST_SLOTS
    } else {
        0
    };
    vec![
        inflexible(readout(format!(
            "Live native slots (process): {} \u{2014} expected at rest here: {expected}",
            live_slot_count()
        ))),
        gap(4.0),
        inflexible(chip("Re-read live count", |_: &mut S| {
            bump(live_refresh_sig())
        })),
    ]
}

/// See the page-fn contract in [`crate::pages`] and the [module docs](self).
pub fn page(_state: &S) -> AnyView<S> {
    let value = slider_value_sig().get();
    let interleaved = interleaved_sig().get();

    let mut header = vec![
        inflexible(text(crate::SANDWICH_LABEL).size(22.0).color(accent())),
        gap(6.0),
        inflexible(caption(SANDWICH_SUMMARY)),
        gap(6.0),
        inflexible(caption(format!("This build: {PLATFORM}."))),
        gap(4.0),
    ];
    header.extend(live_rows());

    let modes = block(vec![
        inflexible(label("What to look for")),
        gap(6.0),
        inflexible(caption(
            "The accent frame and the card are frust paint UNDER the native views; the \
             toolbar, the popover and the badge are frust paint OVER them. On a translucent \
             surface all of it shows with one surface. On an opaque surface (Android's \
             default) the over-paint hides behind the native views unless overlay surfaces \
             are on (FRUST_OVERLAYS=1).",
        )),
        gap(6.0),
        inflexible(checkbox(
            interleaved,
            "Interleaved paint order (two overlays)",
            |_: &mut S, on: bool| interleaved_sig().set(on),
        )),
        gap(4.0),
        inflexible(caption(
            "Interleaved paints the card between the two views. Android stacks every overlay \
             above every native view, so there the card's overlay covers the slider.",
        )),
    ]);

    let stack = block(vec![
        inflexible(sandwich(value, interleaved)),
        // Room for the popover's overhang past the stack's own extent.
        gap(8.0),
        inflexible(readout(format!("Slider (app value): {value}"))),
    ]);

    page_column(vec![block(header), modes, stack])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_geometry_sandwiches_each_view() {
        // The toolbar straddles the field's top edge, the popover overlaps its
        // lower part, and the badge overlaps the slider: each over-paint
        // actually covers its view.
        assert!(TOOLBAR_H > FIELD_TOP);
        assert!(POPOVER_TOP < FIELD_TOP + FRAME + FIELD_H);
        // The card encloses the slider, and the two groups do not overlap.
        assert!(SLIDER_TOP + SLIDER_H < CARD_TOP + CARD_H);
        assert!(POPOVER_TOP + 40.0 < CARD_TOP);
    }
}
