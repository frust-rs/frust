//! Component-scoped controller lifecycle and context sharing.
//!
//! [`use_controller`] ties an app controller's lifetime to the current reactive
//! owner: it is built once when the component mounts and
//! [`ControllerCore::dispose`]d exactly once when the component unmounts —
//! replacing the manual `on_cleanup` juggling components otherwise hand-roll.
//!
//! [`provide_controller`] / [`expect_controller`] are thin context wrappers for
//! *app-scoped* controllers that outlive any single component. Controllers
//! placed in context are **never** disposed by the components that read them;
//! the provider owns their lifecycle.

use std::sync::Arc;

use clean_signals::{ControllerCore, Failure};
use frust::{on_cleanup, provide_context, use_context};

/// Builds an app controller once and disposes it on component teardown.
///
/// `factory` runs a single time (when the owning
/// [`Component`](frust::Component) first builds, inside
/// [`Component::init`](frust::Component::init)). The returned controller is
/// wrapped in an [`Arc`] so the render tree and any spawned tasks can share a
/// cheap handle, and its embedded [`ControllerCore`] is
/// [`dispose`](ControllerCore::dispose)d via
/// [`on_cleanup`](frust::on_cleanup) when the component unmounts — aborting
/// in-flight watches, running registered teardowns, and releasing owned signals.
///
/// The controller must expose its [`ControllerCore`] by `AsRef` (embed-by-
/// composition, the clean-signals idiom) and be `Send + Sync` because
/// [`on_cleanup`](frust::on_cleanup)'s callback is `Send + Sync` — an
/// `Arc<C>` is captured, not an `Rc<C>`, since Frust's reactive owner may run
/// cleanups off the construction thread (unlike leptos's single-threaded browser
/// owner, where this hook returns an `Rc`-backed `StoredValue`).
///
/// # Ambient-Owner contract
///
/// Must be called where a reactive [`Owner`](reactive_graph::owner::Owner) is
/// ambient — i.e. from `Component::init`. Outside an owner, the disposal
/// `on_cleanup` silently no-ops and the controller leaks.
///
/// # Type parameters
///
/// - `C`: the app controller, exposing its [`ControllerCore`] via
///   `AsRef` (`impl AsRef<ControllerCore<F>> for MyController`).
/// - `F`: the app's [`Failure`] type.
///
/// # Example
///
/// ```rust,ignore
/// fn init(&self) -> Arc<UsersController> {
///     use_controller::<UsersController, AppFailure>(|| UsersController::new(repo))
/// }
/// ```
pub fn use_controller<C, F>(factory: impl FnOnce() -> C) -> Arc<C>
where
    C: AsRef<ControllerCore<F>> + Send + Sync + 'static,
    F: Failure + Clone,
{
    let controller = Arc::new(factory());
    let handle = Arc::clone(&controller);
    on_cleanup(move || AsRef::<ControllerCore<F>>::as_ref(&*handle).dispose());
    controller
}

/// Provides an app-scoped controller through Frust's reactive context.
///
/// Use this for controllers whose lifetime spans a large subtree rather than a
/// single component. The provider is responsible for disposal; components that
/// [`expect_controller`] must **not** dispose it.
///
/// Controllers are typically shared as `Arc<C>` (context requires `Send + Sync`,
/// so use `Arc`, not `Rc`) so [`expect_controller`] can hand out cheap clones.
/// Provided under the current owner, the value is visible to that owner and its
/// descendants — call this from `Component::init`.
pub fn provide_controller<C: Send + Sync + 'static>(controller: C) {
    provide_context(controller);
}

/// Retrieves an app-scoped controller previously supplied by
/// [`provide_controller`] in an ancestor component.
///
/// Traverses the reactive ownership graph from the current owner upward and
/// clones the first controller of type `C` it finds. Components must **not**
/// dispose the returned controller — the provider owns its lifecycle.
///
/// # Panics
///
/// Panics if no controller of type `C` is in context (the same contract as
/// leptos `expect_context`), with the message
/// `"expect_controller: no controller of the requested type is provided in
/// context; call provide_controller in an ancestor component first"`.
pub fn expect_controller<C: Clone + 'static>() -> C {
    use_context::<C>().expect(
        "expect_controller: no controller of the requested type is provided in context; \
         call provide_controller in an ancestor component first",
    )
}
