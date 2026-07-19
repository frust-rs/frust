//! [`TrackedScope`]: a custom `reactive_graph` subscriber that runs a frame
//! rebuild with dependency tracking and bridges "a tracked signal changed" into
//! a single, coalesced [`FrameWaker`] fire — **without** the `effects` cargo
//! feature and **without** the executor in the frame path.
//!
//! # How it works
//!
//! `reactive_graph`'s reactive graph is source/subscriber: reading a signal
//! under an ambient [`Observer`] records the signal as a *source* of that
//! observer and the observer as a *subscriber* of the signal (see
//! `reactive_graph::traits::Track`). [`TrackedScope`] *is* such an observer: it
//! implements [`Subscriber`]/[`ReactiveNode`], and [`TrackedScope::track`] runs
//! the caller's closure with itself installed as the observer via
//! [`WithObserver`]. Every signal read inside `track` therefore subscribes this
//! scope to that signal.
//!
//! When any tracked source later changes it notifies this scope
//! ([`ReactiveNode::mark_dirty`] for a directly-read signal, or
//! [`ReactiveNode::mark_check`] for a value reached through a memo — a signal
//! write marks the memo `mark_dirty`, and the memo relays `mark_check` to *its*
//! subscribers). We treat **both** as "something changed": set the dirty flag
//! and, on the clean→dirty edge only, fire the frame waker. That edge-triggering
//! is the coalescing: N signal writes before the next `track` produce exactly
//! one wake. `mark_check` waking is deliberately conservative — a check that
//! resolves to no change wakes spuriously (wasteful but correct); a missed real
//! change would be a bug.
//!
//! This is the *custom subscriber* mechanism, chosen over the `RenderEffect`
//! fallback: it is synchronous, needs no `any_spawner` executor in the frame
//! path, and the `reactive_graph::graph` traits are all publicly reachable, so
//! there is no orphan/visibility wall forcing the fallback.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use reactive_graph::graph::{
    AnySource, AnySubscriber, ReactiveNode, Source, Subscriber, WithObserver,
};

use crate::ReactiveRuntime;

/// A reactive observer for a single frame-rebuild pass.
///
/// Run the rebuild under [`track`](Self::track); afterwards any change to a
/// signal read during that pass sets the dirty flag ([`is_dirty`](Self::is_dirty))
/// and fires the process-wide frame waker once (coalesced). Re-running `track`
/// re-records dependencies from scratch, so a signal read in one pass but not
/// the next stops dirtying the scope.
///
/// Cheap to clone-around by holding the inner `Arc`; construct one per
/// long-lived rebuild target (e.g. one per app root / retained component).
pub struct TrackedScope {
    inner: Arc<ScopeInner>,
}

/// The shared, `Send + Sync` heart of a [`TrackedScope`]. Held behind an `Arc`
/// so a `Weak<dyn Subscriber + Send + Sync>` can be handed to every source's
/// subscriber list (dirty notifications may arrive from any thread).
struct ScopeInner {
    /// The sources (signals/memos) read during the last [`TrackedScope::track`].
    /// `reactive_graph`'s own `SourceSet` is `pub(crate)`, so we keep our own
    /// de-duplicated list; on re-track we walk it to unsubscribe from each
    /// source before recording the new pass's reads.
    sources: Mutex<Vec<AnySource>>,
    /// Set when any tracked source notifies; cleared at the start of `track`.
    /// The clean→dirty edge is what fires the waker (coalescing).
    dirty: AtomicBool,
}

impl ScopeInner {
    fn new() -> Self {
        Self {
            sources: Mutex::new(Vec::new()),
            dirty: AtomicBool::new(false),
        }
    }

    /// Marks the scope dirty and, on the clean→dirty edge only, fires the frame
    /// waker. Callable from any thread (signal writes may originate anywhere).
    ///
    /// Also trips the process-wide signals-dirty flag
    /// (`ReactiveRuntime::mark_signals_dirty`) on *every* call, not just the
    /// clean→dirty edge below — that flag is a plain bool a shell drains once
    /// per frame (`ReactiveRuntime::take_signals_dirty`), so any tracked
    /// scope's invalidation should trip it, coalesced by nature since it has
    /// no "already set" distinction to preserve.
    fn notify_dirty(&self) {
        let rt = ReactiveRuntime::get();
        if let Some(rt) = rt {
            rt.mark_signals_dirty();
        }
        // `swap` gives us the previous value atomically: only the thread that
        // observed `false` (the clean→dirty transition) fires the waker, so N
        // concurrent or sequential writes between tracks coalesce to one wake.
        let was_dirty = self.dirty.swap(true, Ordering::SeqCst);
        if !was_dirty && let Some(rt) = rt {
            rt.wake();
        }
    }
}

