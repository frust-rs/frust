//! The retained-handle registry: tracks the one native view
//! handle a `platform_view` slot currently owns, keyed by
//! [`SlotId`], so a dispose reaching this crate can be resolved against
//! the exact handle it targets.
//!
//! ## Idle-deferred dispose
//!
//! The differ's `Dispose` command for a slot can arrive many frames after
//! that slot's view was created — or even after a *replacement* `Create`
//! already landed for the same (re-used) slot id, since Android's real
//! dispose export (`nativeDisposeControl(view)`) hands back the raw view
//! object, not a slot id. A
//! disposal must therefore be resolved **by identity** — the specific
//! handle a Dispose command actually names via [`Registry::remove_matching`]
//! — never by assuming "whichever handle currently occupies the slot" is
//! the one being torn down; the latter would delete a *live, already-
//! replaced-in* control out from under the app. [`Registry::remove`] (by
//! `slot_id` alone) is the simpler counterpart for a caller that already
//! knows the exact slot — a future widget-teardown path, not the raw
//! dispose export.
//!
//! ## Design: generic core, cfg-gated handle types
//!
//! [`Registry`] itself carries no `#[cfg]` — its insert/remove/live-count
//! contract is exercised on any host by this module's own unit tests
//! (below), using a plain counting fixture, independent of JNI/ObjC. The
//! real per-platform handle types live in the [`android`]/[`apple`]/
//! [`appkit`] submodules, each compiled only under its own target:
//!
//! - [`android::AndroidHandle`] is the RAII owner of every
//!   `Global<JObject<'static>>` a control's create step allocated — the
//!   view itself, plus any secondary refs (a listener object, a child
//!   hierarchy) it retains. Removing (or replacing, or dropping the whole
//!   registry) a [`Registry`] entry pair-deletes them all together — the
//!   paired-delete discipline this design requires: ART aborts the
//!   process at 51,200 live global refs process-wide, so a leaked ref here
//!   is a crash, not a slow leak.
//! - [`apple::AppleHandle`] is the ARC-managed `Retained<UIView>`
//!   counterpart — memory-safe by construction (no paired-delete
//!   discipline needed; `Retained`'s own `Drop` releases the object) — kept
//!   behind the same slot-keyed shape so a surface-recreate replay's
//!   replace-and-release-old contract behaves identically on every platform.
//! - [`appkit::AppKitHandle`] is the macOS twin of [`apple::AppleHandle`]:
//!   the same ARC-managed shape over a `Retained<NSView>` (AppKit instead of
//!   UIKit), behind the same main-thread typestate guard.

use std::collections::HashMap;

/// A `platform_view` slot id (`frust_shell_common::platform_view`'s
/// differ-assigned identity, stamped from `next_slot_id()`'s process-wide
/// counter — `docs/ARCHITECTURE.md`'s `frust-core` row) — the registry's
/// key.
pub type SlotId = u64;

/// A retained-handle registry keyed by [`SlotId`], generic over the
/// concrete platform handle type `H` so its core contract is host-testable
/// with no FFI dependency (see the module doc, and this module's `tests`).
pub struct Registry<H> {
    live: HashMap<SlotId, H>,
}

impl<H> Registry<H> {
    /// An empty registry.
    pub fn new() -> Self {
        Self {
            live: HashMap::new(),
        }
    }

    /// Record a freshly created `handle` for `slot_id`, **replacing**
    /// (never panicking on) any handle already live under the same id —
    /// the surface-recreate-replay / rapid-recreate race this registry
    /// must handle. Returns the replaced handle, if any,
    /// so a caller can log the unexpected-replace path; dropping it
    /// releases its resources exactly like an explicit [`Self::remove`]
    /// would (the registry "treats attach-for-live-slot-id as replace-
    /// and-release-old").
    pub fn insert(&mut self, slot_id: SlotId, handle: H) -> Option<H> {
        self.live.insert(slot_id, handle)
    }

