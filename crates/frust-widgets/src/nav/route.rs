//! [`RouteNavigator`]: the thread-safe, context-reachable navigation seam — a
//! queue of *data* requests any code can append to, drained and applied by the
//! [`Router`](super::router::Router) on the UI thread.
//!
//! # Why data, not behaviour
//!
//! `provide_context`/`use_context` require `T: Send + Sync + 'static`, and
//! neither of the router's own types can ever satisfy that: a
//! [`Router`](super::router::Router) holds `Rc<dyn Fn(&RouteParams) -> AnyView>`
//! page builders and a
//! [`NavigatorController`](super::navigator::NavigatorController) holds an
//! `Rc<RefCell<…>>` op queue. A screen therefore cannot reach the router from a
//! callback — only from a parameter its caller threaded down by hand.
//!
//! `RouteNavigator` breaks that by carrying **no behaviour at all**: it is an
//! `Arc<Mutex<Vec<NavRequest>>>` of plain strings plus a `Send + Sync` waker.
//! Anyone, on any thread, appends a [`NavRequest`]; the router drains it in
//! [`Router::pump`](super::router::Router::pump) — which the facade calls at the
//! top of each rebuild — and applies each request against its own route table.
//! Because the queue is plain data, the whole type rides `provide_context`, and
//! the module pins that with a compile-time assertion (below): if a future edit
//! ever puts an `Rc`, a `Router`, or a `State`-generic closure in here, the
//! crate stops compiling rather than silently regressing to unreachable-again.
//!
//! # Threading
//!
//! **Off-thread use is total, not a panic.** Every method here may be called
//! from any thread: appending takes the queue mutex briefly and then fires the
//! waker (outside the lock) so the shell schedules a frame. This is deliberately
//! unlike `frust_reactive::set_can_pop_provider`, which *panics* off the UI
//! thread because it stores an `Rc` in UI-thread-local storage — the whole point
//! of this shape is that a background task can request navigation without
//! marshalling to the UI thread first. A poisoned mutex is recovered
//! (`into_inner`) rather than propagated: a panic elsewhere must not make
//! navigation permanently unusable.
//!
//! # Latency
//!
//! Zero frames. A request appended during an event handler is applied on the
//! very next rebuild: the app's `Component::build` (which calls the facade's
//! `RouterDeepLinks::track`, hence `pump`) runs *before* the navigator's
//! `rebuild`, so the enqueued controller ops are drained in that same reconcile
//! pass.

use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};

use super::path::{Location, RouteParams};

/// A "wake the shell" callback: cheaply cloneable and callable from any thread,
/// mirroring `frust_reactive`'s `FrameWaker`. `frust-widgets` is reactive-free,
/// so the waker stays opaque here — the facade installs
/// `ReactiveRuntime::wake` when it wires the router up.
pub type NavWaker = Arc<dyn Fn() + Send + Sync>;

/// One queued navigation request: pure data (paths, names, params), never a
/// closure or a page. Applied by [`Router::pump`](super::router::Router::pump)
/// against the router's own route table, so the request and the resolution stay
/// on opposite sides of the `Send + Sync` boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NavRequest {
    /// Reset the stack to the matched chain — [`Router::go`](super::router::Router::go).
    Go(String),
    /// Stack the matched leaf — [`Router::push`](super::router::Router::push).
    Push(String),
    /// Replace the top page with the matched leaf —
    /// [`Router::replace`](super::router::Router::replace).
    Replace(String),
    /// Pop the top page — [`Router::pop`](super::router::Router::pop).
    Pop,
    /// [`Router::go_named`](super::router::Router::go_named).
    GoNamed {
        /// The route's `name`.
        name: String,
        /// Params substituted into the named route's pattern; leftovers become
        /// query parameters.
        params: RouteParams,
    },
    /// [`Router::push_named`](super::router::Router::push_named).
    PushNamed {
        /// The route's `name`.
        name: String,
        /// Params substituted into the named route's pattern; leftovers become
        /// query parameters.
        params: RouteParams,
    },
}

