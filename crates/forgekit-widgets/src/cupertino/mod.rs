//! Cupertino (iOS) widget catalog (Phase 6c, PLAN.md D5): the Flutter-parity
//! counterparts to a subset of [`crate::material`] — no Cupertino equivalent
//! exists for FAB/Chips/Card, so none are built here (documented, PLAN.md D5).
//!
//! # Module map
//!
//! Every catalog module is pre-declared here so the wave-4 task that fills
//! them adds its code to each file directly and never edits this module list
//! (task 01, PLAN.md D6 same-file discipline). All are implemented; the
//! crate root (`forgekit-widgets/src/lib.rs`) flat re-exports every widget +
//! spec type below (task 14), so app code never names this module directly:
//!
//! * [`navbar`] — `CupertinoNavBar` (44pt). Implemented in task 13.
//! * [`tabbar`] — `CupertinoTabBar` (49pt content height). Implemented in
//!   task 13.
//! * [`switch`] — `CupertinoSwitch` (51×31pt, community value, flagged).
//!   Implemented in task 13.
//! * [`alert_dialog`] — `CupertinoAlertDialog`. Implemented in task 13.
//! * [`action_sheet`] — `CupertinoActionSheet`. Implemented in task 13.
//! * [`activity_indicator`] — `CupertinoActivityIndicator` (20pt).
//!   Implemented in task 13.
//! * [`button`] — `CupertinoButton` (kit-mined Small/Medium/Large size
//!   classes; Filled/Gray/Glass styles). Implemented in phase 6f task 12.

pub mod action_sheet;
pub mod activity_indicator;
pub mod alert_dialog;
pub mod button;
pub mod navbar;
pub mod switch;
pub mod tabbar;