    /// Release the handle for `slot_id` directly, by the slot id itself —
    /// for a caller that already knows exactly which slot it means (a
    /// future widget-teardown path), as opposed
    /// to [`Self::remove_matching`]'s identity-based lookup for the real
    /// platform dispose export, which never receives a slot id at all. A
    /// dispose for a slot id that was already replaced or already removed
    /// is a silent no-op (late/duplicate dispose, both expected once
    /// dispose can arrive idle-deferred — see the module doc),
    /// never an error.
    pub fn remove(&mut self, slot_id: SlotId) -> Option<H> {
        self.live.remove(&slot_id)
    }

    /// Find and remove the entry whose handle satisfies `is_match`, **by
    /// identity** — the real disposal contract every platform backend
    /// uses (Android: `Env::is_same_object` against the `JObject` the
    /// `nativeDisposeControl(view)` export receives, which carries no slot
    /// id at all). Never assumes "whichever
    /// slot was created most recently" is the one being disposed: a match
    /// against a handle that was already replaced (the module doc's
    /// idle-deferred-dispose case) or already removed finds nothing and is
    /// a silent no-op, leaving whatever handle currently occupies that
    /// slot (if any) untouched.
    pub fn remove_matching(&mut self, mut is_match: impl FnMut(&H) -> bool) -> Option<(SlotId, H)> {
        let slot_id = self
            .live
            .iter()
            .find(|(_, handle)| is_match(handle))
            .map(|(slot_id, _)| *slot_id)?;
        self.live.remove(&slot_id).map(|handle| (slot_id, handle))
    }

    /// Whether `slot_id` currently has a live handle.
    pub fn contains(&self, slot_id: SlotId) -> bool {
        self.live.contains_key(&slot_id)
    }

    /// The handle currently live under `slot_id`, if any — what a caller
    /// holding a slot id (an `UpdateParams` command, a listener callback
    /// carrying its own slot id) resolves against, as opposed to
    /// [`Self::remove_matching`]'s identity lookup for a dispose that carries
    /// none.
    pub fn get(&self, slot_id: SlotId) -> Option<&H> {
        self.live.get(&slot_id)
    }

    /// [`Self::get`]'s mutable counterpart, for a caller that mutates the
    /// handle in place (the runtime's per-instance props/state).
    pub fn get_mut(&mut self, slot_id: SlotId) -> Option<&mut H> {
        self.live.get_mut(&slot_id)
    }

    /// The number of live handles — the leak bar every create/dispose
    /// cycle must return to `0`.
    pub fn live_count(&self) -> usize {
        self.live.len()
    }
}

impl<H> Default for Registry<H> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(target_os = "android")]
pub mod android {
    //! Android's [`Registry`](super::Registry) handle: a RAII owner of
    //! every `Global<JObject<'static>>` one control retains.

    use jni::objects::JObject;
    use jni::refs::Global;

    /// Every global ref a single control's `create` step allocated: the
    /// view itself, plus any secondary refs it retains (a listener object,
    /// a stress-style child hierarchy — an earlier prototype `Registry` had
    /// the same shape, one `HashMap` per kind of ref). Dropping (or removing
    /// from a [`Registry`](super::Registry)) this pairs-deletes all of
    /// them together — the paired-delete discipline this design
    /// requires; ART aborts the process at 51,200 live global refs
    /// process-wide, so a leaked ref here is a crash, not a slow leak.
    pub struct AndroidHandle {
        /// The control's own retained view.
        pub view: Global<JObject<'static>>,
        /// Any other refs the control's `create` step retained (a
        /// listener, child views) — dropped alongside `view`.
        pub extra: Vec<Global<JObject<'static>>>,
    }

    impl AndroidHandle {
        /// A handle retaining only the view itself.
        pub fn new(view: Global<JObject<'static>>) -> Self {
            Self {
                view,
                extra: Vec::new(),
            }
        }

        /// A handle retaining the view plus every ref in `extra`.
        pub fn with_extra(
            view: Global<JObject<'static>>,
            extra: Vec<Global<JObject<'static>>>,
        ) -> Self {
            Self { view, extra }
        }
    }
}

