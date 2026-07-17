//! Material 3 Expressive widget catalog (Phase 6c, PLAN.md D5): AppBar, Card,
//! Chips, Dialog, FAB, ListView/ListItem, NavigationBar, BottomSheet, Switch,
//! and progress indicators, plus the [`state_layer`] helper they share.
//!
//! # Module map
//!
//! Every catalog module is pre-declared here so the wave-3/4 tasks that fill
//! them each add their code to their own file and never edit this module
//! list (task 01, PLAN.md D6 same-file discipline). All are implemented; the
//! crate root (`forgekit-widgets/src/lib.rs`) flat re-exports every widget +
//! spec type below (task 14), so app code never names this module directly:
//!
//! * [`state_layer`] — the shared M3 interaction-state overlay helper
//!   (hover/focus/pressed/dragged). Implemented in task 01.
//! * [`switch`] — the Switch toggle. Implemented in task 07.
//! * [`chips`] — assist + filter Chips. Implemented in task 07.
//! * [`fab`] — the FloatingActionButton, regular/small/large/extended.
//!   Implemented in task 08.
//! * [`card`] — elevated/filled/outlined Card. Implemented in task 08.
//! * [`appbar`] — the small, center-aligned top AppBar. Implemented in task
//!   09.
//! * [`navbar`] — the bottom NavigationBar. Implemented in task 09.
//! * [`progress`] — linear + circular progress indicators. Implemented in
//!   task 10.
//! * [`dialog`] — the basic modal Dialog. Implemented in task 11.
//! * [`sheet`] — the modal BottomSheet. Implemented in task 11.
//! * [`list_view`] — the virtualized ListView. Implemented in task 12.
//! * [`list_item`] — 1/2/3-line ListItem rows. Implemented in task 12.
//! * [`shape_morph`] — rounded-polygon path interpolation primitive. Added in
//!   Phase 6f (task 05).
//! * [`loading_indicator`] — the M3 Expressive morphing loading indicator.
//!   Added in Phase 6f (task 05).

pub mod appbar;
pub mod card;
pub mod chips;
pub mod dialog;
pub mod fab;
pub mod list_item;
pub mod list_view;
pub mod loading_indicator;
pub mod navbar;
pub mod progress;
pub mod shape_morph;
pub mod sheet;
pub mod state_layer;
pub mod switch;
