//! Material 3 Expressive widget catalog: AppBar, Card, Chips, Dialog, FAB,
//! ListView/ListItem, NavigationBar, BottomSheet, Switch, and progress
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
//! * [`list_view`] — the virtualized ListView.
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
pub mod list_view;
pub mod loading_indicator;
pub mod navbar;
pub mod progress;
pub mod shape_morph;
pub mod sheet;
pub mod split_button;
pub mod state_layer;
pub mod switch;
pub mod toolbar;
