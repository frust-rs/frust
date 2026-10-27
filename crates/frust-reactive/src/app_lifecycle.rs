//! Process-wide **app-lifecycle** source with Flutter's `AppLifecycleState` /
//! `AppLifecycleListener` semantics: a shell reports the state its host is now in via
//! [`push_app_lifecycle`] and asks a cancelable exit via [`request_app_exit`]; app code
//! observes through [`app_lifecycle`] (a tracked [`RwSignal`] read) or the callbacks of an
//! [`AppLifecycleListener`]. The `frust` facade re-exports every public name here.
//!
//! # Thread contract
//!
//! Callbacks run **synchronously on the UI thread, inside the shell's platform callback**
//! (`onPause`, `applicationDidEnterBackground`, a winit event): never block in one — hand
//! the work off (a channel send, `spawn`). Once [`ReactiveRuntime::init`] has run, both
//! shell entry points panic off the UI thread (the `push_deep_link` wiring-bug convention).
//! Before init no thread is the UI thread yet, so a push is accepted as-is: the process
//! mirror and every listener see it, only the signal write is skipped, and the signal is
//! seeded from the mirror when first created. [`AppLifecycleListener::new`] works on any
//! thread, before or after init.
//!
//! Every callback runs under `catch_unwind`: a panic is logged and never stops the
//! remaining callbacks (a `panic = "abort"` build still aborts). Callbacks may create or
//! dispose listeners — no registry lock is held while one runs, and each step dispatches
//! over a snapshot, so a listener created inside a callback first hears the next step and a
//! listener disposed inside one is not called again. A callback must not itself push.

use std::any::Any;
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use reactive_graph::signal::RwSignal;
use reactive_graph::traits::{Get, GetUntracked, Set};

use crate::ReactiveRuntime;
use crate::executor::is_ui_thread;

/// Where the host application is in its lifecycle — Flutter's `AppLifecycleState`. The
/// declaration order is load-bearing: [`push_app_lifecycle`] walks it to generate the
/// intermediate states between two pushes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AppLifecycleState {
    /// No host view exists yet, or it is gone — the initial state, and the final one.
    Detached,
    /// A view is visible and has input focus.
    Resumed,
    /// A view is visible but has no input focus (a system dialog, an unfocused window).
    Inactive,
    /// No view is visible (a minimized desktop window, a hidden web tab); on iOS and
    /// Android it is synthesized between `Inactive` and `Paused`.
    Hidden,
    /// Not visible and not responding to input (iOS and Android only).
    Paused,
}

impl AppLifecycleState {
    const ORDER: [Self; 5] = [
        Self::Detached,
        Self::Resumed,
        Self::Inactive,
        Self::Hidden,
        Self::Paused,
    ];

    fn index(self) -> usize {
        self as usize
    }
}

/// A listener's answer to an exit request — Flutter's `AppExitResponse`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AppExitResponse {
    /// Let the application exit.
    Exit,
    /// Keep the application running (unsaved work, a pending confirmation).
    Cancel,
}

/// The [`MIRROR`] value meaning "no push has happened in this process yet".
const NO_PUSH: u8 = AppLifecycleState::ORDER.len() as u8;

/// The last pushed state's index; an atomic so it works before the runtime exists.
static MIRROR: AtomicU8 = AtomicU8::new(NO_PUSH);

static SIGNAL: OnceLock<RwSignal<AppLifecycleState>> = OnceLock::new();

fn pushed_state() -> Option<AppLifecycleState> {
    let index = usize::from(MIRROR.load(Ordering::SeqCst));
    AppLifecycleState::ORDER.get(index).copied()
}

fn signal(rt: &ReactiveRuntime) -> RwSignal<AppLifecycleState> {
    *SIGNAL.get_or_init(|| {
        let seed = pushed_state().unwrap_or(AppLifecycleState::Detached);
        rt.with_owner(|| RwSignal::new(seed))
    })
}

