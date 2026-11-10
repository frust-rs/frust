//! The live-task table behind the Android backend: pure, so host tests can exercise it.

use std::collections::HashMap;
use std::sync::Arc;

use frust_reactive::TaskState;

/// Live guards by platform id. Ids count up from 1; 0 means "no task", which is what a
/// refused `begin` hands the core guard.
#[derive(Default)]
pub(crate) struct Registry {
    next_id: u64,
    live: HashMap<u64, Arc<TaskState>>,
}

impl Registry {
    /// Track `state` under a fresh id.
    pub(crate) fn allot(&mut self, state: Arc<TaskState>) -> u64 {
        self.next_id += 1;
        self.live.insert(self.next_id, state);
        self.next_id
    }

    /// Stop tracking `id`. `Some` for the first caller only, so whoever gets it owns the
    /// platform end.
    pub(crate) fn take(&mut self, id: u64) -> Option<Arc<TaskState>> {
        self.live.remove(&id)
    }

    /// Mark every live id in `ids` expired and already ended (the platform side is gone,
    /// so a later guard drop must not end it again). Ids no longer tracked, because their
    /// guard already dropped, are skipped. Returns how many were expired.
    pub(crate) fn expire_all(&mut self, ids: &[u64]) -> usize {
        let mut expired = 0;
        for id in ids {
            if let Some(state) = self.live.remove(id) {
                state.set_expired();
                state.mark_ended();
                expired += 1;
            }
        }
        expired
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.live.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{LazyLock, Mutex};

    use frust_reactive::{BackgroundTask, BackgroundTaskBackend, install_background_task_backend};

    use super::*;

    /// Hands out the `TaskState` each guard was begun with: the core keeps its
    /// constructor private, so a real guard is the only way to get one.
    #[derive(Default)]
    struct Capture {
        last: Mutex<Option<Arc<TaskState>>>,
    }

    impl BackgroundTaskBackend for Capture {
        fn begin(&self, _name: &str, state: &Arc<TaskState>) -> u64 {
            *self.last.lock().unwrap() = Some(state.clone());
            99
        }
        fn end(&self, _id: u64, _state: &Arc<TaskState>) {}
    }

    static CAPTURE: LazyLock<Arc<Capture>> = LazyLock::new(Arc::default);
    /// Holds a `begin` and its read of `Capture::last` together across parallel tests.
    static BEGIN: Mutex<()> = Mutex::new(());

    fn guard() -> (BackgroundTask, Arc<TaskState>) {
        let _serial = BEGIN.lock().unwrap_or_else(|e| e.into_inner());
        install_background_task_backend(CAPTURE.clone());
        let task = BackgroundTask::begin("test");
        let state = CAPTURE.last.lock().unwrap().take().unwrap();
        (task, state)
    }

    #[test]
    fn ids_start_at_one_and_never_repeat() {
        let mut registry = Registry::default();
        let (_a, a) = guard();
        let (_b, b) = guard();
        assert_eq!(registry.allot(a), 1);
        assert_eq!(registry.allot(b.clone()), 2);
        registry.take(2);
        assert_eq!(registry.allot(b), 3, "a freed id is not reused");
    }

    #[test]
    fn take_yields_a_state_exactly_once() {
        let mut registry = Registry::default();
        let (_task, state) = guard();
        let id = registry.allot(state.clone());
        assert!(Arc::ptr_eq(&registry.take(id).unwrap(), &state));
        assert!(registry.take(id).is_none());
    }

    #[test]
    fn take_of_the_no_task_id_is_none() {
        let mut registry = Registry::default();
        assert!(registry.take(0).is_none());
    }

    #[test]
    fn expire_all_expires_the_guard_and_claims_its_end() {
        let mut registry = Registry::default();
        let (task, state) = guard();
        let id = registry.allot(state.clone());
        assert!(!task.expired());
        assert_eq!(registry.expire_all(&[id]), 1);
        assert!(task.expired());
        assert!(!state.mark_ended(), "expiry already claimed the end");
        assert!(registry.is_empty());
    }

    #[test]
    fn expire_all_skips_ids_whose_guard_already_ended() {
        let mut registry = Registry::default();
        let (done_task, done) = guard();
        let (live_task, live) = guard();
        let done_id = registry.allot(done);
        let live_id = registry.allot(live);
        registry.take(done_id);
        assert_eq!(registry.expire_all(&[done_id, live_id, 77]), 1);
        assert!(!done_task.expired());
        assert!(live_task.expired());
    }

    #[test]
    fn expire_all_counts_a_repeated_id_once() {
        let mut registry = Registry::default();
        let (_task, state) = guard();
        let id = registry.allot(state);
        assert_eq!(registry.expire_all(&[id, id]), 1);
        assert_eq!(registry.expire_all(&[id]), 0);
    }

    #[test]
    fn is_empty_tracks_the_live_set() {
        let mut registry = Registry::default();
        assert!(registry.is_empty());
        let (_task, state) = guard();
        let id = registry.allot(state);
        assert!(!registry.is_empty());
        registry.take(id);
        assert!(registry.is_empty());
    }
}
