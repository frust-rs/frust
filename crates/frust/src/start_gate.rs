//! [`StartGate`]: show a splash view while start-up work runs, then build the app's real UI from
//! the result.
//!
//! The gate wraps the heavy-work idiom ([`use_task`] and [`AsyncValue`]) in a view. It starts the
//! task when it is mounted, shows the splash while the task is idle or loading, shows an error
//! view with a retry action when the task fails, and builds the real root from the task's value
//! once it succeeds. Dropping the gate aborts a still-pending task, because the task lives under
//! the gate's owner. A panic while the task's future is polled ends the attempt in the error view
//! too, where panics unwind (debug and test builds); the panic hook still reports it. Under the
//! release profile's `panic = "abort"` a panic still ends the process.
//!
//! The gate does not hold the platform splash screen and does not touch `app!`: the first frame
//! shows whatever the app's root `init`/`build` produced, which here is the splash.
//! `open_and_migrate` below stands for the app's own blocking set-up function:
//!
//! ```
//! use frust::{Component, StartGate, View, spawn_blocking, text};
//! # pub struct Database;
//! # fn open_and_migrate() -> Result<Database, std::io::Error> { Ok(Database) }
//!
//! pub struct App;
//!
//! impl Component for App {
//!     type State = ();
//!
//!     fn init(&self) {}
//!
//!     fn build(&self, _state: &mut ()) -> impl View<()> {
//!         StartGate::new(|| async {
//!             spawn_blocking(open_and_migrate).await.map_err(std::io::Error::other)?
//!         })
//!         .splash(text("Starting"))
//!         .ready(|_db: &Database| text("Ready"))
//!     }
//! }
//! ```
//!
//! # Inputs
//!
//! The start closure is captured when the gate mounts; every [`Retry`] re-runs that closure. The
//! ready closure is captured when the ready view mounts, so it must not capture changing parent
//! state by value: read signals inside the subtree instead. The splash and error views follow the
//! latest rebuild of the gate.
//!
//! # Ownership
//!
//! The ready closure runs once per success and the subtree it returns is retained: a later gate
//! rebuild does not call the closure again, and the subtree keeps its state. That subtree is still
//! rebuilt and diffed on every gate rebuild, and the components inside it re-run `build`. State
//! created inside it (a nested [`Component`], a `use_task`, a signal) is owned by it and disposed
//! when the gate swaps to another arm or is dropped.

use std::any::Any;
use std::fmt;
use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll};

use frust_reactive::{AsyncValue, RwSignal, UseTask, use_task};

use crate::{
    AnyView, Column, Component, ComponentView, EitherOf3, SizedBox, View, With, WithUntracked,
    authoring::{BuildCtx, ChangeFlags},
    button, component, text,
};

/// The failure a start-up task reports: the message of the error it returned, or of its panic.
#[derive(Debug)]
struct StartError(String);

impl fmt::Display for StartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for StartError {}

impl StartError {
    /// The failure for a panic raised while the start-up future was polled.
    fn panicked(payload: &(dyn Any + Send)) -> Self {
        let detail = payload
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| payload.downcast_ref::<String>().map(String::as_str));
        Self(match detail {
            Some(detail) => format!("start-up task panicked: {detail}"),
            None => "start-up task panicked".to_owned(),
        })
    }
}

type StartFuture<T> = Pin<Box<dyn Future<Output = Result<T, StartError>> + Send>>;
type Fetch<T> = Rc<dyn Fn() -> StartFuture<T>>;
type ErrorFn = Rc<dyn Fn(&str, Retry) -> AnyView<()>>;
type ReadyFn<T> = Rc<dyn Fn(&T) -> AnyView<()>>;

/// Polls a start-up future and turns a panic inside a poll into a failed attempt.
///
/// `use_task` treats a panicked background task like an aborted one and leaves the state
/// loading, which would keep the splash up for good. Catching the panic inside the task keeps
/// abort silent: aborting drops this future, and the user's future with it, without an error.
/// After a caught panic the inner future is never polled again, only dropped.
struct CatchPanic<T>(StartFuture<T>);

