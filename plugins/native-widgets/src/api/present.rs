//! The app-facing front door to native presentations (`crate::present`):
//! [`show_native_alert`], the awaitable form, and [`show_native_alert_into`],
//! the events-as-signals form (`super::signals`' idiom) that needs no
//! `async` block at all.
//!
//! # The contract both forms keep
//!
//! - **Exactly one outcome.** An accepted alert resolves one
//!   [`AlertOutcome`] — the chosen action's id, `Cancelled`, `Dismissed`
//!   (through [`crate::present::dismiss`]) or `HostLost` — and never a
//!   second: every platform callback after the first is dropped by the
//!   presentation's generation guard.
//! - **One at a time.** A request while another presentation is live is
//!   refused [`PresentError::Busy`] at once — never queued, never replacing
//!   the live one.
//! - **Nothing blocks.** Both forms return immediately; the outcome arrives
//!   when the user answers, driven by the platform's main thread.
//!
//! # Controlled, like every other native control
//!
//! [`show_native_alert_into`] *reports* the outcome by writing
//! `Some(outcome)` into the app's signal once, and never touches it again —
//! it never clears it, and never writes a second time. The app owns the
//! signal: it reads the outcome on its next rebuild, acts on it, and resets
//! it to `None` itself when it is ready to ask again (the controlled
//! convention of `docs/CODE_STANDARDS.md`'s Interaction Semantics and every
//! builder's `on_...` callback). A write wakes exactly one frust frame, like
//! a control's callback does.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};

use frust::{RwSignal, Set};

use crate::present::{
    self, AlertOutcome, AlertSpec, PresentError, Presentation, PresentationHandle,
};

/// Present a native alert and await its one outcome.
///
/// Resolves `Ok(outcome)` once the user (or [`crate::present::dismiss`], or
/// the host going away) ends the alert, or `Err`: an invalid spec, `Busy`,
/// no host to present over, or `Unsupported` on a platform without a native
/// alert arm. The returned [`Presentation`] is a plain [`Future`] — await it
/// from `frust::spawn_local` or any executor — and its
/// [`Presentation::handle`] is what [`crate::present::dismiss`] takes.
/// Dropping it frees the one-at-a-time slot but leaves the platform alert on
/// screen until answered (that answer is then discarded).
///
/// ```ignore
/// frust::spawn_local(async move {
///     let spec = AlertSpec::new("Delete draft?", "This cannot be undone.")
///         .with_action("keep", "Keep", ActionRole::Cancel)
///         .with_action("delete", "Delete", ActionRole::Destructive);
///     if let Ok(AlertOutcome::Action(id)) = show_native_alert(spec).await
///         && id == "delete"
///     {
///         drafts.update(|d| d.clear());
///     }
/// });
/// ```
pub fn show_native_alert(spec: AlertSpec) -> Presentation<AlertOutcome> {
    present::show_alert(spec)
}

/// Present a native alert and write its one outcome into `signal` as
/// `Some(outcome)` — the no-`async` form (module doc's *Controlled*).
///
/// Returns the [`PresentationHandle`] [`crate::present::dismiss`] takes, or
/// the error of a request refused **before** anything was presented —
/// `InvalidSpec`, `Busy`, or a platform refusal decided on the spot — in
/// which case `signal` is never written. An error discovered only later, on
/// the platform's main thread (no host to present over, an iPad action sheet
/// without an anchor requested off the main thread, a platform failure),
/// cannot be written into an outcome signal: it is logged at `warn` and the
/// signal is left as it was. Await [`show_native_alert`] instead when the
/// app must tell those apart.
///
/// Must be called on the UI thread: the outcome is awaited on
/// `frust::spawn_local`'s UI-thread task queue, pumped every frame.
///
/// # Errors
/// The synchronous refusals listed above.
///
/// ```ignore
/// // In a handler — no async block needed:
/// let outcome: RwSignal<Option<AlertOutcome>> = RwSignal::new(None);
/// native_button("Delete").on_press(move || {
///     let spec = AlertSpec::new("Delete draft?", "")
///         .with_action("keep", "Keep", ActionRole::Cancel)
///         .with_action("delete", "Delete", ActionRole::Destructive);
///     if let Err(err) = show_native_alert_into(spec, outcome) {
///         log::warn!("no alert: {err}");
///     }
/// })
/// // …and on a later rebuild, `outcome.get()` is `Some(AlertOutcome::Action(..))`.
/// ```
pub fn show_native_alert_into(
    spec: AlertSpec,
    signal: RwSignal<Option<AlertOutcome>>,
) -> Result<PresentationHandle, PresentError> {
    deliver_into(
        show_native_alert(spec),
        move |outcome| signal.set(Some(outcome)),
        frust::spawn_local,
    )
}

/// A spawned future awaiting one presentation.
type Delivery = Pin<Box<dyn Future<Output = ()>>>;

/// [`show_native_alert_into`]'s body, over an injected `write` and `spawn`
/// so the host tests can drive it without a frust runtime: a refused
/// request answers `Err` synchronously and spawns nothing; an accepted one
/// spawns a task that hands the outcome to `write` — an `FnOnce`, so once
/// by construction.
fn deliver_into(
    mut presentation: Presentation<AlertOutcome>,
    write: impl FnOnce(AlertOutcome) + 'static,
    spawn: impl FnOnce(Delivery),
) -> Result<PresentationHandle, PresentError> {
    let Some(handle) = presentation.handle() else {
        return Err(refusal(&mut presentation));
    };
    spawn(Box::pin(async move {
        match presentation.await {
            Ok(outcome) => write(outcome),
            Err(err) => log::warn!(
                "frust-native-widgets: a native alert ended without an outcome to report: {err}"
            ),
        }
    }));
    Ok(handle)
}

