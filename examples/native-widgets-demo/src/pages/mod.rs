//! The section pages, one per native-widget family, in
//! [`SECTION_LABELS`](crate::SECTION_LABELS) order.
//!
//! Every section is a placeholder today — it renders its label and a
//! one-line summary of the family it will catalog. Each family's page lands
//! as its own module here, replacing that section's [`placeholder`] arm in
//! [`current`].
//!
//! # Page-fn contract (fixed across every section)
//!
//! Each section module exposes exactly:
//!
//! ```ignore
//! pub fn page(state: &NativeWidgetsDemoState) -> AnyView<NativeWidgetsDemoState>;
//! ```
//!
//! - **Argument**: the shared reactive handle the shell owns
//!   ([`NativeWidgetsDemoState`]). `state.brightness` is the one theme
//!   signal; `state.toasts` is the FIFO toast queue (append **inside an
//!   event handler**, never during `build`); `state.nav` is the shared
//!   navigator for pushed overlays.
//! - **Return**: a type-erased [`AnyView`]. The shell wraps it in a
//!   [`scroll_view`](frust::scroll_view), so a page returns its content column
//!   directly (no outer scroll of its own).
//! - **Placement rule**: a native control and its frust-drawn peer sit side by
//!   side under the same theme and the same signal, so a device gate can
//!   compare them in one frame.
//!
//! **Reactivity rule** (see `docs/CODE_STANDARDS.md`): a page's `build` may
//! only *read* signals; every write happens in an event handler.

use frust::{AnyView, Axis, EdgeInsets, FlexView, Padding, SizedBox, Theme, any, inflexible, text};

use crate::{NativeWidgetsDemoState, SECTION_LABELS};

/// One-line summary per section, index-aligned with
/// [`SECTION_LABELS`](crate::SECTION_LABELS) — what each page catalogs.
pub const SECTION_SUMMARIES: [&str; SECTION_LABELS.len()] = [
    "The six base controls - button, label, switch, slider, progress, image - \
     native beside frust-drawn.",
    "The same controls as AppKit views on macOS.",
    "Spinner, segmented control, stepper and date picker.",
    "Native alert and action sheet.",
    "The native tab bar (UITabBar on iOS/iPadOS).",
    "The native page sheet (UISheetPresentationController on iOS/iPadOS).",
    "A native component subtree and its events.",
    "A 50-slot stress harness of native controls.",
];

/// Dispatch to the page for `section`, falling back to the first section for
/// any out-of-range index (defensive — the navigation only ever yields a
/// valid index).
pub fn current(section: usize, state: &NativeWidgetsDemoState) -> AnyView<NativeWidgetsDemoState> {
    let section = if section < SECTION_LABELS.len() {
        section
    } else {
        0
    };
    placeholder(section, state)
}

/// Page padding, logical px.
const PAGE_PADDING_PX: f64 = 16.0;

/// Gap between the placeholder's heading and its summary, logical px.
const HEADING_GAP_PX: f64 = 8.0;

/// A section awaiting its catalog: the label as a theme-accented heading over
/// the section summary (theme `on_surface`) and a muted status line. Every
/// color resolves from the live theme, so it tracks the brightness toggle.
fn placeholder(section: usize, _state: &NativeWidgetsDemoState) -> AnyView<NativeWidgetsDemoState> {
    let theme = frust::use_context::<Theme>().unwrap_or_else(frust_glyph::baseline);
    let scheme = theme.scheme();
    let accent = scheme.primary;
    let muted = scheme.on_surface_variant;

    any(Padding(
        EdgeInsets::all(PAGE_PADDING_PX),
        FlexView::new(
            Axis::Vertical,
            vec![
                inflexible(any(text(SECTION_LABELS[section]).size(22.0).color(accent))),
                inflexible(any(SizedBox(None, Some(HEADING_GAP_PX)))),
                inflexible(any(text(SECTION_SUMMARIES[section]).size(14.0))),
                inflexible(any(SizedBox(None, Some(HEADING_GAP_PX)))),
                inflexible(any(text("This page's catalog is not built yet.")
                    .size(12.0)
                    .color(muted))),
            ],
        ),
    ))
}