// iOS only — not `target_vendor = "apple"`: this plugin's iOS surface is
// UIKit, which doesn't exist on macOS (the macOS arm is AppKit — `appkit`
// below) — see `Cargo.toml`'s comment on the same gate for the linker
// failure `target_vendor = "apple"` caused.
#[cfg(target_os = "ios")]
pub mod apple {
    //! iOS's [`Registry`](super::Registry) handle: an ARC-managed
    //! `Retained<UIView>`.

    use objc2::MainThreadMarker;
    use objc2::rc::Retained;
    use objc2_ui_kit::UIView;

    /// A retained control view. Memory-safe by construction — no
    /// paired-delete discipline needed, `Retained`'s own `Drop` releases
    /// the object — but UIKit types are main-thread-only, so
    /// [`AppleHandle::view`] only hands the `Retained<UIView>` back
    /// alongside proof of a live [`MainThreadMarker`], never off it (every
    /// platform-view create/update/dispose command is drained on the main
    /// thread already, so this is a documentation-and-typestate guard, not
    /// a new runtime cost).
    pub struct AppleHandle {
        view: Retained<UIView>,
    }

    impl AppleHandle {
        /// Retain `view`, requiring proof the caller is on the main
        /// thread.
        pub fn new(view: Retained<UIView>, _mtm: MainThreadMarker) -> Self {
            Self { view }
        }

        /// The retained view, requiring the same main-thread proof.
        pub fn view(&self, _mtm: MainThreadMarker) -> &Retained<UIView> {
            &self.view
        }
    }
}

// macOS only — the AppKit arm's handle, mirroring `apple` above exactly with
// `NSView` in place of `UIView` (the two arms share no UI framework, only the
// ARC ownership model).
#[cfg(target_os = "macos")]
pub mod appkit {
    //! macOS's [`Registry`](super::Registry) handle: an ARC-managed
    //! `Retained<NSView>`.

    use objc2::MainThreadMarker;
    use objc2::rc::Retained;
    use objc2_app_kit::NSView;

    /// A retained control view. Memory-safe by construction — no
    /// paired-delete discipline needed, `Retained`'s own `Drop` releases
    /// the object — but AppKit views are main-thread-only, so
    /// [`AppKitHandle::view`] only hands the `Retained<NSView>` back
    /// alongside proof of a live [`MainThreadMarker`], never off it (every
    /// desktop platform-view create/update/dispose call is made on the main
    /// thread already — `frust_plugin::desktop`'s contract — so this is a
    /// documentation-and-typestate guard, not a new runtime cost).
    pub struct AppKitHandle {
        view: Retained<NSView>,
    }

    impl AppKitHandle {
        /// Retain `view`, requiring proof the caller is on the main
        /// thread.
        pub fn new(view: Retained<NSView>, _mtm: MainThreadMarker) -> Self {
            Self { view }
        }

