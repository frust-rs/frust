//! Material 3 Expressive widget catalog: AppBar, Card, Chips, Dialog, FAB,
//! ListItem, NavigationBar, BottomSheet, Switch, and progress
//! indicators, plus the [`state_layer`] helper they share.
//!
//! # Module map
//!
//! Every catalog module is pre-declared here so each module's code lives in
//! its own file and never spills into this module list. All are implemented;
//! the crate root (`frust-widgets/src/lib.rs`) flat re-exports every widget +
//! spec type below, so app code never names this module directly:
//!
//! * [`state_layer`] — the shared M3 interaction-state overlay helper
//!   (hover/focus/pressed/dragged).
//! * [`switch`] — the Switch toggle.
//! * [`chips`] — assist + filter Chips.
//! * [`fab`] — the FloatingActionButton, regular/small/large/extended.
//! * [`card`] — elevated/filled/outlined Card.
//! * [`appbar`] — the small, center-aligned top AppBar.
//! * [`navbar`] — the bottom NavigationBar.
//! * [`progress`] — linear + circular progress indicators.
//! * [`dialog`] — the basic modal Dialog.
//! * [`sheet`] — the modal BottomSheet.
//! * [`list_view`] — **deprecated** compatibility shim: `ListView` moved to
//!   the baseline widget set (`frust_widgets::list_view`, lazy-list 05) — kept
//!   only so `frust_widgets::material::list_view::…` still resolves.
//! * [`list_item`] — 1/2/3-line ListItem rows.
//! * [`shape_morph`] — rounded-polygon path interpolation primitive (M3
//!   Expressive).
//! * [`loading_indicator`] — the M3 Expressive morphing loading indicator.
//! * [`button_group`] — the M3X connected, single-select button group (its
//!   pressed-member emphasis is [`shape_morph`]'s second consumer).
//! * [`split_button`] — the M3X split button (leading action + trailing menu,
//!   with a rotating chevron).
//! * [`fab_menu`] — the M3X FAB menu (a trigger FAB that reveals a vertical
//!   stack of large menu items), replacing the speed-dial pattern.
//! * [`toolbar`] — the M3X floating/docked toolbar (leading/center/trailing
//!   slots plus an optional fab slot).

pub mod appbar;
pub mod button_group;
pub mod card;
pub mod chips;
pub mod dialog;
pub mod fab;
pub mod fab_menu;
pub mod list_item;
pub mod loading_indicator;
pub mod navbar;
pub mod progress;
pub mod shape_morph;
pub mod sheet;
pub mod split_button;
pub mod state_layer;
pub mod switch;
pub mod toolbar;

/// Deprecated compatibility shim: `ListView` moved to the baseline widget set
/// (`frust_widgets::list_view`; `frust::ListView`/`ListViewWidget`/`list_view`
/// via the facade — lazy-list 05, Ed's decided placement, 2026-07-30). Kept
/// only so `frust_widgets::material::list_view::…` still resolves for an
/// existing caller.
///
/// Each item below is individually `#[deprecated]` rather than the module
/// itself, and each is a type alias / thin wrapper rather than a `pub use` —
/// `#[deprecated]` attached to a `use` re-export does not surface a warning
/// at the *use site* going through that re-export (a confirmed rustc
/// limitation, not a mistake here); a concrete item carrying its own
/// `#[deprecated]` does.
pub mod list_view {
    /// Deprecated: moved to [`crate::list_view::ListView`].
    #[deprecated(
        note = "ListView moved to the baseline widget set — use `frust_widgets::list_view::ListView` (or `frust::ListView` via the facade) instead"
    )]
    pub type ListView<State> = crate::list_view::ListView<State>;

    /// Deprecated: moved to [`crate::list_view::ListViewWidget`].
    #[deprecated(
        note = "ListView moved to the baseline widget set — use `frust_widgets::list_view::ListViewWidget` (or `frust::ListViewWidget` via the facade) instead"
    )]
    pub type ListViewWidget = crate::list_view::ListViewWidget;

    /// Deprecated: moved to [`crate::list_view::list_view`].
    #[deprecated(
        note = "ListView moved to the baseline widget set — use `frust_widgets::list_view::list_view` (or `frust::list_view` via the facade) instead"
    )]
    pub fn list_view<State: 'static>(
        item_count: usize,
        item_extent: f64,
        builder: impl Fn(usize) -> frust_core::AnyView<State> + 'static,
    ) -> crate::list_view::ListView<State> {
        crate::list_view::list_view(item_count, item_extent, builder)
    }
}
