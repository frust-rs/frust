//! Desktop platform-view factory registry — the substrate's **desktop-side**
//! meeting point, mirroring [`crate::android`]'s Android-side one but with the
//! write/read direction reversed: on Android the **shell** writes and
//! **plugins** read; here **plugins** write (register a factory for a
//! `view_type` string, e.g. `"dev.frust.VideoPlayer"`) and the **desktop
//! shell** reads (looks a factory up by the `view_type` string carried on
//! `ViewCommand::Create` — see `frust-shell-common::platform_view`).
//!
//! # Why a process-global registry, not a passed-in map
//!
//! A plugin crate has no reference to the shell's differ or window state, and
//! the shell has no reference to a plugin's factory type — the two meet only
//! through the `view_type` string embedded in a `ViewCommand`. A process-wide
//! table keyed on that string is the only meeting point that needs no new
//! dependency edge in either direction: plugins keep depending only on this
//! leaf crate (never on a shell crate), and a desktop shell crate depends on
//! this leaf crate to resolve `view_type -> factory` without depending on any
//! specific plugin.
//!
//! # Std-only, every target
//!
//! This module has no `cfg` gate: it is plain `std::sync` and `std::ffi`, so
//! it compiles (and its tests run) on every target this crate supports,
//! Android and iOS included, even though only a desktop shell (today: macOS)
//! ever calls [`lookup_view_factory`]. That keeps the crate's "leaf, one
//! substrate, every target" charter (see the crate docs) true of the desktop
//! half exactly as it already is of [`crate::android`].
//!
//! # Main-thread-only contract
//!
//! [`DesktopViewFactory`]'s three methods are called **only** on the platform
//! main thread — winit's event-loop thread, the thread a desktop shell's
//! native view APIs (`NSView` et al.) require. A factory implementation must
//! not block that thread and must not panic across the call: the shell wraps
//! each call in `catch_unwind` per `docs/CODE_STANDARDS.md`'s "no panics at
//! FFI" rule, but a caught panic still means the factory's own state may be
//! left inconsistent, so this is a correctness contract, not just a safety
//! net. [`DesktopViewHandle`] carries a `!Send` marker for the same reason:
//! the pointer it wraps is only ever valid to touch from that thread.

use std::collections::HashMap;
use std::ffi::c_void;
use std::marker::PhantomData;
use std::ptr::NonNull;
use std::sync::{Arc, OnceLock, RwLock};

/// A +1-retained, platform-owned native view pointer — `NSView *` on macOS;
/// the desktop shell of that OS defines the concrete type. Opaque here: this
/// crate never dereferences it, it only carries it between a plugin's
/// [`DesktopViewFactory`] and the desktop shell that owns the window.
///
/// `!Send` (via the `PhantomData<*mut ()>` marker field): the wrapped pointer
/// is only ever valid to create, update or dispose on the platform main
/// thread (see the module docs' main-thread contract), so a handle must never
/// cross to another thread.
///
/// Deliberately has **no `Drop` impl** — ownership transfers explicitly.
/// A handle a factory creates is either handed to the shell (which is
/// responsible for eventually passing it back to
/// [`DesktopViewFactory::dispose`]) or consumed by `dispose` itself; there is
/// no implicit release point that a `Drop` could be trusted to run at the
/// right time (or on the right thread).
pub struct DesktopViewHandle(NonNull<c_void>, PhantomData<*mut ()>);

impl DesktopViewHandle {
    /// Wrap a raw native view pointer.
    ///
    /// # Safety
    ///
    /// `ptr` must be a valid, +1-retained platform view pointer (e.g. an
    /// `NSView *` the caller has already `retain`ed) that stays valid until
    /// this handle is consumed by [`DesktopViewFactory::dispose`]. The caller
    /// must only ever touch the pointee from the platform main thread.
    pub unsafe fn from_raw(ptr: NonNull<c_void>) -> Self {
        Self(ptr, PhantomData)
    }

