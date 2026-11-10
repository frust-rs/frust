//! A platform-neutral "keep the process alive while this short work finishes" guard.
//!
//! [`BackgroundTask`] is a portable face over the platform's background-execution
//! protection — not a scheduler. It does not run anything; it asks the platform not to
//! suspend the process until the guard ends. Every platform is a no-op until the
//! `frust-background-task` plugin is installed, and the plugin provides real backends on
//! iOS (Apple's `beginBackgroundTask`/`endBackgroundTask`) and Android. Without a backend
//! the guard reports itself granted and unexpired: nothing refused it, and nothing
//! protects the work either.
//!
//! # The sanctioned flush pattern
//!
//! Inside a [`Timer::flush_on_background`](crate::Timer::flush_on_background) callback
//! (or an `on_inactive` hook), take the guard on the UI thread, move it into
//! [`spawn_blocking`](crate::spawn_blocking) with the write, and drop it when the write
//! completes:
//!
//! ```ignore
//! let task = BackgroundTask::begin("save");
//! spawn_blocking(move || {
//!     write_to_disk();
//!     drop(task); // or task.end()
//! });
//! ```
//!
//! The pattern needs [`spawn_blocking`](crate::spawn_blocking), which panics on wasm32, so a
//! web build must not use it: do the work some other way there.
//!
//! Apple publishes no numeric budget for the assertion (about 30 s per Apple DTS), so
//! keep the work short. The platform may revoke the assertion early; [`BackgroundTask::expired`]
//! is how an app learns it lost, and it also reads `true` when the platform refused to
//! grant one at all.
//!
//! # Thread contract
//!
//! [`BackgroundTask::begin`] and the two backend installers run on the UI thread (they
//! panic off it once [`ReactiveRuntime::init`](crate::ReactiveRuntime::init) has run).
//! The guard is `Send` and is meant to be dropped on a blocking thread, so
//! [`BackgroundTaskBackend::end`] must be callable from any thread.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::ReactiveRuntime;
use crate::executor::is_ui_thread;

/// State shared between a [`BackgroundTask`] guard and its backend's expiry path.
///
/// Backends drive it through [`set_refused`](Self::set_refused),
/// [`set_expired`](Self::set_expired) and [`mark_ended`](Self::mark_ended).
pub struct TaskState {
    expired: AtomicBool,
    ended: AtomicBool,
    granted: AtomicBool,
}

impl TaskState {
    fn new() -> Self {
        Self {
            expired: AtomicBool::new(false),
            ended: AtomicBool::new(false),
            granted: AtomicBool::new(true),
        }
    }

    /// Record that the platform refused to grant the assertion (call from `begin`).
    pub fn set_refused(&self) {
        self.granted.store(false, Ordering::SeqCst);
    }

    /// Record that the platform revoked the assertion (call from the expiry handler).
    pub fn set_expired(&self) {
        self.expired.store(true, Ordering::SeqCst);
    }

    /// Claim the single right to end the platform task. Returns `true` for the first
    /// caller only; whoever gets `true` must perform the platform end.
    pub fn mark_ended(&self) -> bool {
        !self.ended.swap(true, Ordering::SeqCst)
    }
}

/// The platform half of [`BackgroundTask`], installed by the `frust-background-task` plugin.
pub trait BackgroundTaskBackend: Send + Sync + 'static {
    /// Start a platform assertion named `name` and return an opaque id. Call
    /// [`TaskState::set_refused`] when the platform refuses. On expiry, the backend's
    /// handler calls [`TaskState::set_expired`] and ends the platform task itself when
    /// [`TaskState::mark_ended`] returns `true`. Runs on the UI thread.
    fn begin(&self, name: &str, state: &Arc<TaskState>) -> u64;

    /// End the assertion `id`. May be called from any thread; must be idempotent per id.
    fn end(&self, id: u64, state: &Arc<TaskState>);
}

static BACKEND: Mutex<Option<Arc<dyn BackgroundTaskBackend>>> = Mutex::new(None);

