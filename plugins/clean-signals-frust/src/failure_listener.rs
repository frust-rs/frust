//! Component-scoped subscription to a controller's [`FailureSink`].
//!
//! [`use_failure_listener`] wires a handler to a controller's failure events for
//! as long as the current component is mounted, then unsubscribes on cleanup —
//! the idiomatic replacement for swallowing errors or manually tracking a
//! [`Subscription`](clean_signals::Subscription) guard per component.

use clean_signals::{Failure, FailureSink};
use frust::on_cleanup;

/// Subscribes `handler` to `sink` for the lifetime of the current component.
///
/// Each emitted failure is cloned and passed to `handler` (by value). The
/// underlying [`Subscription`](clean_signals::Subscription) is moved into an
/// [`on_cleanup`](frust::on_cleanup) callback, so it is dropped — and the
/// listener removed — when the component unmounts.
///
/// # Ambient-Owner contract
///
/// Must be called where a reactive [`Owner`](reactive_graph::owner::Owner) is
/// ambient — i.e. from [`Component::init`](frust::Component::init). Outside an
/// owner the cleanup no-ops, so the subscription would leak for the sink's
/// lifetime rather than the component's.
///
/// # Handler contract
///
/// - The handler runs **outside** any reactive tracking scope: reading a signal
///   inside it does not subscribe the surrounding render. Use it to push a
///   snackbar, log, or `set` a signal imperatively.
/// - It must be `Send + Sync` because the emitting controller may run its use
///   cases on a worker thread (native), as [`FailureSink::subscribe`]'s bound
///   requires.
///
/// # Example
///
/// ```rust,ignore
/// use_failure_listener(controller.core().failures(), move |f: AppFailure| {
///     toasts.error(f.user_message());
/// });
/// ```
pub fn use_failure_listener<F>(sink: &FailureSink<F>, handler: impl Fn(F) + Send + Sync + 'static)
where
    F: Failure + Clone,
{
    let subscription = sink.subscribe(move |failure: &F| handler(failure.clone()));
    // Moving the guard into the cleanup closure keeps the subscription alive
    // until unmount, then drops it (unsubscribing).
    on_cleanup(move || drop(subscription));
}