    /// The wrapped pointer, without consuming the handle. Never dereference
    /// this off the platform main thread (see the module docs' main-thread
    /// contract).
    pub fn as_ptr(&self) -> *mut c_void {
        self.0.as_ptr()
    }

    /// Consume the handle and hand back the raw pointer — the explicit
    /// ownership-transfer point in place of a `Drop` impl.
    pub fn into_raw(self) -> NonNull<c_void> {
        self.0
    }
}

/// A plugin's desktop native-view factory: creates, re-parameterizes and
/// disposes the native view backing one `view_type`.
///
/// All three methods are called **only** on the platform main thread (see the
/// module docs); an implementation must not block that thread and must not
/// panic across the call — the shell wraps each call in `catch_unwind` per
/// `docs/CODE_STANDARDS.md`, but a factory should still treat a panic as a
/// bug in itself, not a supported control-flow path.
///
/// `Send + Sync` because the registry stores the factory behind an `Arc` a
/// desktop shell resolves from whichever thread reads the registry (lookup
/// itself is not main-thread-restricted — only the *calls into* the factory
/// are); the factory value must therefore be safe to share, even though every
/// method call on it happens on the main thread.
pub trait DesktopViewFactory: Send + Sync + 'static {
    /// Create a new native view from the slot's creation params
    /// (`ViewCommand::Create`'s `params_json`, opaque JSON this factory
    /// defines the shape of). `None` means a dead slot: the shell logs it
    /// once and leaves the slot empty rather than retrying.
    fn create(&self, params_json: &str) -> Option<DesktopViewHandle>;

    /// Apply new creation params to an already-created view in place
    /// (`ViewCommand::UpdateParams`), without recreating it.
    fn update_params(&self, view: &DesktopViewHandle, params_json: &str);

    /// Tear the native view down (`ViewCommand::Dispose`), consuming the
    /// handle — the explicit ownership-transfer point [`DesktopViewHandle`]
    /// has no `Drop` for.
    fn dispose(&self, view: DesktopViewHandle);
}

/// Failure to register a desktop view factory.
#[derive(thiserror::Error, Debug, PartialEq)]
pub enum RegisterError {
    /// A factory is already registered for this `view_type` — first
    /// registration wins, this crate never silently replaces one.
    #[error("a desktop view factory is already registered for view_type {0:?}")]
    AlreadyRegistered(String),
}

/// The process-global `view_type -> factory` table. `std::sync::RwLock` (not
/// a framework primitive — this crate has no `frust-*` deps) behind a
/// `OnceLock` so it initializes lazily on first use with no explicit
/// `static ... = RwLock::new(...)` const-eval requirement on `HashMap::new`.
static REGISTRY: OnceLock<RwLock<HashMap<&'static str, Arc<dyn DesktopViewFactory>>>> =
    OnceLock::new();