/// The error a refused request (no handle) carries. `take_refusal` is the
/// normal path; the poll is a fallback that cannot suspend (a refused
/// presentation is already resolved), and a refused presentation holding no
/// error would be a `present` contract break, reported as a platform error.
fn refusal(presentation: &mut Presentation<AlertOutcome>) -> PresentError {
    if let Some(err) = presentation.take_refusal() {
        return err;
    }
    match Pin::new(presentation).poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(Err(err)) => err,
        Poll::Ready(Ok(_)) | Poll::Pending => {
            PresentError::Platform("a refused native presentation carried no error".to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use frust::GetUntracked;

    use super::*;
    use crate::present::test_support::{pending_pair, with_slot_held};
    use crate::present::{ActionRole, AlertStyle};

    fn spec() -> AlertSpec {
        AlertSpec::new("Delete draft?", "This cannot be undone.")
            .with_action("keep", "Keep", ActionRole::Cancel)
            .with_action("delete", "Delete", ActionRole::Destructive)
    }

    /// Captures what `deliver_into` spawns, so the test drives it.
    fn capture() -> (Rc<RefCell<Vec<Delivery>>>, impl FnOnce(Delivery)) {
        let spawned = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&spawned);
        (spawned, move |task| sink.borrow_mut().push(task))
    }

    fn poll(task: &mut Delivery) -> Poll<()> {
        task.as_mut().poll(&mut Context::from_waker(Waker::noop()))
    }

    #[test]
    fn the_adapter_writes_the_signal_exactly_once() {
        let signal: RwSignal<Option<AlertOutcome>> = RwSignal::new(None);
        let writes = Rc::new(RefCell::new(0));
        let counter = Rc::clone(&writes);
        let (tx, presentation) = pending_pair::<AlertOutcome>();
        let (spawned, spawn) = capture();

        let handle = deliver_into(
            presentation,
            move |outcome| {
                *counter.borrow_mut() += 1;
                signal.set(Some(outcome));
            },
            spawn,
        );
        assert!(handle.is_ok(), "an accepted request answers its handle");
        let mut task = spawned.borrow_mut().pop().expect("one task spawned");
        assert!(spawned.borrow().is_empty());

        // Pending until the platform answers: nothing written yet.
        assert_eq!(poll(&mut task), Poll::Pending);
        assert_eq!(signal.get_untracked(), None);

        assert!(tx.send(Ok(AlertOutcome::Action("delete".into()))));
        assert_eq!(poll(&mut task), Poll::Ready(()));
        assert_eq!(
            signal.get_untracked(),
            Some(AlertOutcome::Action("delete".into()))
        );

        // The task is finished (an executor drops it now), `write` was an
        // `FnOnce`, and the sender was consumed by its one send — no path
        // is left that could write again.
        assert_eq!(*writes.borrow(), 1);
    }

    #[test]
    fn a_late_error_is_logged_and_never_written() {
        let signal: RwSignal<Option<AlertOutcome>> = RwSignal::new(None);
        let (tx, presentation) = pending_pair::<AlertOutcome>();
        let (spawned, spawn) = capture();

        deliver_into(presentation, move |o| signal.set(Some(o)), spawn).expect("accepted");
        let mut task = spawned.borrow_mut().pop().expect("one task spawned");
        assert!(tx.send(Err(PresentError::NoHost)));
        assert_eq!(poll(&mut task), Poll::Ready(()));
        assert_eq!(signal.get_untracked(), None);
    }

    #[test]
    fn busy_surfaces_as_err_from_both_forms() {
        with_slot_held(|| {
            let mut presentation = show_native_alert(spec());
            assert_eq!(presentation.handle(), None);
            assert_eq!(
                Pin::new(&mut presentation).poll(&mut Context::from_waker(Waker::noop())),
                Poll::Ready(Err(PresentError::Busy))
            );

            // The signal form answers synchronously and spawns nothing (the
            // real `frust::spawn_local` would need a UI-thread executor).
            let signal: RwSignal<Option<AlertOutcome>> = RwSignal::new(None);
            assert_eq!(
                show_native_alert_into(spec(), signal),
                Err(PresentError::Busy)
            );
            assert_eq!(signal.get_untracked(), None);
        });
    }

    #[test]
    fn an_invalid_spec_surfaces_as_err_and_spawns_nothing() {
        let mut too_many = spec()
            .with_action("a", "A", ActionRole::Default)
            .with_action("b", "B", ActionRole::Default);
        too_many.style = AlertStyle::ActionSheet;
        let (spawned, spawn) = capture();
        let written = Rc::new(RefCell::new(false));
        let flag = Rc::clone(&written);

        let result = deliver_into(
            show_native_alert(too_many),
            move |_| *flag.borrow_mut() = true,
            spawn,
        );
        assert!(matches!(result, Err(PresentError::InvalidSpec(_))));
        assert!(spawned.borrow().is_empty());
        assert!(!*written.borrow());
    }
}