/// The shared inner state. Two independent mutexes so publishing a location
/// (router side) never contends with an append (caller side), plus the waker in
/// its own slot because it is replaced at most once, at wiring time.
struct RouteNavInner {
    queue: Mutex<Vec<NavRequest>>,
    waker: Mutex<Option<NavWaker>>,
    location: Mutex<Option<Location>>,
}

/// The context-reachable navigation handle: `Send + Sync + 'static`, cheap to
/// clone (one `Arc`), and safe to hold in app state, in a callback, or under
/// `provide_context`. See the [module docs](self) for why it carries data
/// instead of a router reference.
///
/// Obtain one from
/// [`Router::route_navigator`](super::router::Router::route_navigator); the
/// router it came from applies whatever is queued on its next
/// [`pump`](super::router::Router::pump).
pub struct RouteNavigator {
    inner: Arc<RouteNavInner>,
}

/// The seam's whole point, pinned at compile time: if `RouteNavigator` ever
/// stops being `Send + Sync + 'static` it can no longer ride `provide_context`,
/// and this becomes a compile error rather than a review finding.
const _: fn() = || {
    fn assert<T: Send + Sync + 'static>() {}
    assert::<RouteNavigator>();
};

impl RouteNavigator {
    /// A fresh, empty navigator with no waker installed. Apps get one from
    /// [`Router::route_navigator`](super::router::Router::route_navigator)
    /// rather than constructing it directly.
    pub fn new() -> Self {
        RouteNavigator {
            inner: Arc::new(RouteNavInner {
                queue: Mutex::new(Vec::new()),
                waker: Mutex::new(None),
                location: Mutex::new(None),
            }),
        }
    }

    /// Queue `request` and fire the waker. The single append path every
    /// convenience method below routes through.
    pub fn request(&self, request: NavRequest) {
        lock(&self.inner.queue).push(request);
        self.wake();
    }

    /// Queue a [`NavRequest::Go`] — reset the stack to `location`'s chain.
    pub fn go(&self, location: impl Into<String>) {
        self.request(NavRequest::Go(location.into()));
    }

    /// Queue a [`NavRequest::Push`] — stack `location`'s leaf page.
    pub fn push(&self, location: impl Into<String>) {
        self.request(NavRequest::Push(location.into()));
    }

    /// Queue a [`NavRequest::Replace`] — swap the top page for `location`'s
    /// leaf.
    pub fn replace(&self, location: impl Into<String>) {
        self.request(NavRequest::Replace(location.into()));
    }

    /// Queue a [`NavRequest::Pop`].
    pub fn pop(&self) {
        self.request(NavRequest::Pop);
    }

    /// Queue a [`NavRequest::GoNamed`].
    pub fn go_named(&self, name: impl Into<String>, params: RouteParams) {
        self.request(NavRequest::GoNamed {
            name: name.into(),
            params,
        });
    }

    /// Queue a [`NavRequest::PushNamed`].
    pub fn push_named(&self, name: impl Into<String>, params: RouteParams) {
        self.request(NavRequest::PushNamed {
            name: name.into(),
            params,
        });
    }

    /// The last [`Location`] the router *resolved* (post-redirect), including
    /// the one an unmatched location fell back to the error page with.
    /// `None` before the first navigation.
    ///
    /// Not updated by [`pop`](Self::pop): the router keeps no history stack, so
    /// after a pop this still reports the location that pushed the popped page.
    pub fn location(&self) -> Option<Location> {
        lock(&self.inner.location).clone()
    }

    /// Install the "wake the shell" callback fired after every append. The
    /// facade installs `ReactiveRuntime::wake` when it wires the router; a
    /// navigator with no waker still queues correctly, it just relies on
    /// something else to schedule the next frame.
    pub fn set_waker(&self, waker: NavWaker) {
        *lock(&self.inner.waker) = Some(waker);
    }

    /// Take every queued request in append order, leaving the queue empty.
    /// [`Router::pump`](super::router::Router::pump) is the only caller in
    /// normal use.
    pub fn drain(&self) -> Vec<NavRequest> {
        std::mem::take(&mut *lock(&self.inner.queue))
    }

    /// Publish the location the router just resolved (see
    /// [`location`](Self::location)).
    pub(super) fn set_location(&self, location: Location) {
        *lock(&self.inner.location) = Some(location);
    }