fn backend() -> Option<Arc<dyn BackgroundTaskBackend>> {
    BACKEND.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn assert_ui_thread(entry: &str) {
    if ReactiveRuntime::get().is_some() && !is_ui_thread() {
        panic!(
            "frust-reactive: {entry} was called off the UI thread. Background-task wiring \
             runs on the UI thread (the one `ReactiveRuntime::init` ran on) — this is a \
             wiring bug: begin the task and install the backend there, then move the \
             guard to a blocking thread."
        );
    }
}

/// Install `b` only if no backend is installed yet. Returns whether it was installed.
/// UI-thread-only once the runtime exists.
pub fn install_background_task_backend(b: Arc<dyn BackgroundTaskBackend>) -> bool {
    assert_ui_thread("install_background_task_backend");
    let mut slot = BACKEND.lock().unwrap_or_else(|e| e.into_inner());
    if slot.is_some() {
        return false;
    }
    *slot = Some(b);
    true
}

/// Replace the installed backend (a relaunched shell re-owns it). UI-thread-only once
/// the runtime exists.
pub fn set_background_task_backend(b: Arc<dyn BackgroundTaskBackend>) {
    assert_ui_thread("set_background_task_backend");
    *BACKEND.lock().unwrap_or_else(|e| e.into_inner()) = Some(b);
}

static WARNED_NO_BACKEND: AtomicBool = AtomicBool::new(false);

/// Log once per process that a mobile build has no backend installed.
fn warn_no_backend() {
    if cfg!(any(target_os = "android", target_os = "ios"))
        && !WARNED_NO_BACKEND.swap(true, Ordering::Relaxed)
    {
        log::warn!(
            "frust: BackgroundTask::begin ran with no backend installed, so the work is \
             unprotected; add the `frust-background-task` plugin and call its `install()`"
        );
    }
}

/// A guard that keeps the process alive until it is ended or dropped. See the module
/// docs. Every guard ends its platform task exactly once.
pub struct BackgroundTask {
    id: u64,
    state: Arc<TaskState>,
    backend: Option<Arc<dyn BackgroundTaskBackend>>,
}

impl BackgroundTask {
    /// Begin a background task named `name`. UI-thread-only once the runtime exists.
    /// With no backend installed this is a granted no-op; on Android and iOS it also logs
    /// one warning per process naming the `frust-background-task` plugin.
    ///
    /// Android may show `name` in a notification, so it must not contain user data.
    pub fn begin(name: &str) -> BackgroundTask {
        assert_ui_thread("BackgroundTask::begin");
        let state = Arc::new(TaskState::new());
        let backend = backend();
        let id = match &backend {
            Some(b) => b.begin(name, &state),
            None => {
                warn_no_backend();
                0
            }
        };
        BackgroundTask { id, state, backend }
    }

    /// Whether the work lost its assertion: the platform revoked it or never granted it.
    pub fn expired(&self) -> bool {
        self.state.expired.load(Ordering::SeqCst) || !self.is_granted()
    }

    /// Whether the platform did not refuse the assertion. On a platform with no backend this
    /// reads `true` although nothing protects the work.
    pub fn is_granted(&self) -> bool {
        self.state.granted.load(Ordering::SeqCst)
    }

    /// End the task now (equivalent to dropping the guard).
    pub fn end(self) {
        drop(self);
    }
}

impl Drop for BackgroundTask {
    fn drop(&mut self) {
        if self.state.mark_ended()
            && let Some(b) = &self.backend
        {
            b.end(self.id, &self.state);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::MutexGuard;
    use std::sync::atomic::AtomicU64;

    fn setup() -> MutexGuard<'static, ()> {
        let g = crate::WAKER_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        ReactiveRuntime::init(Arc::new(|| {}));
        *BACKEND.lock().unwrap_or_else(|e| e.into_inner()) = None;
        g
    }

    type Expiry = Box<dyn Fn() + Send + Sync>;

    #[derive(Default)]
    struct Mock {
        begins: Mutex<Vec<String>>,
        ends: Mutex<Vec<(u64, std::thread::ThreadId)>>,
        refuse: bool,
        next: AtomicU64,
        me: Mutex<std::sync::Weak<Mock>>,
        /// Per-id expiry closures, built the way a real backend builds one.
        expiries: Mutex<Vec<(u64, Expiry)>>,
    }

    impl Mock {
        fn arc() -> Arc<Mock> {
            let m = Arc::new(Mock::default());
            *m.me.lock().unwrap() = Arc::downgrade(&m);
            m
        }

        /// Run the platform-expiry handler for task `id`.
        fn fire_expiry(&self, id: u64) {
            let expiries = self.expiries.lock().unwrap();
            let (_, handler) = expiries.iter().find(|(i, _)| *i == id).unwrap();
            handler();
        }
    }

    impl BackgroundTaskBackend for Mock {
        fn begin(&self, name: &str, state: &Arc<TaskState>) -> u64 {
            self.begins.lock().unwrap().push(name.to_owned());
            if self.refuse {
                state.set_refused();
            }
            let id = self.next.fetch_add(1, Ordering::SeqCst) + 7;
            let state = state.clone();
            let this = self.me.lock().unwrap().clone();
            self.expiries.lock().unwrap().push((
                id,
                Box::new(move || {
                    state.set_expired();
                    if state.mark_ended()
                        && let Some(m) = this.upgrade()
                    {
                        m.end(id, &state);
                    }
                }),
            ));
            id
        }
        fn end(&self, id: u64, _state: &Arc<TaskState>) {
            self.ends
                .lock()
                .unwrap()
                .push((id, std::thread::current().id()));
        }
    }

    #[test]
    fn no_backend_is_granted_noop() {
        let _g = setup();
        let t = BackgroundTask::begin("save");
        assert!(t.is_granted() && !t.expired());
        drop(t);
    }

    #[test]
    fn drop_and_explicit_end_each_end_once() {
        let _g = setup();
        let m = Mock::arc();
        assert!(install_background_task_backend(m.clone()));
        let t = BackgroundTask::begin("save");
        assert_eq!(*m.begins.lock().unwrap(), vec!["save".to_owned()]);
        drop(t);
        assert_eq!(m.ends.lock().unwrap().len(), 1);
        assert_eq!(m.ends.lock().unwrap()[0].0, 7);
        BackgroundTask::begin("again").end();
        assert_eq!(m.ends.lock().unwrap().len(), 2);
        assert_eq!(m.ends.lock().unwrap()[1].0, 8);
    }

    #[test]
    fn expiry_path_ends_once() {
        let _g = setup();
        let m = Mock::arc();
        install_background_task_backend(m.clone());
        let t = BackgroundTask::begin("save");
        m.fire_expiry(7);
        assert!(t.expired());
        assert_eq!(m.ends.lock().unwrap().len(), 1);
        drop(t);
        assert_eq!(m.ends.lock().unwrap().len(), 1);
    }

    #[test]
    fn expiry_after_drop_is_a_no_op_and_drop_after_expiry_ends_nothing_twice() {
        let _g = setup();
        let m = Mock::arc();
        install_background_task_backend(m.clone());

        // Drop first: the late expiry finds `ended` already claimed.
        drop(BackgroundTask::begin("first"));
        assert_eq!(m.ends.lock().unwrap().len(), 1);
        m.fire_expiry(7);
        assert_eq!(m.ends.lock().unwrap().len(), 1);

        // Expiry first: the later drop must not end again.
        let t = BackgroundTask::begin("second");
        m.fire_expiry(8);
        assert_eq!(m.ends.lock().unwrap().len(), 2);
        drop(t);
        assert_eq!(m.ends.lock().unwrap().len(), 2);
    }

    #[test]
    fn install_keeps_first_set_replaces() {
        let _g = setup();
        let a = Mock::arc();
        let b = Mock::arc();
        assert!(install_background_task_backend(a.clone()));
        assert!(!install_background_task_backend(b.clone()));
        drop(BackgroundTask::begin("x"));
        assert_eq!(a.begins.lock().unwrap().len(), 1);
        assert!(b.begins.lock().unwrap().is_empty());
        set_background_task_backend(b.clone());
        drop(BackgroundTask::begin("y"));
        assert_eq!(b.begins.lock().unwrap().len(), 1);
        assert_eq!(a.begins.lock().unwrap().len(), 1);
    }

    #[test]
    fn guard_is_send_and_ends_on_other_thread() {
        fn assert_send<T: Send>() {}
        assert_send::<BackgroundTask>();
        let _g = setup();
        let m = Mock::arc();
        install_background_task_backend(m.clone());
        let t = BackgroundTask::begin("save");
        std::thread::spawn(move || drop(t)).join().unwrap();
        let ends = m.ends.lock().unwrap();
        assert_eq!(ends.len(), 1);
        assert_ne!(ends[0].1, std::thread::current().id());
    }

    #[test]
    #[should_panic(expected = "BackgroundTask::begin was called off the UI thread")]
    fn begin_off_ui_thread_panics() {
        let _g = setup();
        let r = std::thread::spawn(|| {
            BackgroundTask::begin("save");
        })
        .join();
        if let Err(e) = r {
            std::panic::resume_unwind(e);
        }
    }

    #[test]
    fn refused_reads_expired() {
        let _g = setup();
        let m = Arc::new(Mock {
            refuse: true,
            ..Mock::default()
        });
        install_background_task_backend(m);
        let t = BackgroundTask::begin("save");
        assert!(!t.is_granted());
        assert!(t.expired());
    }
}
