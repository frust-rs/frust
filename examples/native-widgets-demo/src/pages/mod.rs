//! The tab bodies, one module per [`Tab`](crate::Tab), the pushed detail page,
//! the per-family section modules they are built from, and [`common`] (the
//! helpers they share):
//!
//! | Tab | Module | Shows |
//! |---|---|---|
//! | Home | [`home`] | work rows pushing [`detail`] pages |
//! | Controls | [`controls_tab`] | [`controls`], [`new_controls`], [`composite`] |
//! | Dialogs | [`dialogs`] | [`alerts`], [`sheet`] and the chrome's menus |
//! | Lab | [`lab`] | [`macos`], [`tab_bar`], [`stress`], [`sandwich`] |
//!
//! # Page-fn contract
//!
//! A tab body is `pub fn page(state: &NativeWidgetsDemoState) ->
//! AnyView<NativeWidgetsDemoState>`, the root page of that tab's navigator.
//! It owns its scroll: a [`scroll_view`](frust::scroll_view) on
//! [`state.scroll(tab)`](crate::NativeWidgetsDemoState::scroll), with the safe
//! area INSIDE the scroller so content scrolls under floating bars. It never
//! sets chrome: the shell derives the bar from the tab's route observer, and a
//! pushed page carries its bar title as a [`DetailRoute`](crate::DetailRoute).
//!
//! **Reactivity rule** (see `docs/CODE_STANDARDS.md`): a page's `build` may
//! only *read* signals; every write happens in an event handler.

pub mod alerts;
pub mod common;
pub mod composite;
pub mod controls;
pub mod controls_tab;
pub mod detail;
pub mod dialogs;
pub mod home;
pub mod lab;
pub mod macos;
pub mod new_controls;
pub mod sandwich;
pub mod sheet;
pub mod stress;
pub mod tab_bar;

use frust::AnyView;

use crate::{NativeWidgetsDemoState, SECTION_LABELS, Tab};

pub use common::{AT_REST_SLOTS, SECTION_AT_REST_SLOTS, at_rest_slots};

/// One-line summary per tab, by [`Tab::index`].
pub const TAB_SUMMARIES: [&str; Tab::COUNT] = [
    "Native chrome around a frust-drawn home: rows push detail pages.",
    "Every native control beside its frust-drawn peer, one signal each.",
    "Native alerts, sheets and the chrome's own pull-down menus.",
    "Slot accounting, the stress harness, sandwich paint and the macOS reference.",
];

/// One-line summary per section, index-aligned with
/// [`SECTION_LABELS`](crate::SECTION_LABELS).
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

/// The page of one section on its own, falling back to the first section for
/// an out-of-range index — the per-section view the headless gate tests
/// mount, and what the tab bodies stack.
pub fn current(section: usize, state: &NativeWidgetsDemoState) -> AnyView<NativeWidgetsDemoState> {
    match section {
        1 => macos::page(state),
        2 => new_controls::page(state),
        3 => alerts::page(state),
        4 => tab_bar::page(state),
        5 => sheet::page(state),
        6 => composite::page(state),
        7 => stress::page(state),
        crate::SANDWICH_SECTION => sandwich::page(state),
        _ => controls::page(state),
    }
}
