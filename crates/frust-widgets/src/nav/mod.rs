//! Imperative navigation: a retained page stack with push/pop/replace
//! and per-page result callbacks, opaque-page paint culling, and test-pinned
//! capture/focus/IME page-switch semantics.
//!
//! # Module map
//!
//! Every navigation module is pre-declared here so each module's code lives
//! in its own file and never spills into this module list:
//!
//! * [`navigator`] — the page stack, [`NavigatorWidget`](navigator::NavigatorWidget),
//!   opaque-page paint culling, and the page-switch capture/focus/IME contract.
//!
//!   Its five **private** sibling files below hold the rest of the navigator;
//!   `navigator` re-exports every public name they define, so `nav::navigator::*`
//!   stays the one public path to all of them and the split adds no new API:
//!   - `options` — the page/callback type aliases plus `PushOptions`/
//!     `ReplaceOptions`/`BackPolicy`/`PopResult`/`PageVisibility`/`NavOp`/
//!     `NavigatorId`.
//!   - `controller` — `NavigatorController`, the op-recording app-state handle
//!     and its published read seams.
//!   - `view` — `NavigatorView` and its `navigator`/`overlay_host` constructors
//!     (the `View` impl itself lives with the widget core).
//!   - `ambient` — the two thread-local ambient scopes (page back-reach **R23**,
//!     swipe claim **R-B3-inner**).
//!   - `edge_swipe` — the interactive edge-swipe back gesture: tuning constants,
//!     gesture state, and the arm/begin/drive/settle driver methods.
//! * [`transition`] — page-transition animations.
//! * [`hero`] — shared-element ("hero") transition wrapper: the
//!   [`hero(tag, child)`](hero::hero) view that morphs a tagged element between
//!   two pages during a transition.
//! * [`router`] — declarative route table (a go_router-subset layer).
//! * [`path`] — path/URL parsing for the router.
//! * [`route`] — [`RouteNavigator`](route::RouteNavigator), the `Send + Sync`
//!   navigation-request queue a screen reaches through `provide_context` (the
//!   router itself cannot ride context — it holds `Rc`s).
//! * [`route_state`] — [`RouteStack`](route_state::RouteStack)/
//!   [`NavChange`](route_state::NavChange): the signal-free route-state
//!   observable published by [`navigator`]'s `publish_state` after every
//!   committed stack mutation.

mod ambient;
mod controller;
mod edge_swipe;
pub mod hero;
pub mod navigator;
mod options;
pub mod path;
pub mod route;
pub mod route_state;
pub mod router;
pub mod transition;
mod view;
