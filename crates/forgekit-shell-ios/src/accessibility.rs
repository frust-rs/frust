//! iOS accesskit adapter wiring (phase 6d D3-ios) — **NOT COMPILED ON THIS
//! HOST**.
//!
//! This module is `#[cfg(target_os = "ios")]`: it depends on `accesskit_ios`,
//! which pulls in `objc2`/`objc2-ui-kit` and can only build for an iOS target.
//! It is written structurally mirroring the Android adapter (a
//! `SubclassingAdapter` held in the app handle, a full `TreeUpdate` pushed
//! post-layout from `frame()`, and an `ActionRequest` → `perform_accessibility_action`
//! routing path) so the phase-6e compile pass on a macOS host is mechanical.
//! The pure, host-testable half — the `SemanticsUpdate` → `accesskit::TreeUpdate`
//! assembly — lives in [`crate::ffi_support::build_tree_update`], covered by
//! `cargo test --workspace`.
//!
//! # Threading model (documented, not fully compile-enforced)
//!
//! Every method here is reached only from the UIKit main thread: the adapter is
//! constructed from `forgekit_init_accessibility` (called on the main thread from
//! `ForgeKitViewController`), and both the semantics push and the action drain run
//! from `IosAppHandle::frame()`, which is driven by the main-thread `CADisplayLink`.
//! accesskit_ios's contract ("all handlers are called on the main thread";
//! `QueuedEvents::raise()` must not be called while holding a lock the view's
//! accessibility methods might need) is honoured *by construction*: the action
//! handler never re-enters the tree — it only enqueues the request onto a plain
//! [`RefCell`]-guarded queue that `frame()` drains outside the adapter — so no
//! borrow of the app tree is ever held across `raise()`. This is the same
//! deadlock-avoidance the research (RESEARCH.md accesskit-adapter-apis, iOS
//! `QueuedEvents::raise` caveat) calls for. accesskit_ios enforces main-thread-ness
//! only via a one-time runtime `MainThreadMarker` panic in the adapter constructor
//! (refuted-claim R4), not compile-time typestate, so this is a documented contract.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::ffi::c_void;
use std::rc::Rc;

use accesskit::{ActionHandler, ActionRequest, ActivationHandler, DeactivationHandler, TreeUpdate};
use accesskit_ios::SubclassingAdapter;
use forgekit_core::SemanticsUpdate;

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
/// Enqueue-don't-execute is deliberate (spec §9 / research iOS `raise()` caveat):
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

/// accesskit `ActivationHandler` that defers the initial tree.
///
/// Returning `None` is valid and intentional: it tells accesskit "I'll send the
/// full tree shortly" — the very next `IosAppHandle::frame()` calls
/// `update_if_active`, which (because `request_initial_tree` returned `None`) is
/// contractually required to, and does, push a **full** tree. ForgeKit's
/// semantics pass always produces a full tree with stable ids (phase-6d D1), so
/// this "defer, then full-push next frame" shape is always correct — and it avoids
/// having the handler reach back into the app tree it cannot borrow (see
/// [`QueueingActionHandler`]).
struct DeferredActivationHandler;

impl ActivationHandler for DeferredActivationHandler {
    fn request_initial_tree(&mut self) -> Option<TreeUpdate> {
        None
    }
}

/// accesskit `DeactivationHandler`: nothing to tear down.
///
/// ForgeKit holds no accessibility-only state that must be reconstructed on
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
/// subclasses the app's `ForgeKitView` to implement the UIKit accessibility
/// methods) plus the shared [`ActionQueue`]. Its `Drop` (via `SubclassingAdapter`)
/// restores the view's original class — that runs when the app handle is dropped
/// in `forgekit_destroy`, on the main thread, before Swift releases the view.
pub(crate) struct IosA11yAdapter {
    adapter: SubclassingAdapter,
    queue: ActionQueue,
}

impl IosA11yAdapter {
    /// Create the adapter for the app's `ForgeKitView`.
    ///
    /// # Safety
    ///
    /// `view` must be a valid, unreleased pointer to the `UIView` (the app's
    /// `ForgeKitView`), and this must be called on the UIKit main thread and
    /// **before** the view is first shown or focused (accesskit_ios
    /// `SubclassingAdapter::new` contract). The generated `ForgeKitViewController`
    /// satisfies this: it calls `forgekit_init_accessibility(handle, self.view)`
    /// on the first layout, right after `forgekit_init` succeeds.
    pub(crate) unsafe fn new(view: *mut c_void) -> Self {
        let queue: ActionQueue = Rc::new(RefCell::new(VecDeque::new()));
        // SAFETY: forwarded from this fn's contract — `view` is a live `UIView*`
        // and we are on the main thread. The action handler holds only a clone of
        // the plain `Rc` queue (no tree borrow), satisfying the `'static` bound
        // accesskit_ios requires without a `Send`/lock dance.
        let adapter = unsafe {
            SubclassingAdapter::new(
                view,
                DeferredActivationHandler,
                QueueingActionHandler {
                    queue: queue.clone(),
                },
                NoopDeactivationHandler,
            )
        };
        Self { adapter, queue }
    }

    /// Push a full semantics tree **iff** an assistive technology is active.
    ///
    /// `factory` is only invoked when the adapter is active (VoiceOver / Switch
    /// Control / Speak Screen running), so the semantics tree walk — and thus the
    /// per-frame cost — is paid only while an AT actually needs it; when nothing is
    /// listening this is a cheap early return inside `update_if_active`. The
    /// resulting `QueuedEvents` is raised immediately (we are on the main thread and
    /// hold no tree borrow — see the module docs).
    ///
    /// v1 pushes the whole tree every active frame (accesskit dedupes unchanged
    /// nodes internally, so this does not spam AT events — PLAN.md's
    /// "full-tree-every-update acceptable v1"); generation-gated skipping (the
    /// `semantics_if_changed` seam) is a later optimization.
    pub(crate) fn push_if_active(&self, factory: impl FnOnce() -> SemanticsUpdate) {
        let events = self.adapter.update_if_active(|| {
            let update = factory();
            let focus = update.focus_id();
            crate::ffi_support::build_tree_update(update.nodes, update.root, focus)
        });
        if let Some(events) = events {
            events.raise();
        }
    }

    /// Drain all queued action requests in FIFO order, transferring ownership to
    /// the caller so the queue's `RefCell` borrow is released before the caller
    /// mutates the app tree (which `perform_accessibility_action` does).
    pub(crate) fn drain_actions(&self) -> Vec<ActionRequest> {
        self.queue.borrow_mut().drain(..).collect()
    }
}
