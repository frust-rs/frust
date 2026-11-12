//! [`StartGate`]: show a splash view while start-up work runs, then build the app's real UI from
//! the result.
//!
//! The gate wraps the heavy-work idiom ([`use_task`] and [`AsyncValue`]) in a view. It starts the
//! task when it is mounted, shows the splash while the task is idle or loading, shows an error
//! view with a retry action when the task fails, and builds the real root from the task's value
//! once it succeeds. Dropping the gate aborts a still-pending task, because the task lives under
//! the gate's owner.
//!
//! The gate does not hold the platform splash screen and does not touch `app!`: the first frame
//! shows whatever the app's root `init`/`build` produced, which here is the splash.
//!
//! ```ignore
//! use frust::{Component, StartGate, View, spawn_blocking, text};
//!
//! #[derive(Default)]
//! pub struct App;
//!
//! impl Component for App {
//!     type State = ();
//!
//!     fn init(&self) {}
//!
//!     fn build(&self, _state: &mut ()) -> impl View<()> {
//!         StartGate::new(|| async {
//!             // Open the database and run migrations off the UI thread.
//!             spawn_blocking(|| {
//!                 let db = frust_db::Database::open("app.db")?;
//!                 db.migrate()?;
//!                 Ok::<_, frust_db::Error>(db)
//!             })
//!             .await
//!             .map_err(|e| e.to_string())?
//!             .map_err(|e| e.to_string())
//!         })
//!         .splash(text("Starting"))
//!         .ready(|db: &frust_db::Database| text(format!("{} tables", db.table_count())))
//!     }
//! }
//! ```
//!
//! The same shape with a plain future, which compiles as written:
//!
//! ```
//! use std::convert::Infallible;
//!
//! use frust::{Component, StartGate, View, text};
//!
//! #[derive(Default)]
//! pub struct App;
//!
//! impl Component for App {
//!     type State = ();
//!
//!     fn init(&self) {}
//!
//!     fn build(&self, _state: &mut ()) -> impl View<()> {
//!         StartGate::new(|| async { Ok::<_, Infallible>(42_u32) })
//!             .splash(text("Starting"))
//!             .ready(|answer: &u32| text(format!("Ready: {answer}")))
//!     }
//! }
//! ```
//!
//! # Ownership
//!
//! The ready closure runs once per success, when the real root is mounted, and the subtree it
//! returns is retained: an unrelated signal change rebuilds the gate but not the ready subtree.
//! State created inside the ready subtree (a nested [`Component`], a `use_task`, a signal) is
//! owned by it and disposed when the gate swaps to another arm or is dropped.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;

use frust_reactive::{AsyncValue, RwSignal, UseTask, use_task};

use crate::{
    AnyView, Column, Component, EitherOf3, View, With, WithUntracked, button, component, text,
};

/// The failure a start-up task reports: the message of the error it returned.
#[derive(Debug)]
struct StartError(String);

impl fmt::Display for StartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for StartError {}

type Fetch<T> = Rc<dyn Fn() -> Pin<Box<dyn Future<Output = Result<T, StartError>> + Send>>>;
type ErrorFn = Rc<dyn Fn(&str, Retry) -> AnyView<()>>;
type ReadyFn<T> = Rc<dyn Fn(&T) -> AnyView<()>>;

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
        Self::new(AnyView::new(crate::SizedBox::<()>(None, None)))
    }
}

impl View<()> for Slot {
    type Element = <AnyView<()> as View<()>>::Element;

    fn build(&self, ctx: &mut crate::authoring::BuildCtx<'_>) -> Self::Element {
        self.0.build(ctx)
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut Self::Element,
        ctx: &mut crate::authoring::BuildCtx<'_>,
    ) -> crate::authoring::ChangeFlags {
        self.0.rebuild(&prev.0, element, ctx)
    }

    fn teardown(&self, element: &mut Self::Element, ctx: &mut crate::authoring::BuildCtx<'_>) {
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

fn default_error(message: &str, retry: Retry) -> AnyView<()> {
    AnyView::new(Column((
        text(message),
        button::<(), _>("Retry", move |_| retry.run()),
    )))
}

/// A view that runs start-up work behind a splash, then shows the real UI. See the
/// [module docs](self) for an example.
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
    /// here, and is aborted when that owner is disposed. The error's message is shown by the
    /// error view.
    pub fn new<E, Fut, F>(start: F) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
        Fut: Future<Output = Result<T, E>> + Send + 'static,
        F: Fn() -> Fut + 'static,
    {
        let fetch: Fetch<T> = Rc::new(move || {
            let future = start();
            Box::pin(async move { future.await.map_err(|e| StartError(e.to_string())) })
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
    /// the task. Without it the gate shows the message and a Retry button.
    pub fn error<V: View<()>>(mut self, error: impl Fn(&str, Retry) -> V + 'static) -> Self {
        self.error = Some(Rc::new(move |message, retry| {
            AnyView::new(error(message, retry))
        }));
        self
    }

    /// Builds the app's real root from the task's value.
    ///
    /// The closure runs once per success; the returned subtree is retained across unrelated
    /// rebuilds and owns the state created inside it.
    pub fn ready<V: View<()>>(mut self, ready: impl Fn(&T) -> V + 'static) -> Self {
        self.ready = Some(Rc::new(move |value| AnyView::new(ready(value))));
        self
    }

    fn host(&self) -> crate::ComponentView<GateHost<T>> {
        component(GateHost {
            fetch: self.fetch.clone(),
            splash: self.splash.clone(),
            error: self.error.clone(),
            ready: self.ready.clone(),
        })
    }
}

impl<S: 'static, T: Send + Sync + 'static> View<S> for StartGate<T> {
    type Element = <crate::ComponentView<GateHost<T>> as View<S>>::Element;

    fn build(&self, ctx: &mut crate::authoring::BuildCtx<'_>) -> Self::Element {
        View::<S>::build(&self.host(), ctx)
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut Self::Element,
        ctx: &mut crate::authoring::BuildCtx<'_>,
    ) -> crate::authoring::ChangeFlags {
        View::<S>::rebuild(&self.host(), &prev.host(), element, ctx)
    }

    fn teardown(&self, element: &mut Self::Element, ctx: &mut crate::authoring::BuildCtx<'_>) {
        View::<S>::teardown(&self.host(), element, ctx);
    }
}
