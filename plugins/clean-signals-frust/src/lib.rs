//! clean-signals-frust — the Frust bridge for `clean-signals`.
//!
//! This is the Frust counterpart of [`clean-signals-leptos`]: the crate
//! where the framework meets a rendering runtime, so the core [`clean_signals`]
//! crate stays presentation-free. It supplies the helpers a Frust
//! presentation layer needs to drive controllers safely from components:
//!
//! - [`use_controller`] — build a controller once, dispose it on unmount.
//! - [`provide_controller`] / [`expect_controller`] — share an app-scoped
//!   controller through Frust's reactive context.
//! - [`use_failure_listener`] — surface a controller's failures, scoped to the
//!   component.
//! - [`async_view`] — map a controller's [`AsyncState`](clean_signals::AsyncState)
//!   snapshot to a view.
//! - [`use_interval`] — run a callback on a timer for as long as a component
//!   is mounted.
//!
//! [`clean-signals-leptos`]: https://github.com/f0x-it-llc/clean-signals-rs
//!
//! # Two contracts every helper here depends on
//!
//! ## 1. Ambient-Owner requirement
//!
//! [`use_controller`] and [`use_failure_listener`] register an
//! [`on_cleanup`](frust::on_cleanup) callback, and [`provide_controller`] /
//! [`expect_controller`] read and write the reactive context. All four **only**
//! work under an ambient reactive [`Owner`](reactive_graph::owner::Owner):
//! outside one, `on_cleanup` silently no-ops (disposal never runs) and context
//! reads/writes go nowhere. In practice this means **call these helpers from
//! [`Component::init`](frust::Component::init)** (or another owner scope) —
//! Frust runs each component's `init` under that component's own reactive
//! owner, so cleanups bind to *that component's* teardown and context is scoped
//! to its subtree. Frust guarantees an ambient root owner on all entry paths.
//!
//! ## 2. Coarse-grained reactivity model
//!
//! Unlike leptos, where a view is a fine-grained reactive closure that re-runs
//! only the affected fragment, a Frust [`Component`](frust::Component)
//! re-runs its **entire** `build` whenever its tracked scope dirties. A
//! controller therefore does not push individual DOM updates; it writes an
//! `AsyncState` (or other signal), the shell re-renders the component's `build`,
//! and `build` reads the current snapshot with a tracked `.get()`. Callers must
//! do that tracked `.get()` themselves inside `build` — the helpers here are
//! plain functions over already-read values, not reactive wrappers.
//!
//! # The page-controller pattern
//!
//! A controller (view model) embeds a
//! [`ControllerCore`](clean_signals::ControllerCore) by composition and exposes
//! its state signals; a component wires it up with the helpers below. Frust's
//! `examples/inbox` is the end-to-end reference wiring.
//!
//! ```rust,ignore
//! use clean_signals::{ControllerCore, async_state_signal, AsyncState};
//! use clean_signals_frust::{use_controller, use_failure_listener};
//! use frust::{AnyView, Component, any, text};
//! use std::sync::Arc;
//!
//! impl Component for InboxScreen {
//!     type State = Arc<InboxController>;
//!
//!     fn init(&self) -> Arc<InboxController> {
//!         // An Owner is ambient here, so disposal binds to this component's
//!         // teardown.
//!         let controller = use_controller::<InboxController, AppFailure>(
//!             InboxController::new,
//!         );
//!         use_failure_listener(controller.core().failures(), move |f: AppFailure| {
//!             log::error!("{}", f.user_message());
//!         });
//!         controller
//!     }
//!
//!     fn build(&self, state: &mut Arc<InboxController>) -> AnyView<Arc<InboxController>> {
//!         // Tracked read: re-runs `build` when the controller writes a new state.
//!         match state.messages.get() { /* ... */ }
//!     }
//! }
//! ```

pub mod async_view;
pub mod failure_listener;
pub mod hooks;
pub mod interval;

pub use async_view::async_view;
pub use failure_listener::use_failure_listener;
pub use hooks::{expect_controller, provide_controller, use_controller};
pub use interval::use_interval;
