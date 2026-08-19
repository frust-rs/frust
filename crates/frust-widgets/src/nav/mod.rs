//! Imperative navigation: a retained page stack with push/pop/replace
//! and per-page result callbacks, opaque-page paint culling, and test-pinned
//! capture/focus/IME page-switch semantics.
//!
//! # Module map
//!
//! Every navigation module is pre-declared here so each module's code lives
//! in its own file and never spills into this module list:
//!
//! * [`navigator`] — the page stack, [`NavigatorController`](navigator::NavigatorController),
//!   opaque-page paint culling, and the page-switch capture/focus/IME contract.
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

pub mod hero;
pub mod navigator;
pub mod path;
pub mod route;
pub mod route_state;
pub mod router;
pub mod transition;