impl<T> Future for CatchPanic<T> {
    type Output = Result<T, StartError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let inner = self.0.as_mut();
        catch_unwind(AssertUnwindSafe(|| inner.poll(cx)))
            .unwrap_or_else(|payload| Poll::Ready(Err(StartError::panicked(&*payload))))
    }
}

/// Restarts a [`StartGate`]'s start-up task. Handed to the gate's error view.
#[derive(Clone)]
pub struct Retry(Rc<dyn Fn()>);

impl Retry {
    /// Re-run the start-up task: the gate goes back to its splash and, on success, builds the
    /// ready view.
    pub fn run(&self) {
        (self.0)();
    }
}

/// A shared, already-erased view, so a stored view can be handed to every rebuild.
#[derive(Clone)]
struct Slot(Rc<AnyView<()>>);

impl Slot {
    fn new(view: AnyView<()>) -> Self {
        Self(Rc::new(view))
    }

    fn empty() -> Self {
        Self::new(AnyView::new(SizedBox::<()>(None, None)))
    }
}

impl View<()> for Slot {
    type Element = <AnyView<()> as View<()>>::Element;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> Self::Element {
        self.0.build(ctx)
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut Self::Element,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        self.0.rebuild(&prev.0, element, ctx)
    }

    fn teardown(&self, element: &mut Self::Element, ctx: &mut BuildCtx<'_>) {
        self.0.teardown(element, ctx);
    }
}

/// Hosts one stored view under its own owner, so each gate arm has an independent lifetime.
struct Pane(Slot);

impl Component for Pane {
    type State = ();

    fn init(&self) {}

    fn build(&self, _state: &mut ()) -> impl View<()> {
        self.0.clone()
    }
}

/// Builds the ready subtree once, in `init`, and keeps it for the owner's lifetime.
struct ReadyHost<T: Send + Sync + 'static> {
    signal: RwSignal<AsyncValue<T>>,
    ready: Option<ReadyFn<T>>,
}

impl<T: Send + Sync + 'static> Component for ReadyHost<T> {
    type State = Slot;

    fn init(&self) -> Slot {
        self.signal
            .with_untracked(|state| match (state.ready(), &self.ready) {
                (Some(value), Some(ready)) => Slot::new(ready(value)),
                _ => Slot::empty(),
            })
    }

    fn build(&self, state: &mut Slot) -> impl View<Slot> {
        component(Pane(state.clone()))
    }
}

/// The component behind [`StartGate`]: starts the task under the gate's owner and picks the arm
/// to show. Public only because it names [`StartGate`]'s element type; it is not nameable.
#[doc(hidden)]
pub struct GateHost<T: Send + Sync + 'static> {
    fetch: Fetch<T>,
    splash: Slot,
    error: Option<ErrorFn>,
    ready: Option<ReadyFn<T>>,
}

impl<T: Send + Sync + 'static> Component for GateHost<T> {
    type State = UseTask<T>;

    fn init(&self) -> UseTask<T> {
        let fetch = self.fetch.clone();
        use_task(move || fetch())
    }

    fn build(&self, task: &mut UseTask<T>) -> impl View<UseTask<T>> {
        enum Arm {
            Splash,
            Error(String),
            Ready,
        }
        let signal = task.signal();
        let arm = signal.with(|state| match state {
            AsyncValue::Ready(_) => Arm::Ready,
            AsyncValue::Error(e) => Arm::Error(e.to_string()),
            AsyncValue::Idle | AsyncValue::Loading(_) => Arm::Splash,
        });
        match arm {
            Arm::Splash => EitherOf3::A(component(Pane(self.splash.clone()))),
            Arm::Error(message) => {
                let task = task.clone();
                let retry = Retry(Rc::new(move || task.restart()));
                let view = match &self.error {
                    Some(error) => error(&message, retry),
                    None => default_error(&message, retry),
                };
                EitherOf3::B(component(Pane(Slot::new(view))))
            }
            Arm::Ready => EitherOf3::C(component(ReadyHost {
                signal,
                ready: self.ready.clone(),
            })),
        }
    }
}