    /// Fire the installed waker, if any — cloned out first so the mutex is not
    /// held across a callback that may re-enter.
    fn wake(&self) {
        let waker = lock(&self.inner.waker).clone();
        if let Some(waker) = waker {
            waker();
        }
    }
}

impl Clone for RouteNavigator {
    fn clone(&self) -> Self {
        RouteNavigator {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl Default for RouteNavigator {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for RouteNavigator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RouteNavigator")
            .field("pending", &lock(&self.inner.queue).len())
            .field("location", &*lock(&self.inner.location))
            .finish_non_exhaustive()
    }
}

/// Lock a slot, recovering from poisoning rather than propagating it (see the
/// [module docs](self)' threading note: a panic elsewhere must not make
/// navigation permanently unusable).
fn lock<T>(slot: &Mutex<T>) -> MutexGuard<'_, T> {
    slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn params(pairs: &[(&str, &str)]) -> RouteParams {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn queues_in_append_order_and_drains_once() {
        let nav = RouteNavigator::new();
        nav.go("/home");
        nav.push("/detail");
        nav.replace("/other");
        nav.pop();
        nav.go_named("user", params(&[("id", "7")]));
        nav.push_named("user", params(&[("id", "8")]));

        assert_eq!(
            nav.drain(),
            vec![
                NavRequest::Go("/home".to_string()),
                NavRequest::Push("/detail".to_string()),
                NavRequest::Replace("/other".to_string()),
                NavRequest::Pop,
                NavRequest::GoNamed {
                    name: "user".to_string(),
                    params: params(&[("id", "7")]),
                },
                NavRequest::PushNamed {
                    name: "user".to_string(),
                    params: params(&[("id", "8")]),
                },
            ]
        );
        // Draining is destructive: a second pump sees nothing.
        assert!(nav.drain().is_empty());
    }

    #[test]
    fn waker_fires_once_per_append() {
        let nav = RouteNavigator::new();
        let woke = Arc::new(AtomicUsize::new(0));
        let counter = woke.clone();
        nav.set_waker(Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        }));

        nav.go("/a");
        nav.pop();
        assert_eq!(woke.load(Ordering::SeqCst), 2);
        // Draining is not an append — it must not wake.
        let _ = nav.drain();
        assert_eq!(woke.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn appends_from_another_thread_are_total() {
        // The shape's whole point: a background thread requests navigation
        // without marshalling to the UI thread, and does not panic doing so.
        let nav = RouteNavigator::new();
        let woke = Arc::new(AtomicUsize::new(0));
        let counter = woke.clone();
        nav.set_waker(Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        }));

        let off_thread = nav.clone();
        std::thread::spawn(move || off_thread.push("/from-a-thread"))
            .join()
            .expect("the off-thread append must not panic");

        assert_eq!(
            nav.drain(),
            vec![NavRequest::Push("/from-a-thread".to_string())]
        );
        assert_eq!(woke.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn clones_share_one_queue() {
        let nav = RouteNavigator::new();
        let other = nav.clone();
        other.go("/shared");
        assert_eq!(nav.drain(), vec![NavRequest::Go("/shared".to_string())]);
    }

    #[test]
    fn location_starts_empty_and_reports_the_last_published() {
        let nav = RouteNavigator::new();
        assert!(nav.location().is_none());
        nav.set_location(Location::parse("/users/42?tab=posts"));
        let loc = nav.location().expect("published");
        assert_eq!(loc.path, "/users/42");
        assert_eq!(loc.query.get("tab").map(String::as_str), Some("posts"));
    }

    #[test]
    fn a_poisoned_queue_still_works() {
        // Recovery, not propagation: a panic elsewhere in the process must not
        // make navigation permanently unusable.
        let nav = RouteNavigator::new();
        let poisoner = nav.clone();
        let _ = std::thread::spawn(move || {
            let _guard = poisoner.inner.queue.lock().expect("fresh mutex");
            panic!("poison the queue");
        })
        .join();

        nav.go("/after-poison");
        assert_eq!(
            nav.drain(),
            vec![NavRequest::Go("/after-poison".to_string())]
        );
    }
}
