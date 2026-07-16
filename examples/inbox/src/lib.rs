//! The phase-5.5 compatibility proof: a `clean_signals` controller driving a
//! ForgeKit [`Component`] (spec §5.5).
//!
//! This example wires the two frameworks together end to end:
//!
//! - a real async [`clean_signals::UseCase`] ([`LoadMessages`], a fake inbox
//!   repo with a `tokio::time::sleep` and a deliberate first-call failure so the
//!   [`RetryPolicy`] is exercised);
//! - a presentation-layer controller ([`InboxController`]) that *embeds* a
//!   [`clean_signals::ControllerCore`] by composition and drives the use case
//!   through [`ControllerCore::run_into`], writing an
//!   [`AsyncState`](clean_signals::AsyncState) into an `RwSignal`;
//! - a ForgeKit [`Component`] ([`Inbox`]) whose `init` builds the controller,
//!   kicks off the load on the UI-thread local task queue
//!   ([`forgekit::spawn_local`]), and registers controller disposal on component
//!   teardown ([`on_cleanup`]); whose `build` renders the controller's
//!   `AsyncState` signal.
//!
//! # This example is a template for the future glue crate
//!
//! The two glue pieces — [`use_controller`] (component-scoped controller
//! lifecycle) and [`async_view`] (an `AsyncState`-to-view renderer) — are the
//! ForgeKit transliterations of `clean-signals-leptos`'s `use_controller` hook
//! and `AsyncView` component. They are written here as clean, module-level,
//! *extractable* helpers (generic over the controller and failure types, not
//! tied to `InboxController`) so a future `clean-signals-forgekit` crate can
//! lift them out verbatim. The rest of the module is the app-specific wiring
//! that consumes them.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use clean_signals::{
    ControllerCore, Failure, NoParams, RetryPolicy, RunOptions, UseCase, async_state_signal,
};
// Re-exported so the headless test (which has no direct clean-signals dependency)
// can name the state enum it asserts on.
pub use clean_signals::AsyncState;
use forgekit::{AnyView, Column, Component, any, button, component, on_cleanup, text};
use reactive_graph::signal::RwSignal;
use reactive_graph::traits::Get;

// ---------------------------------------------------------------------------
// Extractable glue #1: component-scoped controller lifecycle.
//
// The ForgeKit transliteration of `clean-signals-leptos`'s `use_controller`
// hook. Leptos scopes the controller to the current reactive owner and disposes
// it on `on_cleanup`; ForgeKit's `Component::init` runs under the component's
// own reactive `Owner` (ambient during `init`), so the identical `on_cleanup`
// call binds disposal to *this component's* teardown — the exact seam this
// example proves. This is generic over the controller `C` and failure `F`; move
// it into a `clean-signals-forgekit` crate unchanged.
// ---------------------------------------------------------------------------

/// Builds an app controller once and disposes it on component teardown.
///
/// `factory` runs a single time (when the owning [`Component`] first builds,
/// inside `Component::init`). The returned controller is wrapped in an [`Arc`]
/// so the render tree and the spawned load task can share a cheap handle, and
/// its embedded [`ControllerCore`] is [`dispose`](ControllerCore::dispose)d via
/// [`on_cleanup`] when the component unmounts — aborting in-flight watches,
/// running registered teardowns, and releasing owned signals.
///
/// The controller must expose its [`ControllerCore`] by `AsRef` (embed-by-
/// composition, the clean-signals idiom) and be `Send + Sync` because
/// [`on_cleanup`]'s callback is `Send + Sync` (an `Arc<C>` is captured, not an
/// `Rc<C>` — the ForgeKit reactive owner may run cleanups off the construction
/// thread, unlike leptos's single-threaded browser owner).
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

// ---------------------------------------------------------------------------
// Extractable glue #2: an AsyncState renderer.
//
// The ForgeKit transliteration of `clean-signals-leptos`'s `<AsyncView>`
// component. Leptos re-renders reactively on signal change; ForgeKit components
// re-run `build` every frame (per-component tracked scopes are deferred to a
// later phase), so this is a plain match on the current `AsyncState` producing
// an `AnyView`. Generic over the value `T`, failure `F`, and outer state `S`;
// move it into a `clean-signals-forgekit` crate unchanged.
// ---------------------------------------------------------------------------

/// Maps a snapshot of an [`AsyncState`] to a view, mirroring
/// `clean-signals-leptos`'s `<AsyncView>`:
///
/// - **`Loading`** → `loading()` (a first-load placeholder);
/// - **`Data(v)` / `Reloading(v)`** → `data(v)` (stale data stays visible while
///   a refresh is in flight);
/// - **`Error { failure, .. }`** → `error(failure)`.
///
/// Take the snapshot with a tracked `signal.get()` at the call site so the
/// shell's frame-tracking scope re-renders when the controller writes a new
/// state.
pub fn async_view<S, T, F>(
    state: AsyncState<T, F>,
    data: impl FnOnce(T) -> AnyView<S>,
    loading: impl FnOnce() -> AnyView<S>,
    error: impl FnOnce(F) -> AnyView<S>,
) -> AnyView<S>
where
    S: 'static,
    F: Failure,
{
    match state {
        AsyncState::Loading => loading(),
        AsyncState::Data(value) | AsyncState::Reloading(value) => data(value),
        AsyncState::Error { failure, .. } => error(failure),
    }
}