/// The error view used without [`StartGate::error`]: the error's `Display` text, verbatim, above a
/// Retry button.
fn default_error(message: &str, retry: Retry) -> AnyView<()> {
    AnyView::new(Column((
        text(message),
        button::<(), _>("Retry", move |_| retry.run()),
    )))
}

/// A view that runs start-up work behind a splash, then shows the real UI. See the
/// [module docs](self) for an example and for which inputs are fixed at mount.
///
/// Build it with [`new`](Self::new), then set [`splash`](Self::splash),
/// [`error`](Self::error) (optional) and [`ready`](Self::ready). It is a [`View`] over any state.
/// A gate with no ready closure shows an empty box on success.
pub struct StartGate<T: Send + Sync + 'static> {
    fetch: Fetch<T>,
    splash: Slot,
    error: Option<ErrorFn>,
    ready: Option<ReadyFn<T>>,
}

impl<T: Send + Sync + 'static> StartGate<T> {
    /// A gate over the start-up task `start`, the same kind of closure [`use_task`] takes.
    ///
    /// `T` need not be `Clone`. The task starts when the gate is mounted under its owner, not
    /// here, and is aborted when that owner is disposed. The error's `Display` text is what the
    /// error view receives; a panic while the future is polled arrives as a message saying the
    /// start-up task panicked.
    ///
    /// `start` is called on the UI thread when the gate mounts and on every [`Retry`], so the code
    /// it runs before returning its future must not block. The future is polled on the background
    /// runtime's few async workers, which blocking work stalls too: route that work through
    /// [`spawn_blocking`](crate::spawn_blocking).
    pub fn new<E, Fut, F>(start: F) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
        Fut: Future<Output = Result<T, E>> + Send + 'static,
        F: Fn() -> Fut + 'static,
    {
        let fetch: Fetch<T> = Rc::new(move || {
            let future = start();
            Box::pin(CatchPanic(Box::pin(async move {
                future.await.map_err(|e| StartError(e.to_string()))
            })))
        });
        Self {
            fetch,
            splash: Slot::empty(),
            error: None,
            ready: None,
        }
    }

    /// The view shown while the task is idle or loading. Empty by default.
    pub fn splash(mut self, view: impl View<()>) -> Self {
        self.splash = Slot::new(AnyView::new(view));
        self
    }

    /// The view shown when the task fails, given the error message and a [`Retry`] that restarts
    /// the task.
    ///
    /// Only the message string reaches this closure, never the error value. Without it the gate
    /// shows that message verbatim above a Retry button, so pass a closure here to show
    /// user-facing wording instead of an internal error's text.
    pub fn error<V: View<()>>(mut self, error: impl Fn(&str, Retry) -> V + 'static) -> Self {
        self.error = Some(Rc::new(move |message, retry| {
            AnyView::new(error(message, retry))
        }));
        self
    }

    /// Builds the app's real root from the task's value.
    ///
    /// The closure runs once per success and is captured when the ready view mounts; the returned
    /// subtree is retained across later rebuilds and owns the state created inside it.
    pub fn ready<V: View<()>>(mut self, ready: impl Fn(&T) -> V + 'static) -> Self {
        self.ready = Some(Rc::new(move |value| AnyView::new(ready(value))));
        self
    }

    fn host(&self) -> ComponentView<GateHost<T>> {
        component(GateHost {
            fetch: self.fetch.clone(),
            splash: self.splash.clone(),
            error: self.error.clone(),
            ready: self.ready.clone(),
        })
    }
}

impl<S: 'static, T: Send + Sync + 'static> View<S> for StartGate<T> {
    type Element = <ComponentView<GateHost<T>> as View<S>>::Element;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> Self::Element {
        View::<S>::build(&self.host(), ctx)
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut Self::Element,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        View::<S>::rebuild(&self.host(), &prev.host(), element, ctx)
    }

    fn teardown(&self, element: &mut Self::Element, ctx: &mut BuildCtx<'_>) {
        View::<S>::teardown(&self.host(), element, ctx);
    }
}
