//! iOS accesskit adapter wiring — **NOT COMPILED ON THIS
//! HOST**.
//!
//! This module is `#[cfg(target_os = "ios")]`: it depends on `accesskit_ios`,
//! which pulls in `objc2`/`objc2-ui-kit` and can only build for an iOS target.
//! It is written structurally mirroring the Android adapter (a
//! `SubclassingAdapter` held in the app handle, a full `TreeUpdate` pushed
//! post-layout from `frame()`, and an `ActionRequest` → `perform_accessibility_action`
//! routing path) so a compile pass on a macOS host is mechanical.
//! The pure, host-testable half — the `SemanticsUpdate` → `accesskit::TreeUpdate`
//! assembly — lives in [`crate::ffi_support::build_tree_update`], covered by
//! `cargo test --workspace`.
//!
//! # Threading model (documented, not fully compile-enforced)
//!
//! Every method here is reached only from the UIKit main thread: the adapter is
//! constructed from `frust_init_accessibility` (called on the main thread from
//! `FrustViewController`), and both the semantics push and the action drain run
//! from `IosAppHandle::frame()`, which is driven by the main-thread `CADisplayLink`.
//! accesskit_ios's contract ("all handlers are called on the main thread";
//! `QueuedEvents::raise()` must not be called while holding a lock the view's
//! accessibility methods might need) is honoured *by construction*: the action
//! handler never re-enters the tree — it only enqueues the request onto a plain
//! [`RefCell`]-guarded queue that `frame()` drains outside the adapter — so no
//! borrow of the app tree is ever held across `raise()`. This is the same
//! deadlock-avoidance accesskit_ios's `QueuedEvents::raise` caveat calls for.
//! accesskit_ios enforces main-thread-ness
//! only via a one-time runtime `MainThreadMarker` panic in the adapter constructor,
//! not compile-time typestate, so this is a documented contract.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::ffi::c_void;
use std::rc::Rc;

use accesskit::{ActionHandler, ActionRequest, ActivationHandler, DeactivationHandler, TreeUpdate};
use accesskit_ios::SubclassingAdapter;
use frust_core::SemanticsUpdate;

/// A FIFO of accesskit action requests handed to us by an assistive technology.
///
/// Shared (`Rc`) between the adapter-owned [`QueueingActionHandler`] (the
/// producer) and the app handle's per-frame drain (the consumer). A plain
/// `Rc<RefCell<..>>` is sufficient — never a `Mutex`/`Arc` — because both ends run
/// on the UIKit main thread (see the module docs' threading model); accesskit_ios
/// requires the handler be `'static` but, unlike Android's adapter, imposes no
/// `Send` bound.
type ActionQueue = Rc<RefCell<VecDeque<ActionRequest>>>;

/// accesskit `ActionHandler` that never touches the widget tree directly: it just
/// enqueues each request for `IosAppHandle::frame()` to drain.
///
/// Enqueue-don't-execute is deliberate:
/// the handler is owned by the adapter, which is owned by the app handle, so it
/// *cannot* borrow the handle to run the action here without aliasing — and even
/// if it could, running the action (which mutates state) from inside the adapter
/// would violate the "no tree borrow held across `raise()`" contract. Draining on
/// the next frame both breaks the ownership cycle and keeps the reentrancy model
/// identical to touch/IME input (mutate state now, rebuild on the next frame).
struct QueueingActionHandler {
    queue: ActionQueue,
}

impl ActionHandler for QueueingActionHandler {
    fn do_action(&mut self, request: ActionRequest) {
        // Runs on the main thread (accesskit_ios contract). Enqueue only.
        self.queue.borrow_mut().push_back(request);
    }
}

/// accesskit `ActivationHandler` that serves the last full tree the frame loop
/// published.
///
/// Returns a clone of the [`IosA11yAdapter`]'s `tree_snapshot` — the tree
/// `IosAppHandle::publish_semantics` stores on every push — so an assistive
/// technology that activates on a *static* screen (nothing semantics-relevant
/// changed since the last push) still receives real content immediately rather
/// than the adapter's placeholder window. `None` only before the first
/// post-layout semantics pass, in which case the very next `update_if_active`
/// (which the deferred-`None` contract requires to push a **full** tree) fills
/// it in.
///
/// This snapshot is what lets `publish_semantics`'s generation gate safely
/// *skip* re-pushing an unchanged tree without starving a late-activating AT —
/// the same design the Android adapter's `ForgeActivationHandler`/`tree_snapshot`
/// uses. It reaches only the shared snapshot slot, never the app
/// tree it cannot borrow (see [`QueueingActionHandler`]). Runs on the UIKit main
/// thread.
struct SnapshotActivationHandler {
    tree_snapshot: Rc<RefCell<Option<TreeUpdate>>>,
}

impl ActivationHandler for SnapshotActivationHandler {
    fn request_initial_tree(&mut self) -> Option<TreeUpdate> {
        self.tree_snapshot.borrow().clone()
    }
}

/// accesskit `DeactivationHandler`: nothing to tear down.
///
/// Frust holds no accessibility-only state that must be reconstructed on
/// reactivation — the tree is pulled fresh from the retained widget tree via the
/// semantics seam on the next `update_if_active`. So deactivation is a no-op; a
/// later `request_initial_tree`/`update_if_active` transparently re-pushes.
struct NoopDeactivationHandler;

impl DeactivationHandler for NoopDeactivationHandler {
    fn deactivate_accessibility(&mut self) {}
}