// ---------------------------------------------------------------------------
// App-specific domain: the failure, the message, and the fake-repo use case.
// ---------------------------------------------------------------------------

/// The app's failure type. `Network` is retryable (the [`RetryPolicy`] default
/// predicate is [`Failure::is_retryable`]), so the first-attempt `Network`
/// failure the fake repo returns is retried rather than surfaced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppFailure {
    /// A transient network error — retryable.
    Network,
}

impl std::fmt::Display for AppFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppFailure::Network => f.write_str("network unavailable"),
        }
    }
}

impl Failure for AppFailure {
    fn is_retryable(&self) -> bool {
        matches!(self, AppFailure::Network)
    }
}

/// One inbox message. `Clone + Send + Sync + 'static` so it can flow through
/// [`ControllerCore::run_into`]'s `AsyncState<Vec<Message>, _>` signal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub id: u64,
    pub subject: String,
}

/// A fake inbox repository as a [`UseCase`]: an in-process async fn that sleeps
/// briefly (so the load is genuinely asynchronous) and **fails on its first
/// attempt** with a retryable [`AppFailure::Network`], succeeding on every
/// attempt thereafter — exercising the controller's [`RetryPolicy`]. The
/// per-instance attempt counter is exposed via [`LoadMessages::attempts`] so a
/// test can prove the retry happened.
pub struct LoadMessages {
    attempts: Arc<AtomicU32>,
}

impl LoadMessages {
    fn new() -> Self {
        Self {
            attempts: Arc::new(AtomicU32::new(0)),
        }
    }

    /// How many times [`UseCase::execute`] has run on this instance.
    pub fn attempts(&self) -> u32 {
        self.attempts.load(Ordering::SeqCst)
    }
}

// Dual `cfg_attr`: `Send` futures on native, `?Send` on wasm — the clean-signals
// convention (the example is desktop-run, but staying mobile-buildable is a
// bonus). `clean_signals::async_trait` is the re-exported `async-trait` macro,
// so this crate adds no `async-trait` dependency of its own.
#[cfg_attr(not(target_arch = "wasm32"), clean_signals::async_trait)]
#[cfg_attr(target_arch = "wasm32", clean_signals::async_trait(?Send))]
impl UseCase for LoadMessages {
    type Params = NoParams;
    type Output = Vec<Message>;
    type Failure = AppFailure;

    async fn execute(&self, _params: NoParams) -> Result<Vec<Message>, AppFailure> {
        let attempt = self.attempts.fetch_add(1, Ordering::SeqCst) + 1;
        // A small real async delay: registers with the background tokio timer
        // driver when the local task is pumped inside the runtime context.
        clean_signals::sleep(Duration::from_millis(20)).await;
        if attempt == 1 {
            // First call always fails (retryable) — the RetryPolicy then retries.
            return Err(AppFailure::Network);
        }
        Ok(vec![
            Message {
                id: 1,
                subject: "Welcome to ForgeKit".to_string(),
            },
            Message {
                id: 2,
                subject: "Your build finished".to_string(),
            },
            Message {
                id: 3,
                subject: "clean-signals says hi".to_string(),
            },
        ])
    }
}

/// The number of messages [`LoadMessages`] returns on success — the headless
/// test asserts the painted scene reflects exactly this many rows.
pub const MESSAGE_COUNT: usize = 3;

// ---------------------------------------------------------------------------
// The presentation-layer controller: embeds a ControllerCore by composition.
// ---------------------------------------------------------------------------

/// The inbox screen's controller (view model). Embeds a [`ControllerCore`] by
/// composition and drives [`LoadMessages`] into its `messages` signal.
pub struct InboxController {
    core: ControllerCore<AppFailure>,
    /// The reactive state the component renders. `pub` so a test can observe the
    /// Loading→Data transition directly alongside the painted scene.
    pub messages: RwSignal<AsyncState<Vec<Message>, AppFailure>>,
    load_messages: LoadMessages,
    /// Bumped once by an `on_dispose` cleanup so a test can prove disposal runs
    /// exactly once.
    dispose_calls: Arc<AtomicU32>,
}

impl InboxController {
    /// Builds a fresh controller. Runs inside [`use_controller`]'s factory, so
    /// under the component's reactive owner: `async_state_signal()` and
    /// `ControllerCore::new()` allocate their reactive slots against that owner
    /// and are released on teardown.
    fn new() -> Self {
        let core = ControllerCore::new();
        let dispose_calls = Arc::new(AtomicU32::new(0));
        // A disposal probe: run once, LIFO, when `dispose()` runs.
        let probe = Arc::clone(&dispose_calls);
        core.on_dispose(move || {
            probe.fetch_add(1, Ordering::SeqCst);
        });
        Self {
            core,
            messages: async_state_signal(),
            load_messages: LoadMessages::new(),
            dispose_calls,
        }
    }