/// The app-facing lifecycle read surface, obtained via [`app_lifecycle`].
#[derive(Clone, Copy, Debug)]
pub struct AppLifecycle {
    state: RwSignal<AppLifecycleState>,
}

impl AppLifecycle {
    /// The current state as a **tracked** read: a rebuild (any `TrackedScope`) that calls
    /// this re-runs on every lifecycle step a shell pushes.
    pub fn state(&self) -> AppLifecycleState {
        self.state.get()
    }

    /// The current state without subscribing — for non-rendering reads only.
    pub fn state_untracked(&self) -> AppLifecycleState {
        self.state.get_untracked()
    }
}

/// The process-wide lifecycle state. It is [`AppLifecycleState::Detached`] until a shell's
/// first push. Call on the UI thread.
///
/// # Panics
///
/// Panics if [`ReactiveRuntime::init`] has not run (the `deep_links` convention).
pub fn app_lifecycle() -> AppLifecycle {
    let rt = ReactiveRuntime::get().expect(
        "frust-reactive: app_lifecycle() was called before ReactiveRuntime::init — an app \
         must run under the Frust facade's entry point (which initializes the reactive \
         runtime) before reading the app lifecycle",
    );
    AppLifecycle { state: signal(rt) }
}

type Hook = Arc<dyn Fn() + Send + Sync>;
type StateHook = Arc<dyn Fn(AppLifecycleState) + Send + Sync>;
type ExitHook = Arc<dyn Fn() -> AppExitResponse + Send + Sync>;

#[derive(Default)]
struct Callbacks {
    show: Option<Hook>,
    resume: Option<Hook>,
    hide: Option<Hook>,
    inactive: Option<Hook>,
    pause: Option<Hook>,
    restart: Option<Hook>,
    detach: Option<Hook>,
    state_change: Option<StateHook>,
    exit_requested: Option<ExitHook>,
    disposed: bool,
}

type Slot = Arc<Mutex<Callbacks>>;

static LISTENERS: Mutex<Vec<(u64, Slot)>> = Mutex::new(Vec::new());
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn snapshot() -> Vec<Slot> {
    lock(&LISTENERS)
        .iter()
        .map(|(_, slot)| Arc::clone(slot))
        .collect()
}

/// Clones one callback out of a live slot, so it runs without the slot's lock held.
fn live<T>(slot: &Slot, pick: impl FnOnce(&Callbacks) -> Option<T>) -> Option<T> {
    let callbacks = lock(slot);
    if callbacks.disposed {
        None
    } else {
        pick(&callbacks)
    }
}

/// Observes the process-wide app lifecycle — Flutter's `AppLifecycleListener`. Registered
/// on construction; each `on_*` method installs (or replaces) one callback. Callbacks run
/// in registration order, and a step's transition callback runs before its
/// `on_state_change`. Dropping the listener (or [`dispose`](Self::dispose)) unregisters it.
#[must_use = "dropping an AppLifecycleListener unregisters it; keep it alive while its callbacks should run"]
pub struct AppLifecycleListener {
    id: u64,
    slot: Slot,
}

impl AppLifecycleListener {
    /// Register a listener with no callbacks. Works on any thread, before or after
    /// [`ReactiveRuntime::init`].
    pub fn new() -> Self {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let slot = Slot::default();
        lock(&LISTENERS).push((id, Arc::clone(&slot)));
        Self { id, slot }
    }

    fn install(self, set: impl FnOnce(&mut Callbacks)) -> Self {
        set(&mut lock(&self.slot));
        self
    }

    /// `Inactive` reached from `Hidden`: a view is visible again.
    pub fn on_show(self, f: impl Fn() + Send + Sync + 'static) -> Self {
        self.install(|c| c.show = Some(Arc::new(f)))
    }

