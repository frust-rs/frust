//! Cupertino (iOS) widget catalog (Phase 6c, PLAN.md D5): the Flutter-parity
//! counterparts to a subset of [`crate::material`] — no Cupertino equivalent
//! exists for FAB/Chips/Card, so none are built here (documented, PLAN.md D5).
//!
//! # Module map
//!
//! Every catalog module is pre-declared here so the wave-4 task that fills
//! them adds its code to each file directly and never edits this module list
//! (task 01, PLAN.md D6 same-file discipline):
//!
//! * [`navbar`] — `CupertinoNavBar` (44pt) (task 13). A doc-only stub today.
//! * [`tabbar`] — `CupertinoTabBar` (49pt content height) (task 13). A
//!   doc-only stub today.
//! * [`switch`] — `CupertinoSwitch` (51×31pt, community value, flagged)
//!   (task 13). A doc-only stub today.
//! * [`alert_dialog`] — `CupertinoAlertDialog` (task 13). A doc-only stub
//!   today.
//! * [`action_sheet`] — `CupertinoActionSheet` (task 13). A doc-only stub
//!   today.
//! * [`activity_indicator`] — `CupertinoActivityIndicator` (20pt) (task 13).
//!   A doc-only stub today.

pub mod action_sheet;
pub mod activity_indicator;
pub mod alert_dialog;
pub mod navbar;
pub mod switch;
pub mod tabbar;