impl ReactiveNode for ScopeInner {
    /// A directly-read signal that changed notifies us here.
    fn mark_dirty(&self) {
        self.notify_dirty();
    }

    /// A value reached through a memo relays `mark_check` to us when the memo's
    /// upstream changed. We can't cheaply prove the memo's output actually
    /// changed without recomputing it, so we wake — spurious wakes are correct,
    /// missed changes are not.
    fn mark_check(&self) {
        self.notify_dirty();
    }

    /// A [`TrackedScope`] is a leaf observer: nothing subscribes to *it*, so it
    /// has no downstream to propagate a check to.
    fn mark_subscribers_check(&self) {}

    /// Reports whether the scope has been marked dirty since the last `track`.
    fn update_if_necessary(&self) -> bool {
        self.dirty.load(Ordering::SeqCst)
    }
}

impl Subscriber for ScopeInner {
    /// Records a signal/memo read during `track` as a source of this scope.
    /// De-duplicated so reading the same signal twice in one pass records it
    /// once.
    fn add_source(&self, source: AnySource) {
        let mut sources = self.sources.lock().expect("TrackedScope sources poisoned");
        if !sources.contains(&source) {
            sources.push(source);
        }
    }

    /// Drops every recorded source, unsubscribing this scope from each so a
    /// source read in a previous pass but not the current one can no longer
    /// dirty us.
    fn clear_sources(&self, subscriber: &AnySubscriber) {
        let drained: Vec<AnySource> = {
            let mut sources = self.sources.lock().expect("TrackedScope sources poisoned");
            std::mem::take(&mut *sources)
        };
        for source in drained {
            source.remove_subscriber(subscriber);
        }
    }
}

impl TrackedScope {
    /// Creates an empty scope with no tracked sources and a clean dirty flag.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(ScopeInner::new()),
        }
    }

    /// Builds the type-erased subscriber handle for this scope.
    ///
    /// The `reactive_graph::ToAnySubscriber` trait can't be implemented for
    /// `Arc<ScopeInner>` here (orphan rule: both `Arc` and the trait are
    /// foreign), so we construct the `AnySubscriber` directly — the same
    /// `(ptr-as-usize, Weak<dyn Subscriber + Send + Sync>)` shape
    /// `reactive_graph` uses internally. The `usize` is the stable allocation
    /// pointer, which the graph's source/subscriber sets key identity on.
    fn any_subscriber(&self) -> AnySubscriber {
        AnySubscriber(
            Arc::as_ptr(&self.inner) as usize,
            Arc::downgrade(&self.inner) as Weak<dyn Subscriber + Send + Sync>,
        )
    }

    /// Runs `f` with this scope installed as the reactive observer.
    ///
    /// Dependency tracking is re-recorded from scratch each call: previously
    /// recorded sources are unsubscribed first, the dirty flag is cleared, then
    /// every signal read inside `f` re-subscribes this scope. Returns whatever
    /// `f` returns.
    pub fn track<R>(&self, f: impl FnOnce() -> R) -> R {
        let any = self.any_subscriber();
        // Re-track from scratch: unsubscribe from the previous pass's sources so
        // a signal no longer read this pass stops notifying us.
        any.clear_sources(&any);
        // Start the pass clean; any write *after* this that we still subscribe
        // to will re-dirty us.
        self.inner.dirty.store(false, Ordering::SeqCst);
        any.with_observer(f)
    }

    /// Whether any tracked source has notified since the last [`track`](Self::track).
    pub fn is_dirty(&self) -> bool {
        self.inner.dirty.load(Ordering::SeqCst)
    }
}

impl Default for TrackedScope {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reactive_graph::computed::Memo;
    use reactive_graph::signal::RwSignal;
    use reactive_graph::traits::{Get, Set};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use crate::FrameWaker;

    /// A recording waker: an `Arc<AtomicUsize>` bumped once per `wake()`.
    fn recording_waker() -> (FrameWaker, Arc<AtomicUsize>) {
        let counter = Arc::new(AtomicUsize::new(0));
        let seen = counter.clone();
        let waker: FrameWaker = Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        (waker, seen)
    }