        /// The retained view, requiring the same main-thread proof.
        pub fn view(&self, _mtm: MainThreadMarker) -> &Retained<NSView> {
            &self.view
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// A host-testable stand-in for a platform handle: increments a shared
    /// counter on construction, decrements on drop — the leak bar's
    /// substitute for ART's global-ref count / ARC's retain count. Carries
    /// an `identity` so tests can simulate the real Android dispose
    /// export's identity-based lookup (`remove_matching`) rather than a
    /// slot-id-keyed one.
    struct CountingHandle {
        identity: u32,
        count: Rc<RefCell<i32>>,
    }

    impl CountingHandle {
        fn new(identity: u32, count: &Rc<RefCell<i32>>) -> Self {
            *count.borrow_mut() += 1;
            Self {
                identity,
                count: Rc::clone(count),
            }
        }
    }

    impl Drop for CountingHandle {
        fn drop(&mut self) {
            *self.count.borrow_mut() -= 1;
        }
    }

    #[test]
    fn create_dispose_cycles_return_to_zero_live_refs() {
        let count = Rc::new(RefCell::new(0));
        let mut registry: Registry<CountingHandle> = Registry::new();

        for slot in 0..100u64 {
            registry.insert(slot, CountingHandle::new(slot as u32, &count));
            assert_eq!(registry.live_count(), 1);
            let removed = registry.remove(slot);
            assert!(removed.is_some(), "dispose finds the handle it created");
            drop(removed);
            assert_eq!(registry.live_count(), 0);
        }

        assert_eq!(
            *count.borrow(),
            0,
            "every one of the 100 creates was paired with a dispose"
        );
    }

    #[test]
    fn insert_replaces_and_releases_the_old_handle_for_a_reused_slot() {
        // The surface-recreate-replay / rapid-recreate race this registry
        // must handle: a fresh Create for an
        // already-live slot id must replace in place, not panic or leak.
        let count = Rc::new(RefCell::new(0));
        let mut registry: Registry<CountingHandle> = Registry::new();
        let slot = 7u64;

        registry.insert(slot, CountingHandle::new(100, &count));
        assert_eq!(*count.borrow(), 1);

        let replaced = registry.insert(slot, CountingHandle::new(200, &count));
        assert!(replaced.is_some());
        drop(replaced);
        assert_eq!(
            *count.borrow(),
            1,
            "replacing drops the old handle immediately, leaving only the new one"
        );
        assert_eq!(registry.live_count(), 1);
    }

    /// The idle-deferred-dispose case (see the module doc):
    /// a slot's original handle is replaced by a fresh Create
    /// before that handle's own late Dispose is ever drained. The late
    /// Dispose carries the OLD handle's identity — `remove_matching` must
    /// find nothing (the old handle is already gone) and must NOT touch
    /// the NEW handle now occupying the slot.
    #[test]
    fn a_late_dispose_for_a_replaced_handle_is_a_no_op_via_identity_match() {
        let count = Rc::new(RefCell::new(0));
        let mut registry: Registry<CountingHandle> = Registry::new();
        let slot = 42u64;

        const OLD_IDENTITY: u32 = 1;
        const NEW_IDENTITY: u32 = 2;

        registry.insert(slot, CountingHandle::new(OLD_IDENTITY, &count));

        // A fresh Create replaces the slot's handle (surface-recreate
        // replay, or a rapid recreate race) before the old handle's
        // Dispose is ever drained — dropping the old handle immediately.
        let replaced = registry.insert(slot, CountingHandle::new(NEW_IDENTITY, &count));
        drop(replaced);
        assert_eq!(*count.borrow(), 1);

        // The late Dispose finally arrives, naming the OLD (already-gone)
        // handle's identity.
        let found = registry.remove_matching(|h| h.identity == OLD_IDENTITY);
        assert!(
            found.is_none(),
            "a late dispose for an already-replaced handle is a no-op"
        );
        assert_eq!(*count.borrow(), 1, "the current (new) handle is untouched");
        assert!(registry.contains(slot));

        // The eventual, correctly-targeted dispose for the CURRENT handle
        // still works.
        let found = registry.remove_matching(|h| h.identity == NEW_IDENTITY);
        assert!(found.is_some());
        drop(found); // the removed handle is only released once dropped
        assert_eq!(*count.borrow(), 0);
        assert!(!registry.contains(slot));
    }

    #[test]
    fn duplicate_dispose_is_a_silent_no_op() {
        let count = Rc::new(RefCell::new(0));
        let mut registry: Registry<CountingHandle> = Registry::new();
        let slot = 3u64;

        registry.insert(slot, CountingHandle::new(9, &count));
        assert!(registry.remove(slot).is_some());
        assert_eq!(*count.borrow(), 0);

        // A second dispose for the same (now-gone) slot id must not panic
        // or double-decrement — just report nothing to release.
        assert!(registry.remove(slot).is_none());
        assert_eq!(*count.borrow(), 0);
    }

    #[test]
    fn dispose_for_an_unknown_slot_is_a_silent_no_op() {
        let mut registry: Registry<CountingHandle> = Registry::new();
        assert!(registry.remove(999).is_none());
        assert_eq!(registry.live_count(), 0);
    }

    #[test]
    fn remove_matching_against_an_empty_registry_finds_nothing() {
        let mut registry: Registry<CountingHandle> = Registry::new();
        assert!(registry.remove_matching(|_| true).is_none());
    }
}
