//! Cupertino (iOS) widget catalog: the Flutter-parity counterparts to a
//! subset of [`crate::material`] — no Cupertino equivalent exists for
//! FAB/Chips/Card, so none are built here.
//!
//! # Module map
//!
//! Every catalog module is pre-declared here so a new widget adds its code
//! to each file directly rather than editing this module list. All are
//! implemented; the crate root (`frust-widgets/src/lib.rs`) flat re-exports
//! every widget + spec type below, so app code never names this module
//! directly:
//!
//! * [`navbar`] — `CupertinoNavBar` (44pt).
//! * [`tabbar`] — `CupertinoTabBar` (49pt content height).
//! * [`switch`] — `CupertinoSwitch` (64×28pt, matching the current iOS UI Kit
//!   metric — an earlier revision used a community-approximate 51×31pt).
//! * [`alert_dialog`] — `CupertinoAlertDialog`.
//! * [`action_sheet`] — `CupertinoActionSheet`.
//! * [`activity_indicator`] — `CupertinoActivityIndicator` (20pt).
//! * [`button`] — `CupertinoButton` (kit-mined Small/Medium/Large size
//!   classes; Filled/Gray/Glass styles).

pub mod action_sheet;
pub mod activity_indicator;
pub mod alert_dialog;
pub mod button;
pub mod navbar;
pub mod switch;
pub mod tabbar;