    /// `Resumed` reached: visible with input focus.
    pub fn on_resume(self, f: impl Fn() + Send + Sync + 'static) -> Self {
        self.install(|c| c.resume = Some(Arc::new(f)))
    }

    /// `Hidden` reached from `Inactive` (or as the first state): no view is visible.
    pub fn on_hide(self, f: impl Fn() + Send + Sync + 'static) -> Self {
        self.install(|c| c.hide = Some(Arc::new(f)))
    }

    /// `Inactive` reached from `Resumed` (or as the first state): input focus lost.
    pub fn on_inactive(self, f: impl Fn() + Send + Sync + 'static) -> Self {
        self.install(|c| c.inactive = Some(Arc::new(f)))
    }

    /// `Paused` reached from `Hidden` (or as the first state).
    pub fn on_pause(self, f: impl Fn() + Send + Sync + 'static) -> Self {
        self.install(|c| c.pause = Some(Arc::new(f)))
    }

    /// `Hidden` reached from `Paused`: the app is coming back from the background.
    pub fn on_restart(self, f: impl Fn() + Send + Sync + 'static) -> Self {
        self.install(|c| c.restart = Some(Arc::new(f)))
    }

    /// `Detached` reached: the host view is gone.
    pub fn on_detach(self, f: impl Fn() + Send + Sync + 'static) -> Self {
        self.install(|c| c.detach = Some(Arc::new(f)))
    }

    /// Every step, after that step's transition callback.
    pub fn on_state_change(self, f: impl Fn(AppLifecycleState) + Send + Sync + 'static) -> Self {
        self.install(|c| c.state_change = Some(Arc::new(f)))
    }

    /// Asked by [`request_app_exit`]; a listener without one answers `Exit`.
    pub fn on_exit_requested(
        self,
        f: impl Fn() -> AppExitResponse + Send + Sync + 'static,
    ) -> Self {
        self.install(|c| c.exit_requested = Some(Arc::new(f)))
    }

    /// Unregister now — the explicit form of dropping the listener.
    pub fn dispose(self) {
        drop(self);
    }
}

