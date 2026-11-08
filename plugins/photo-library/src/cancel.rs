//! Cooperative cancellation and throttled progress for a pick.

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::PickProgress;

type Hook = Box<dyn FnOnce() + Send>;

struct Inner {
    cancelled: AtomicBool,
    hook: Mutex<Option<Hook>>,
}

/// A handle that cancels an in-flight [`PhotoLibrary::pick`](crate::PhotoLibrary::pick).
///
/// Clones share one state. Hand a clone to [`PickOptions::cancel`](crate::PickOptions::cancel)
/// and call [`cancel`](Self::cancel) from any thread. A token that is already cancelled makes
/// the pick return [`PhotoError::Cancelled`](crate::PhotoError::Cancelled) at once.
#[derive(Clone)]
pub struct CancelToken(Arc<Inner>);

impl CancelToken {
    /// A fresh, uncancelled token.
    pub fn new() -> Self {
        Self(Arc::new(Inner {
            cancelled: AtomicBool::new(false),
            hook: Mutex::new(None),
        }))
    }

    /// Cancels the pick. Runs the platform hook registered by the pick, if any, exactly once.
    pub fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::SeqCst);
        let hook = lock(&self.0.hook).take();
        if let Some(hook) = hook {
            hook();
        }
    }

    /// Whether [`cancel`](Self::cancel) has been called.
    pub fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::SeqCst)
    }

    /// Registers the platform hook for one pick. Runs `f` immediately (outside the lock) when
    /// the token is already cancelled.
    #[allow(dead_code)] // Used by the mobile arms.
    pub(crate) fn set_hook(&self, f: impl FnOnce() + Send + 'static) {
        {
            let mut slot = lock(&self.0.hook);
            if !self.is_cancelled() {
                *slot = Some(Box::new(f));
                return;
            }
        }
        f();
    }

    /// Drops the registered hook so a token reused across picks never fires a stale one.
    #[allow(dead_code)] // Used by the mobile arms.
    pub(crate) fn clear_hook(&self) {
        lock(&self.0.hook).take();
    }
}

impl Default for CancelToken {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for CancelToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CancelToken")
            .field("cancelled", &self.is_cancelled())
            .finish()
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Minimum spacing between non-first, non-final deliveries (20 calls per second).
const MIN_INTERVAL: Duration = Duration::from_millis(50);

type Callback = Arc<dyn Fn(PickProgress) + Send + Sync>;
type Clock = Box<dyn Fn() -> Duration + Send + Sync>;

#[derive(Default)]
struct State {
    last_sent: Option<Duration>,
    done: usize,
    fraction: f64,
    finished: bool,
}

/// Wraps the caller's progress callback: clamps, keeps values monotonic and limits the rate.
///
/// The first call and the final call (`done == items`, `fraction == 1.0`) are always delivered.
/// The callback runs outside the internal lock.
#[allow(dead_code)] // Used by the desktop and mobile arms.
pub(crate) struct ProgressThrottle {
    callback: Callback,
    clock: Clock,
    state: Mutex<State>,
}

#[allow(dead_code)] // Used by the desktop and mobile arms.
impl ProgressThrottle {
    pub(crate) fn new(callback: Callback) -> Self {
        let start = Instant::now();
        Self::with_clock(callback, Box::new(move || start.elapsed()))
    }

    /// `clock` returns the time elapsed since an arbitrary fixed origin.
    pub(crate) fn with_clock(callback: Callback, clock: Clock) -> Self {
        Self {
            callback,
            clock,
            state: Mutex::new(State::default()),
        }
    }

