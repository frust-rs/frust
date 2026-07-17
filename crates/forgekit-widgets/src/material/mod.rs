//! Material 3 Expressive widget catalog (Phase 6c, PLAN.md D5): AppBar, Card,
//! Chips, Dialog, FAB, ListView/ListItem, NavigationBar, BottomSheet, Switch,
//! and progress indicators, plus the [`state_layer`] helper they share.
//!
//! # Module map
//!
//! Every catalog module is pre-declared here so the wave-3/4 tasks that fill
//! them each add their code to their own file and never edit this module
//! list (task 01, PLAN.md D6 same-file discipline):
//!
//! * [`state_layer`] — the shared M3 interaction-state overlay helper
//!   (hover/focus/pressed/dragged). Implemented in task 01.
//! * [`switch`] — the Switch toggle (task 07). A doc-only stub today.
//! * [`chips`] — assist + filter Chips (task 07). A doc-only stub today.
//! * [`fab`] — the FloatingActionButton, regular/small/large/extended (task 08).
//!   A doc-only stub today.
//! * [`card`] — elevated/filled/outlined Card (task 08). A doc-only stub today.
//! * [`appbar`] — the small, center-aligned top AppBar (task 09). A doc-only
//!   stub today.
//! * [`navbar`] — the bottom NavigationBar (task 09). A doc-only stub today.
//! * [`progress`] — linear + circular progress indicators (task 10). A
//!   doc-only stub today.
//! * [`dialog`] — the basic modal Dialog (task 11). A doc-only stub today.
//! * [`sheet`] — the modal BottomSheet (task 11). A doc-only stub today.
//! * [`list_view`] — the virtualized ListView (task 12). A doc-only stub today.
//! * [`list_item`] — 1/2/3-line ListItem rows (task 12). A doc-only stub today.

pub mod appbar;
pub mod card;
pub mod chips;
pub mod dialog;
pub mod fab;
pub mod list_item;
pub mod list_view;
pub mod navbar;
pub mod progress;
pub mod sheet;
pub mod state_layer;
pub mod switch;