impl Default for AppLifecycleListener {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for AppLifecycleListener {
    fn drop(&mut self) {
        lock(&self.slot).disposed = true;
        lock(&LISTENERS).retain(|(id, _)| *id != self.id);
    }
}

impl fmt::Debug for AppLifecycleListener {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AppLifecycleListener")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

/// The steps from `current` (`None` before the first push) to `target` — Flutter's
/// `_generateStateTransitions`.
fn transitions(
    current: Option<AppLifecycleState>,
    target: AppLifecycleState,
) -> Vec<AppLifecycleState> {
    let order = &AppLifecycleState::ORDER;
    match current {
        Some(current) if current == target => Vec::new(),
        None => vec![target],
        Some(current) if target == AppLifecycleState::Detached => order[current.index() + 1..]
            .iter()
            .copied()
            .chain([AppLifecycleState::Detached])
            .collect(),
        Some(current) if current.index() > target.index() => order[target.index()..current.index()]
            .iter()
            .rev()
            .copied()
            .collect(),
        Some(current) => order[current.index() + 1..=target.index()].to_vec(),
    }
}

/// The transition callback for one step — Flutter's `didChangeAppLifecycleState`.
fn transition_hook(
    callbacks: &Callbacks,
    previous: Option<AppLifecycleState>,
    step: AppLifecycleState,
) -> Option<(&'static str, Hook)> {
    use AppLifecycleState::{Detached, Hidden, Inactive, Paused, Resumed};
    let (name, hook) = match (step, previous) {
        (Resumed, _) => ("on_resume", &callbacks.resume),
        (Inactive, Some(Hidden)) => ("on_show", &callbacks.show),
        (Inactive, None | Some(Resumed)) => ("on_inactive", &callbacks.inactive),
        (Hidden, Some(Paused)) => ("on_restart", &callbacks.restart),
        (Hidden, None | Some(Inactive)) => ("on_hide", &callbacks.hide),
        (Paused, None | Some(Hidden)) => ("on_pause", &callbacks.pause),
        (Detached, _) => ("on_detach", &callbacks.detach),
        _ => return None,
    };
    hook.clone().map(|hook| (name, hook))
}

fn guarded<R>(name: &str, f: impl FnOnce() -> R) -> Option<R> {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(value) => Some(value),
        Err(payload) => {
            eprintln!(
                "frust-reactive: an AppLifecycleListener {name} callback panicked ({}); the \
                 panic was caught and the remaining callbacks still run",
                panic_message(&*payload)
            );
            None
        }
    }
}

fn panic_message(payload: &(dyn Any + Send)) -> &str {
    payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("non-string payload")
}

fn assert_ui_thread(entry: &str) {
    if ReactiveRuntime::get().is_some() && !is_ui_thread() {
        panic!(
            "frust-reactive: {entry} was called off the UI thread. App-lifecycle events can \
             only be delivered on the UI thread (the one `ReactiveRuntime::init` ran on) — \
             this is a wiring bug: route the platform callback through the UI thread first, \
             the same contract `Executor::spawn_local` enforces."
        );
    }
}

/// Deliver the state the host is now in. Shell-facing; panics off the UI thread once the
/// runtime exists (see the module docs' thread contract).
///
/// Steps follow Flutter's `_generateStateTransitions`: the same state yields none; the
/// first push in a process jumps straight to `target`; `Detached` walks every state after
/// the current one, then `Detached`; an earlier `target` walks back through the states in
/// between; a later one walks forward. Each step updates the state, then each listener
/// (registration order) gets its transition callback and `on_state_change`: `Resumed` →
/// `on_resume`; `Inactive` from `Hidden` → `on_show`, from `Resumed`/none → `on_inactive`;
/// `Hidden` from `Paused` → `on_restart`, from `Inactive`/none → `on_hide`; `Paused` from
/// `Hidden`/none → `on_pause`; `Detached` → `on_detach`.
pub fn push_app_lifecycle(target: AppLifecycleState) {
    assert_ui_thread("push_app_lifecycle");
    push(ReactiveRuntime::get(), target);
}

fn push(rt: Option<&ReactiveRuntime>, target: AppLifecycleState) {
    let mut previous = pushed_state();
    for step in transitions(previous, target) {
        MIRROR.store(step.index() as u8, Ordering::SeqCst);
        match rt {
            Some(rt) => signal(rt).set(step),
            None if cfg!(debug_assertions) => eprintln!(
                "frust-reactive: push_app_lifecycle({step:?}) before ReactiveRuntime::init — \
                 the mirror and listeners saw it; the signal is seeded from it once created"
            ),
            None => {}
        }
        for slot in snapshot() {
            if let Some((name, hook)) = live(&slot, |c| transition_hook(c, previous, step)) {
                guarded(name, || hook());
            }
            if let Some(hook) = live(&slot, |c| c.state_change.clone()) {
                guarded("on_state_change", || hook(step));
            }
        }
        previous = Some(step);
    }
}

/// Ask every listener whether the app may exit — Flutter's `handleRequestAppExit`.
/// Shell-facing: a desktop shell calls it only for a cancelable exit (a window close, the
/// Quit menu item), never on mobile or web. Panics off the UI thread once the runtime exists.
///
/// Every registered `on_exit_requested` is called, in registration order, without
/// short-circuiting; the answer is `Cancel` if any returned `Cancel`, else `Exit` (also
/// with no handler). A panicking handler is logged and counts as `Exit`. Unlike Flutter's
/// `Future`, this is synchronous: decide on the spot and hand slow work off.
pub fn request_app_exit() -> AppExitResponse {
    assert_ui_thread("request_app_exit");
    let mut response = AppExitResponse::Exit;
    for slot in snapshot() {
        let Some(hook) = live(&slot, |c| c.exit_requested.clone()) else {
            continue;
        };
        if guarded("on_exit_requested", || hook()) == Some(AppExitResponse::Cancel) {
            response = AppExitResponse::Cancel;
        }
    }
    response
}

/// Forget every push, so a test starts from "no push yet". Callers hold `WAKER_TEST_LOCK`.
#[cfg(test)]
fn reset_for_test() {
    MIRROR.store(NO_PUSH, Ordering::SeqCst);
    if let Some(signal) = SIGNAL.get() {
        signal.set(AppLifecycleState::Detached);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TrackedScope;
    use AppLifecycleState::{Detached, Hidden, Inactive, Paused, Resumed};

    type Log = Arc<Mutex<Vec<String>>>;

    /// Serializes on the crate's global-state lock, marks this thread as the UI thread and
    /// forgets earlier pushes.
    fn setup() -> (MutexGuard<'static, ()>, Log) {
        let guard = crate::WAKER_TEST_LOCK
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        ReactiveRuntime::init(Arc::new(|| {}));
        reset_for_test();
        (guard, Log::default())
    }

    fn note(log: &Log, entry: impl Into<String>) {
        lock(log).push(entry.into());
    }

    fn take(log: &Log) -> Vec<String> {
        std::mem::take(&mut *lock(log))
    }

    /// A listener logging `tag:<callback>` for every lifecycle callback.
    fn recorder(tag: &'static str, log: &Log) -> AppLifecycleListener {
        let hook = |name: &'static str| {
            let log = Arc::clone(log);
            move || note(&log, format!("{tag}:{name}"))
        };
        let state_log = Arc::clone(log);
        AppLifecycleListener::new()
            .on_show(hook("show"))
            .on_resume(hook("resume"))
            .on_hide(hook("hide"))
            .on_inactive(hook("inactive"))
            .on_pause(hook("pause"))
            .on_restart(hook("restart"))
            .on_detach(hook("detach"))
            .on_state_change(move |s| note(&state_log, format!("{tag}:state:{s:?}")))
    }

    fn exit_handler(tag: &'static str, log: &Log, answer: AppExitResponse) -> AppLifecycleListener {
        let log = Arc::clone(log);
        AppLifecycleListener::new().on_exit_requested(move || {
            note(&log, tag);
            answer
        })
    }

    fn expect(log: &Log, entries: &[&str]) {
        assert_eq!(take(log), entries);
    }

    #[test]
    fn transition_table_matches_flutter() {
        assert_eq!(transitions(None, Resumed), [Resumed]);
        assert_eq!(transitions(None, Paused), [Paused]);
        assert_eq!(
            transitions(Some(Resumed), Paused),
            [Inactive, Hidden, Paused]
        );
        assert_eq!(
            transitions(Some(Paused), Resumed),
            [Hidden, Inactive, Resumed]
        );
        assert_eq!(
            transitions(Some(Inactive), Detached),
            [Hidden, Paused, Detached]
        );
        assert_eq!(
            transitions(Some(Resumed), Detached),
            [Inactive, Hidden, Paused, Detached]
        );
        assert_eq!(transitions(Some(Paused), Detached), [Detached]);
        assert_eq!(transitions(Some(Detached), Inactive), [Resumed, Inactive]);
        for state in AppLifecycleState::ORDER {
            assert!(transitions(Some(state), state).is_empty());
        }
    }

    #[test]
    fn first_push_jumps_straight_to_the_target() {
        let (_guard, log) = setup();
        let _a = recorder("a", &log);
        push_app_lifecycle(Resumed);
        expect(&log, &["a:resume", "a:state:Resumed"]);
        assert_eq!(app_lifecycle().state_untracked(), Resumed);
    }

    #[test]
    fn resumed_to_paused_walks_inactive_and_hidden() {
        let (_guard, log) = setup();
        let _a = recorder("a", &log);
        push_app_lifecycle(Resumed);
        take(&log);
        push_app_lifecycle(Paused);
        expect(
            &log,
            &[
                "a:inactive",
                "a:state:Inactive",
                "a:hide",
                "a:state:Hidden",
                "a:pause",
                "a:state:Paused",
            ],
        );
    }

    #[test]
    fn paused_to_resumed_walks_back_with_restart_and_show() {
        let (_guard, log) = setup();
        let _a = recorder("a", &log);
        push_app_lifecycle(Paused);
        take(&log);
        push_app_lifecycle(Resumed);
        expect(
            &log,
            &[
                "a:restart",
                "a:state:Hidden",
                "a:show",
                "a:state:Inactive",
                "a:resume",
                "a:state:Resumed",
            ],
        );
    }

    #[test]
    fn inactive_round_trip_never_restarts() {
        let (_guard, log) = setup();
        let _a = recorder("a", &log);
        push_app_lifecycle(Resumed);
        push_app_lifecycle(Inactive);
        push_app_lifecycle(Resumed);
        expect(
            &log,
            &[
                "a:resume",
                "a:state:Resumed",
                "a:inactive",
                "a:state:Inactive",
                "a:resume",
                "a:state:Resumed",
            ],
        );
    }

    #[test]
    fn detach_walks_forward_then_detaches() {
        let (_guard, log) = setup();
        let _a = recorder("a", &log);
        push_app_lifecycle(Inactive);
        take(&log);
        push_app_lifecycle(Detached);
        expect(
            &log,
            &[
                "a:hide",
                "a:state:Hidden",
                "a:pause",
                "a:state:Paused",
                "a:detach",
                "a:state:Detached",
            ],
        );
        push_app_lifecycle(Paused);
        take(&log);
        push_app_lifecycle(Detached);
        expect(&log, &["a:detach", "a:state:Detached"]);
    }

    #[test]
    fn same_state_push_is_silent() {
        let (_guard, log) = setup();
        let _a = recorder("a", &log);
        push_app_lifecycle(Hidden);
        take(&log);
        push_app_lifecycle(Hidden);
        expect(&log, &[]);
    }

    #[test]
    fn listeners_run_in_registration_order_transition_first() {
        let (_guard, log) = setup();
        let _a = recorder("a", &log);
        let _b = recorder("b", &log);
        push_app_lifecycle(Resumed);
        expect(
            &log,
            &["a:resume", "a:state:Resumed", "b:resume", "b:state:Resumed"],
        );
    }

    #[test]
    fn a_later_callback_replaces_the_earlier_one() {
        let (_guard, log) = setup();
        let (first, second) = (Arc::clone(&log), Arc::clone(&log));
        let _a = AppLifecycleListener::new()
            .on_resume(move || note(&first, "first"))
            .on_resume(move || note(&second, "second"));
        push_app_lifecycle(Resumed);
        expect(&log, &["second"]);
    }

    #[test]
    fn dispose_and_drop_unregister() {
        let (_guard, log) = setup();
        recorder("disposed", &log).dispose();
        drop(recorder("dropped", &log));
        let _kept = AppLifecycleListener::default();
        push_app_lifecycle(Resumed);
        expect(&log, &[]);
        assert_eq!(
            lock(&LISTENERS).len(),
            1,
            "only the kept listener stays registered"
        );
    }

    #[test]
    fn callbacks_can_register_and_dispose_listeners() {
        let (_guard, log) = setup();
        let victim_slot: Arc<Mutex<Option<AppLifecycleListener>>> = Arc::default();
        let born: Arc<Mutex<Vec<AppLifecycleListener>>> = Arc::default();
        let (victim, born_in, born_log) = (
            Arc::clone(&victim_slot),
            Arc::clone(&born),
            Arc::clone(&log),
        );
        let _actor = AppLifecycleListener::new().on_state_change(move |_| {
            if let Some(listener) = lock(&victim).take() {
                listener.dispose();
            }
            let born_log = Arc::clone(&born_log);
            let child = AppLifecycleListener::new()
                .on_state_change(move |s| note(&born_log, format!("born:{s:?}")));
            lock(&born_in).push(child);
        });
        *lock(&victim_slot) = Some(recorder("victim", &log));
        push_app_lifecycle(Resumed);
        expect(&log, &[]);
        push_app_lifecycle(Inactive);
        expect(&log, &["born:Inactive"]);
        assert_eq!(lock(&born).len(), 2);
    }

    #[test]
    fn a_panicking_callback_does_not_stop_the_others() {
        let (_guard, log) = setup();
        let state_log = Arc::clone(&log);
        let _a = AppLifecycleListener::new()
            .on_resume(|| panic!("resume handler failed"))
            .on_state_change(move |s| note(&state_log, format!("a:state:{s:?}")));
        let _b = recorder("b", &log);
        push_app_lifecycle(Resumed);
        expect(&log, &["a:state:Resumed", "b:resume", "b:state:Resumed"]);
    }

    #[test]
    fn registration_and_a_pre_init_push_need_no_runtime() {
        let (_guard, log) = setup();
        let listener = std::thread::spawn({
            let log = Arc::clone(&log);
            move || recorder("a", &log)
        })
        .join()
        .expect("registering off the UI thread must not panic");
        let lifecycle = app_lifecycle();
        push(None, Resumed);
        expect(&log, &["a:resume", "a:state:Resumed"]);
        assert_eq!(pushed_state(), Some(Resumed));
        assert_eq!(lifecycle.state_untracked(), Detached);
        drop(listener);
    }

    #[test]
    fn a_tracked_state_read_reruns_on_a_push() {
        let (_guard, _log) = setup();
        let scope = TrackedScope::new();
        assert_eq!(scope.track(|| app_lifecycle().state()), Detached);
        assert!(!scope.is_dirty());
        push_app_lifecycle(Resumed);
        assert!(
            scope.is_dirty(),
            "a push must dirty a scope tracking state()"
        );
        assert_eq!(scope.track(|| app_lifecycle().state()), Resumed);
    }

    #[test]
    #[should_panic(expected = "wiring bug")]
    fn push_off_the_ui_thread_panics() {
        let (_guard, _log) = setup();
        std::thread::spawn(|| push_app_lifecycle(Resumed))
            .join()
            .unwrap_or_else(|e| std::panic::resume_unwind(e));
    }

    #[test]
    fn exit_with_no_handlers_is_exit() {
        let (_guard, log) = setup();
        let _a = recorder("a", &log);
        assert_eq!(request_app_exit(), AppExitResponse::Exit);
    }

    #[test]
    fn one_cancel_wins_and_every_handler_is_asked() {
        let (_guard, log) = setup();
        let _a = exit_handler("a", &log, AppExitResponse::Exit);
        let _b = exit_handler("b", &log, AppExitResponse::Cancel);
        let _c = exit_handler("c", &log, AppExitResponse::Exit);
        assert_eq!(request_app_exit(), AppExitResponse::Cancel);
        expect(&log, &["a", "b", "c"]);
    }

    #[test]
    fn a_panicking_exit_handler_counts_as_exit() {
        let (_guard, log) = setup();
        let _a = AppLifecycleListener::new().on_exit_requested(|| panic!("exit handler failed"));
        let _b = exit_handler("b", &log, AppExitResponse::Exit);
        assert_eq!(request_app_exit(), AppExitResponse::Exit);
        expect(&log, &["b"]);
    }

    #[test]
    fn a_disposed_listener_is_not_asked_to_exit() {
        let (_guard, log) = setup();
        exit_handler("gone", &log, AppExitResponse::Cancel).dispose();
        assert_eq!(request_app_exit(), AppExitResponse::Exit);
        expect(&log, &[]);
    }
}
