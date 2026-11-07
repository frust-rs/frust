//! `navigator.rs`'s test suite, split by theme. `support` carries
//! fixtures/harnesses shared by more than one theme; every other child
//! module is one theme, split along the original `#[cfg(test)] mod tests`
//! section-divider comments (stack/ops, back-request routing, page
//! transitions, edge-swipe gesture, page visibility, route-state
//! observable, root-overlay-host semantics, and keyed page-list
//! reconciliation).

mod support;

mod back;
mod edge_swipe;
mod route_state;
mod semantics;
mod set_pages;
mod stack;
mod transition;
mod visibility;