    pub(crate) fn report(&self, progress: PickProgress) {
        let now = (self.clock)();
        let out = {
            let mut st = lock(&self.state);
            let items = progress.items;
            let done = progress.done.min(items).max(st.done);
            let raw = if progress.fraction.is_nan() {
                0.0
            } else {
                progress.fraction.clamp(0.0, 1.0)
            };
            let fraction = raw.max(st.fraction);
            let is_final = done == items && fraction >= 1.0;
            if st.finished {
                return;
            }
            st.done = done;
            st.fraction = fraction;
            let due = match st.last_sent {
                None => true,
                Some(t) => now.saturating_sub(t) >= MIN_INTERVAL,
            };
            if !(due || is_final) {
                return;
            }
            st.last_sent = Some(now);
            st.finished = is_final;
            PickProgress {
                items,
                done,
                fraction,
            }
        };
        (self.callback)(out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn hook_fires_once_after_cancel() {
        let t = CancelToken::new();
        let n = Arc::new(AtomicUsize::new(0));
        let n2 = n.clone();
        t.set_hook(move || {
            n2.fetch_add(1, Ordering::SeqCst);
        });
        t.cancel();
        t.cancel();
        assert_eq!(n.load(Ordering::SeqCst), 1);
        assert!(t.is_cancelled());
    }

    #[test]
    fn hook_set_after_cancel_runs_immediately_once() {
        let t = CancelToken::new();
        t.cancel();
        let n = Arc::new(AtomicUsize::new(0));
        let n2 = n.clone();
        t.set_hook(move || {
            n2.fetch_add(1, Ordering::SeqCst);
        });
        t.cancel();
        assert_eq!(n.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn cleared_hook_never_fires() {
        let t = CancelToken::new();
        let n = Arc::new(AtomicUsize::new(0));
        let n2 = n.clone();
        t.set_hook(move || {
            n2.fetch_add(1, Ordering::SeqCst);
        });
        t.clear_hook();
        t.cancel();
        assert_eq!(n.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn clones_share_state_and_debug_shows_flag() {
        let t = CancelToken::new();
        let c = t.clone();
        c.cancel();
        assert!(t.is_cancelled());
        assert_eq!(format!("{t:?}"), "CancelToken { cancelled: true }");
    }

    type Log = Arc<Mutex<Vec<PickProgress>>>;

    fn throttle() -> (ProgressThrottle, Log, Arc<AtomicUsize>) {
        let log: Log = Arc::default();
        let ms = Arc::new(AtomicUsize::new(0));
        let (l, m) = (log.clone(), ms.clone());
        let th = ProgressThrottle::with_clock(
            Arc::new(move |p| l.lock().unwrap().push(p)),
            Box::new(move || Duration::from_millis(m.load(Ordering::SeqCst) as u64)),
        );
        (th, log, ms)
    }

    fn p(items: usize, done: usize, fraction: f64) -> PickProgress {
        PickProgress {
            items,
            done,
            fraction,
        }
    }

    #[test]
    fn first_and_final_always_delivered() {
        let (th, log, _) = throttle();
        th.report(p(3, 0, 0.0));
        th.report(p(3, 1, 0.3));
        th.report(p(3, 3, 1.0));
        let got = log.lock().unwrap().clone();
        assert_eq!(got, vec![p(3, 0, 0.0), p(3, 3, 1.0)]);
    }

    #[test]
    fn rate_is_limited_to_twenty_per_second() {
        let (th, log, ms) = throttle();
        for i in 0..=200usize {
            ms.store(i * 10, Ordering::SeqCst);
            th.report(p(1000, i, i as f64 / 1000.0));
        }
        let n = log.lock().unwrap().len();
        assert!((38..=42).contains(&n), "got {n}");
    }

    #[test]
    fn values_are_clamped_and_monotonic() {
        let (th, log, ms) = throttle();
        th.report(p(4, 2, 0.5));
        ms.store(100, Ordering::SeqCst);
        th.report(p(4, 1, 0.2));
        ms.store(200, Ordering::SeqCst);
        th.report(p(4, 9, f64::NAN));
        ms.store(300, Ordering::SeqCst);
        th.report(p(4, 4, 7.0));
        let got = log.lock().unwrap().clone();
        assert_eq!(
            got,
            vec![p(4, 2, 0.5), p(4, 2, 0.5), p(4, 4, 0.5), p(4, 4, 1.0)]
        );
    }

    #[test]
    fn nothing_after_final() {
        let (th, log, ms) = throttle();
        th.report(p(1, 1, 1.0));
        ms.store(500, Ordering::SeqCst);
        th.report(p(1, 1, 1.0));
        assert_eq!(log.lock().unwrap().len(), 1);
    }
}
