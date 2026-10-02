//! The section pages, one module per [`SECTION_LABELS`](crate::SECTION_LABELS)
//! entry, plus [`common`] (the chrome and helpers they share):
//!
//! | Label | Module | Demonstrates |
//! |---|---|---|
//! | Controls | [`controls`] | the six base builders beside frust-drawn peers, write-back, theme toggle |
//! | macOS | [`macos`] | the AppKit arm: class map, documented AppKit gaps, a live `Cover`/`Contain` image pair |
//! | New | [`new_controls`] | spinner, segmented, stepper, date picker |
//! | Alerts | [`alerts`] | native alerts + an anchored action sheet, Busy, programmatic dismiss |
//! | TabBar | [`tab_bar`] | a bare native tab bar driving a page-local index |
//! | Sheet | [`sheet`] | the native page sheet: detents, grabber, non-dismissible, programmatic control |
//! | Composite | [`composite`] | the plugin's `DemoCard` composite and its events, behind a toggle |
//! | Stress | [`stress`] | the live-slot readout, mount/unmount cycler and 50-slot stress toggle |
//!
//! Each page states its documented at-rest native-slot count in its header
//! ([`common::AT_REST_SLOTS`]).
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
//! only *read* signals; every write happens in an event handler — a frust
//! `on_*` callback or a native control's listener.

pub mod alerts;
pub mod common;
pub mod composite;
pub mod controls;
pub mod macos;
pub mod new_controls;
pub mod sheet;
pub mod stress;
pub mod tab_bar;

use frust::AnyView;

use crate::{NativeWidgetsDemoState, SECTION_LABELS};

pub use common::{AT_REST_SLOTS, at_rest_slots};

/// One-line summary per section, index-aligned with
/// [`SECTION_LABELS`](crate::SECTION_LABELS) — what each page catalogs.
pub const SECTION_SUMMARIES: [&str; SECTION_LABELS.len()] = [
    "The six base controls - button, label, switch, slider, progress, image - \
     native beside frust-drawn.",
    "The AppKit arm: which class each builder mounts on macOS and the gaps it documents.",
    "Spinner, segmented control, stepper and date picker.",
    "Native alerts and an anchored action sheet.",
    "The native tab bar (UITabBar on iOS/iPadOS).",
    "The native page sheet (UISheetPresentationController on iOS/iPadOS).",
    "A native component subtree and its events.",
    "Live-slot readout, mount/unmount cycler and a 50-slot stress toggle.",
];

/// Dispatch to the page for `section`, falling back to the first section for
/// any out-of-range index (defensive — the navigation only ever yields a
/// valid index).
pub fn current(section: usize, state: &NativeWidgetsDemoState) -> AnyView<NativeWidgetsDemoState> {
    match section {
        1 => macos::page(state),
        2 => new_controls::page(state),
        3 => alerts::page(state),
        4 => tab_bar::page(state),
        5 => sheet::page(state),
        6 => composite::page(state),
        7 => stress::page(state),
        _ => controls::page(state),
    }
}