    /// Loads the inbox into the `messages` signal, retrying the (retryable)
    /// first-attempt failure up to three total attempts with a short backoff.
    /// Every write to `messages` is disposal-gated inside `run_into`, so a
    /// controller disposed mid-load never touches its (possibly freed) signal.
    async fn load(&self) {
        let opts = RunOptions {
            retry: RetryPolicy::new(3, Duration::from_millis(5)),
            ..Default::default()
        };
        let _ = self
            .core
            .run_into(&self.load_messages, NoParams, self.messages, opts)
            .await;
    }

    /// How many times the fake repo's `execute` ran — `2` after a successful
    /// load (attempt 1 fails, attempt 2 succeeds), proving the retry path.
    pub fn attempts(&self) -> u32 {
        self.load_messages.attempts()
    }

    /// How many times disposal has run (the `on_dispose` probe). Exactly `1`
    /// after the component tears down.
    pub fn dispose_calls(&self) -> u32 {
        self.dispose_calls.load(Ordering::SeqCst)
    }

    /// Whether the embedded controller core has been disposed.
    pub fn is_disposed(&self) -> bool {
        self.core.is_disposed()
    }
}

/// Embed-by-composition: expose the [`ControllerCore`] so [`use_controller`] can
/// dispose it generically.
impl AsRef<ControllerCore<AppFailure>> for InboxController {
    fn as_ref(&self) -> &ControllerCore<AppFailure> {
        &self.core
    }
}

/// A test spy: [`Inbox::init`] publishes the freshly built controller here so a
/// headless test can observe its load/attempts/disposal. `None` in production.
pub type ControllerSpy = Arc<Mutex<Option<Arc<InboxController>>>>;

// ---------------------------------------------------------------------------
// The Component.
// ---------------------------------------------------------------------------

/// The inbox screen, a ForgeKit [`Component`] whose retained state is the shared
/// [`InboxController`] handle.
///
/// - `init` builds the controller (via [`use_controller`], wiring disposal to
///   teardown), kicks off the initial [`InboxController::load`] on the UI-thread
///   local task queue, and — for tests — publishes the controller to a spy.
/// - `build` renders the controller's `AsyncState` signal via [`async_view`].
#[derive(Default)]
pub struct Inbox {
    spy: Option<ControllerSpy>,
}

impl Inbox {
    /// A production inbox (no test spy).
    pub fn new() -> Self {
        Self::default()
    }

    /// An inbox that publishes its controller to `spy` on `init` (headless test).
    pub fn with_spy(spy: ControllerSpy) -> Self {
        Self { spy: Some(spy) }
    }
}

impl Component for Inbox {
    type State = Arc<InboxController>;

    fn init(&self) -> Arc<InboxController> {
        // The component's reactive `Owner` is ambient here, so `use_controller`'s
        // `on_cleanup` binds disposal to *this* component's teardown.
        let controller = use_controller::<InboxController, AppFailure>(InboxController::new);

        if let Some(spy) = &self.spy {
            *spy.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::clone(&controller));
        }

        // Kick off the load on the UI-thread local task queue (a `!Send`-friendly
        // handle; the shell drains this queue each frame via `pump_local`).
        let handle = Arc::clone(&controller);
        forgekit::spawn_local(async move {
            handle.load().await;
        });

        controller
    }

    fn build(&self, state: &mut Arc<InboxController>) -> AnyView<Arc<InboxController>> {
        // Tracked read: the shell's frame-tracking scope re-renders this
        // component when `run_into` writes the next state.
        let snapshot = state.messages.get();
        async_view(
            snapshot,
            // Data: one text row per message.
            |messages: Vec<Message>| {
                any(Column(
                    messages.into_iter().map(|m| any(text(m.subject))).collect(),
                ))
            },
            // Loading: a single spinner line.
            || any(text("Loading messages…")),
            // Failure: a message plus a retry button that re-kicks the load.
            |failure: AppFailure| {
                any(Column(vec![
                    any(text(format!(
                        "Could not load inbox: {}",
                        failure.user_message()
                    ))),
                    any(button::<Arc<InboxController>, _>(
                        "Retry",
                        |controller: &mut Arc<InboxController>| {
                            let handle = Arc::clone(controller);
                            forgekit::spawn_local(async move {
                                handle.load().await;
                            });
                        },
                    )),
                ]))
            },
        )
    }
}

/// The app root: hosts the [`Inbox`] component so its `init` runs under a real
/// component [`Owner`](reactive_graph::owner::Owner) (disposal-on-teardown
/// scoping). `forgekit::run` seeds this root's `()` state and rebuilds it every
/// frame.
#[derive(Default)]
pub struct InboxApp;

impl Component for InboxApp {
    type State = ();

    fn init(&self) {}

    fn build(&self, _state: &mut ()) -> AnyView<()> {
        any(component(Inbox::new()))
    }
}