/// The iOS accessibility adapter held by [`crate::app::IosAppHandle`].
///
/// Wraps `accesskit_ios::SubclassingAdapter` (which dynamically Objective-C
/// subclasses the app's `FrustView` to implement the UIKit accessibility
/// methods) plus the shared [`ActionQueue`]. Its `Drop` (via `SubclassingAdapter`)
/// restores the view's original class — that runs when the app handle is dropped
/// in `frust_destroy`, on the main thread, before Swift releases the view.
pub(crate) struct IosA11yAdapter {
    adapter: SubclassingAdapter,
    queue: ActionQueue,
    /// The latest full tree the frame loop published, served to a
    /// late-activating AT through [`SnapshotActivationHandler`]. Shared (`Rc`)
    /// with that handler; written on every [`Self::publish`]. `None` until the
    /// first push. Mirrors the Android adapter's `tree_snapshot`.
    tree_snapshot: Rc<RefCell<Option<TreeUpdate>>>,
    /// The semantics generation last assembled and pushed, so
    /// `IosAppHandle::publish_semantics` skips reassembling+re-pushing an
    /// unchanged tree (the `semantics_generation`/`semantics_if_changed` dirty
    /// gate — see `docs/CODE_STANDARDS.md`'s Semantics Conventions). Mirrors the
    /// Android adapter's `last_pushed_gen`.
    last_pushed_gen: u64,
}

impl IosA11yAdapter {
    /// Create the adapter for the app's `FrustView`.
    ///
    /// # Safety
    ///
    /// `view` must be a valid, unreleased pointer to the `UIView` (the app's
    /// `FrustView`), and this must be called on the UIKit main thread and
    /// **before** the view is first shown or focused (accesskit_ios
    /// `SubclassingAdapter::new` contract). The generated `FrustViewController`
    /// satisfies this: it calls `frust_init_accessibility(handle, self.view)`
    /// on the first layout, right after `frust_init` succeeds.
    pub(crate) unsafe fn new(view: *mut c_void) -> Self {
        let queue: ActionQueue = Rc::new(RefCell::new(VecDeque::new()));
        let tree_snapshot: Rc<RefCell<Option<TreeUpdate>>> = Rc::new(RefCell::new(None));
        // SAFETY: forwarded from this fn's contract — `view` is a live `UIView*`
        // and we are on the main thread. Both handlers hold only plain `Rc`
        // clones (the action queue / the tree snapshot slot — no app-tree
        // borrow), satisfying the `'static` bound accesskit_ios requires without
        // a `Send`/lock dance.
        let adapter = unsafe {
            SubclassingAdapter::new(
                view,
                SnapshotActivationHandler {
                    tree_snapshot: tree_snapshot.clone(),
                },
                QueueingActionHandler {
                    queue: queue.clone(),
                },
                NoopDeactivationHandler,
            )
        };
        Self {
            adapter,
            queue,
            tree_snapshot,
            last_pushed_gen: 0,
        }
    }

    /// The semantics generation last pushed (see [`Self::last_pushed_gen`] field
    /// docs) — read by `IosAppHandle::publish_semantics` before deciding whether
    /// the tree changed enough to reassemble and re-push.
    pub(crate) fn last_pushed_gen(&self) -> u64 {
        self.last_pushed_gen
    }

    /// Publish one already-generation-gated semantics update to the adapter.
    ///
    /// The caller (`IosAppHandle::publish_semantics`) has already established via
    /// [`AppTree::semantics_if_changed`](frust_shell_common::AppTree::semantics_if_changed)
    /// that the tree changed since [`Self::last_pushed_gen`], so this always does
    /// real work: it assembles the full `accesskit::TreeUpdate`, snapshots it for
    /// a late-activating AT's `request_initial_tree`
    /// ([`SnapshotActivationHandler`]), and hands it to `update_if_active` — which
    /// pushes only while an assistive technology is active (a cheap no-op
    /// otherwise) and returns the `QueuedEvents` to raise. Full-tree every push is
    /// valid because our node ids are stable across frames and
    /// accesskit dedupes unchanged nodes internally.
    ///
    /// `generation` is the tree's current
    /// [`semantics_generation`](frust_shell_common::AppTree::semantics_generation),
    /// recorded so the next `publish_semantics` can gate again. The
    /// `QueuedEvents` is raised immediately: we are on the main thread and hold
    /// no app-tree borrow (see the module docs' threading model).
    pub(crate) fn publish(&mut self, update: SemanticsUpdate, generation: u64) {
        let focus = update.focus_id();
        let tree_update = crate::ffi_support::build_tree_update(update.nodes, update.root, focus);
        // Snapshot the full tree so a late-activating AT (one that connects on a
        // static screen the generation gate would otherwise skip) is served real
        // content immediately. Cloned once here, not per idle frame.
        *self.tree_snapshot.borrow_mut() = Some(tree_update.clone());
        if let Some(events) = self.adapter.update_if_active(move || tree_update) {
            events.raise();
        }
        self.last_pushed_gen = generation;
    }

    /// Drain all queued action requests in FIFO order, transferring ownership to
    /// the caller so the queue's `RefCell` borrow is released before the caller
    /// mutates the app tree (which `perform_accessibility_action` does).
    pub(crate) fn drain_actions(&self) -> Vec<ActionRequest> {
        self.queue.borrow_mut().drain(..).collect()
    }
}