    /// The whole tracked-scope scenario runs in one `#[test]` under the shared
    /// waker lock: the frame waker is process-global and swappable, so any test
    /// that installs a recording waker and counts wakes must not run concurrently
    /// with another that swaps it (`runtime`'s end-to-end test does). Each block
    /// maps to an acceptance criterion.
    #[test]
    fn tracked_scope_dirty_and_wake_bridge() {
        let _guard = crate::WAKER_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        let (waker, wakes) = recording_waker();
        let rt = ReactiveRuntime::init(waker);

        // Criterion 1: track a signal, then multiple writes → dirty + exactly one
        // coalesced wake before the next track.
        let scope = TrackedScope::new();
        let sig = rt.with_owner(|| RwSignal::new(0));
        scope.track(|| sig.get());
        assert!(!scope.is_dirty(), "fresh track starts clean");
        let before = wakes.load(Ordering::SeqCst);
        sig.set(1);
        sig.set(2);
        sig.set(3);
        assert!(scope.is_dirty(), "a tracked write must dirty the scope");
        assert_eq!(
            wakes.load(Ordering::SeqCst) - before,
            1,
            "N writes between tracks must coalesce to exactly one wake"
        );

        // Criterion 2: a signal NOT read inside track does not dirty the scope.
        let other = rt.with_owner(|| RwSignal::new(0));
        scope.track(|| sig.get()); // re-track: still only `sig`
        assert!(!scope.is_dirty());
        let before = wakes.load(Ordering::SeqCst);
        other.set(99);
        assert!(
            !scope.is_dirty(),
            "an untracked signal must not dirty the scope"
        );
        assert_eq!(
            wakes.load(Ordering::SeqCst),
            before,
            "an untracked write must not wake"
        );

        // Criterion 3: a signal read in run N but not run N+1 no longer dirties
        // (re-track clears the old sources).
        scope.track(|| other.get()); // now tracking `other`, dropped `sig`
        assert!(!scope.is_dirty());
        let before = wakes.load(Ordering::SeqCst);
        sig.set(4); // `sig` was dropped last re-track
        assert!(
            !scope.is_dirty(),
            "a source dropped on re-track must no longer dirty the scope"
        );
        assert_eq!(
            wakes.load(Ordering::SeqCst),
            before,
            "dropped source must not wake"
        );
        // ...but the newly-tracked `other` still dirties.
        other.set(100);
        assert!(
            scope.is_dirty(),
            "the freshly-tracked source must still dirty"
        );

        // Criterion 4: a write from a spawned OS thread dirties the scope and
        // fires the waker.
        let scope2 = TrackedScope::new();
        let cross = rt.with_owner(|| RwSignal::new(0));
        scope2.track(|| cross.get());
        assert!(!scope2.is_dirty());
        let before = wakes.load(Ordering::SeqCst);
        std::thread::spawn(move || {
            cross.set(7);
        })
        .join()
        .expect("cross-thread writer panicked");
        assert!(
            scope2.is_dirty(),
            "a write from another thread must dirty the scope"
        );
        assert_eq!(
            wakes.load(Ordering::SeqCst) - before,
            1,
            "a cross-thread write must fire the waker once"
        );

        // Criterion 5: a memo chain (memo over a signal, memo read inside track,
        // signal written) wakes. The signal write marks the memo dirty, and the
        // memo relays `mark_check` to this scope.
        let scope3 = TrackedScope::new();
        let base = rt.with_owner(|| RwSignal::new(2));
        let doubled = rt.with_owner(|| Memo::new(move |_| base.get() * 2));
        let seen = scope3.track(|| doubled.get());
        assert_eq!(seen, 4, "memo computes from the signal");
        assert!(!scope3.is_dirty());
        let before = wakes.load(Ordering::SeqCst);
        base.set(5);
        assert!(
            scope3.is_dirty(),
            "writing a memo's upstream signal must dirty the tracking scope"
        );
        assert!(
            wakes.load(Ordering::SeqCst) > before,
            "a memo-chain change must fire the waker"
        );

        // Sanity: give any late cross-thread notification a beat (there is none
        // outstanding, but this keeps the test honest about ordering).
        std::thread::sleep(Duration::from_millis(1));
    }
}