fn registry() -> &'static RwLock<HashMap<&'static str, Arc<dyn DesktopViewFactory>>> {
    REGISTRY.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Register `factory` as the desktop view factory for `view_type`.
///
/// First registration for a given `view_type` wins: a second call for the
/// same string returns [`RegisterError::AlreadyRegistered`] rather than
/// replacing the existing factory silently. `view_type` is process-global
/// state (like [`crate::android`]'s handle slot), so plugin authors should
/// use a collision-resistant name (e.g. `"dev.frust.VideoPlayer"`).
///
/// # Panics
///
/// Panics only if the internal lock is poisoned (a prior panic while the
/// lock was held elsewhere) — the same non-recoverable condition any
/// `std::sync` user propagates; it is not a normal control-flow path for a
/// well-behaved factory (see the trait's no-panic contract).
pub fn register_view_factory(
    view_type: &'static str,
    factory: Arc<dyn DesktopViewFactory>,
) -> Result<(), RegisterError> {
    let mut table = registry().write().expect("desktop view registry poisoned");
    if table.contains_key(view_type) {
        return Err(RegisterError::AlreadyRegistered(view_type.to_string()));
    }
    table.insert(view_type, factory);
    Ok(())
}

/// Look up the desktop view factory registered for `view_type`, or `None` if
/// nothing has registered that string yet.
///
/// # Panics
///
/// Panics only if the internal lock is poisoned (see
/// [`register_view_factory`]'s panic note).
pub fn lookup_view_factory(view_type: &str) -> Option<Arc<dyn DesktopViewFactory>> {
    let table = registry().read().expect("desktop view registry poisoned");
    table.get(view_type).cloned()
}

/// Every `view_type` currently registered — diagnostics only (e.g. logging
/// what a shell can resolve), not meant for hot-path lookups.
///
/// # Panics
///
/// Panics only if the internal lock is poisoned (see
/// [`register_view_factory`]'s panic note).
pub fn registered_view_types() -> Vec<&'static str> {
    let table = registry().read().expect("desktop view registry poisoned");
    table.keys().copied().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A trivial factory that hands back a fixed sentinel pointer and counts
    /// calls, so tests can assert both round-trip behavior and that the
    /// registry actually dispatched to *this* factory.
    struct FakeFactory {
        create_calls: AtomicUsize,
    }

    impl FakeFactory {
        fn new() -> Self {
            Self {
                create_calls: AtomicUsize::new(0),
            }
        }
    }

    impl DesktopViewFactory for FakeFactory {
        fn create(&self, _params_json: &str) -> Option<DesktopViewHandle> {
            self.create_calls.fetch_add(1, Ordering::SeqCst);
            // A dangling-but-nonnull sentinel: this test never dereferences it,
            // it only round-trips the pointer value through the handle.
            let ptr = NonNull::new(std::ptr::dangling_mut::<c_void>())
                .expect("dangling_mut never returns null");
            Some(unsafe { DesktopViewHandle::from_raw(ptr) })
        }

        fn update_params(&self, _view: &DesktopViewHandle, _params_json: &str) {}

        fn dispose(&self, _view: DesktopViewHandle) {}
    }

    /// Register then look up: the returned factory is usable and dispatches
    /// back to the same instance (observed via its call counter).
    #[test]
    fn register_then_lookup_round_trips_to_the_same_factory() {
        let factory = Arc::new(FakeFactory::new());
        register_view_factory("dev.frust.test.RoundTrip", factory.clone())
            .expect("first registration must succeed");

        let looked_up =
            lookup_view_factory("dev.frust.test.RoundTrip").expect("factory must be found");
        let handle = looked_up
            .create("{}")
            .expect("fake factory always returns Some");
        looked_up.dispose(handle);

        assert_eq!(factory.create_calls.load(Ordering::SeqCst), 1);
    }

    /// Registering the same `view_type` twice is `AlreadyRegistered`, not a
    /// silent replace.
    #[test]
    fn duplicate_registration_is_rejected() {
        let view_type = "dev.frust.test.Duplicate";
        register_view_factory(view_type, Arc::new(FakeFactory::new()))
            .expect("first registration must succeed");

        let second = register_view_factory(view_type, Arc::new(FakeFactory::new()));
        assert_eq!(
            second,
            Err(RegisterError::AlreadyRegistered(view_type.to_string()))
        );
    }

    /// A `view_type` nothing has registered looks up as `None`.
    #[test]
    fn unknown_view_type_looks_up_as_none() {
        assert!(lookup_view_factory("dev.frust.test.NeverRegistered").is_none());
    }

    /// Concurrent readers over the registry do not deadlock or panic — a
    /// smoke test for the `RwLock` read path being genuinely shared.
    #[test]
    fn concurrent_lookups_do_not_deadlock() {
        let view_type = "dev.frust.test.Concurrent";
        register_view_factory(view_type, Arc::new(FakeFactory::new()))
            .expect("first registration must succeed");

        let handles: Vec<_> = (0..8)
            .map(|_| {
                std::thread::spawn(move || {
                    assert!(lookup_view_factory(view_type).is_some());
                    assert!(lookup_view_factory("dev.frust.test.NeverRegistered").is_none());
                })
            })
            .collect();

        for handle in handles {
            handle.join().expect("reader thread must not panic");
        }

        assert!(registered_view_types().contains(&view_type));
    }
}
