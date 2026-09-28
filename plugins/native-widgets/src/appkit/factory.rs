//! The macOS platform-view factory: ONE Rust
//! [`DesktopViewFactory`] registered with `frust_plugin::desktop` under
//! [`VIEW_TYPE`], which `crates/frust-shell-macos`' desktop Mode-A host
//! resolves for every slot this plugin's builders publish — the whole
//! platform surface this arm needs however many controls [`crate::runtime`]
//! grows, with no C export, no Swift and no `#[unsafe(no_mangle)]`.
//!
//! # The contract with the desktop host
//!
//! | `DesktopViewFactory` method | This module | Contract |
//! |---|---|---|
//! | `create(params_json)` | [`create_control`] | Rust builds the real control and hands the host a +1-retained `NSView`; **every** failure returns an attachable empty view instead — *The failure contract*, below |
//! | `update_params(view, params_json)` | [`update_control`] | Props change, Rust-diffed before any setter runs; the slot id in the params is authoritative, the lent view is not consulted |
//! | `dispose(view)` | [`dispose_control`] | Take the +1 back, resolve the instance by view identity, release everything it retained |
//!
//! All three run on the platform main thread only (`frust_plugin::desktop`'s
//! contract; the host's winit event loop is that thread), inside
//! `catch_unwind` here as well as in the host — `docs/CODE_STANDARDS.md`'s
//! no-unwind-across-FFI rule, the same discipline `crate::apple::factory`'s
//! three ObjC-entered methods follow.
//!
//! # Which call carries the slot id
//!
//! Same as iOS: `create`/`update_params` read the differ's slot id out of the
//! params' reserved `__frustSlot` key (`crate::runtime`'s generic-factory
//! contract), while `dispose` is handed only the view — so disposal resolves
//! by **object identity** (`NativeRuntime::take_matching` over a raw
//! pointer comparison), never by "whichever instance holds that slot now"
//! (`crate::runtime`'s *Late and duplicate disposal*).
//!
//! # Retain accounting — one retain, exchanged twice
//!
//! Adapted from `plugins/video-player/src/macos_view.rs`' identical section,
//! and the exact mirror of the host's own half
//! (`crates/frust-shell-macos/src/platform_view.rs`' *Retain accounting*):
//!
//! - **`create`** clones the instance's own `Retained<NSView>` (one ARC
//!   retain — the runtime keeps its own reference in the registry's
//!   [`AppKitHandle`](crate::registry::appkit::AppKitHandle)) and hands that
//!   **same +1** to the host by consuming it with [`Retained::into_raw`] into a
//!   [`DesktopViewHandle`]. Nothing here calls an extra `retain`: the +1
//!   `create` promises *is* the `Retained` it already owned. A dead-slot view
//!   is handed over the same way, with the handle's +1 as its only retain.
//! - **`update_params`** never touches the count: the host lends a handle
//!   built from the pointer it still owns, and this module does not even read
//!   it (the params name the slot).
//! - **`dispose`** takes the +1 back with [`Retained::from_raw`] on the exact
//!   pointer `create` handed out, resolves and disposes the runtime instance
//!   holding the same view (releasing the runtime's own reference and the
//!   control's `State`), and lets the reclaimed `Retained` drop — one retain
//!   out, one release in, net zero. A dead-slot view has no instance, so its
//!   reclaimed `Retained` is its last reference.
//!
//! # The failure contract: an empty view, never a declined create
//!
//! The desktop host treats a declined create (`None`) as terminal for that
//! slot: it records nothing, ignores every later update naming it, and the
//! differ never re-issues `Create` for a live slot whose `view_type` is
//! unchanged — so `None` strands the slot forever. Every recoverable failure
//! therefore answers `Some` of an empty `NSView` ([`dead_slot_view`]): unknown
//! control kind (the kinds this arm does not register yet land here),
//! malformed params, a control's own create error, a re-entrant runtime
//! borrow, and a caught panic. Each is logged once, at the point it happens.
//!
//! **The one `None`**: a call off the main thread. `NSView` is
//! `MainThreadOnly`, so there is no view this call could soundly construct —
//! it is a caller contract violation (the host checks `MainThreadMarker`
//! itself and skips the whole batch first), not a recoverable slot state, and
//! `plugins/video-player/src/macos_view.rs` answers it the same way.
//!
//! # Registration is lazy, and first-wins
//!
//! [`ensure_registered`] runs from `crate::runtime::ensure_platform_factory`,
//! which every builder's params encode (`crate::runtime::with_identity`) and
//! `crate::component::register_component` reach — a frame before the host can
//! drain the first `Create` naming [`VIEW_TYPE`]. It is a `Once`, because
//! `frust_plugin::desktop::register_view_factory` is
//! first-registration-wins and answers `AlreadyRegistered` to any later call.

use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::NonNull;
use std::sync::{Arc, Once};

use frust_plugin::desktop::{DesktopViewFactory, DesktopViewHandle, register_view_factory};
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::NSView;

use super::NativeCtx;
use crate::NativeWidgetError;
use crate::runtime::{self, UpdateOutcome};

/// The desktop `view_type` string this factory registers under — the key the
/// host resolves a slot's factory by, and therefore the api layer's macOS
/// `VIEW_TYPE` (`crate::api::builders` names this constant rather than
/// repeating the literal). The Android factory's FQCN spelling, so the
/// wire-facing string is the same on the two arms that key by string (iOS
/// keys by bare ObjC class name instead).
pub(crate) const VIEW_TYPE: &str = "dev.frust.nativewidgets.FrustNativeControlFactory";

/// This arm's [`DesktopViewFactory`] — stateless; every instance lives in the
/// thread-local `crate::runtime::NativeRuntime`.
pub(crate) struct AppKitFactory;

impl DesktopViewFactory for AppKitFactory {
    fn create(&self, params_json: &str) -> Option<DesktopViewHandle> {
        let Some(mtm) = MainThreadMarker::new() else {
            // Module doc's *The one `None`*.
            log::error!("{OFF_MAIN_CREATE_MESSAGE}");
            return None;
        };
        let created = catch_unwind(AssertUnwindSafe(|| create_control(mtm, params_json)));
        let view = match created {
            Ok(Some(view)) => view,
            // Already reported by `create_control`.
            Ok(None) => dead_slot_view(mtm),
            Err(_) => {
                log::warn!("{PANIC_MESSAGE_CREATE}");
                dead_slot_view(mtm)
            }
        };
        into_handle(view)
    }

    fn update_params(&self, _view: &DesktopViewHandle, params_json: &str) {
        // The lent handle is not consulted: the slot id in the params is
        // authoritative (module doc's *Which call carries the slot id*).
        let outcome = catch_unwind(AssertUnwindSafe(|| match MainThreadMarker::new() {
            Some(mtm) => update_control(mtm, params_json),
            None => log::error!(
                "frust-native-widgets: macOS update_params called off the main thread — ignored"
            ),
        }));
        if outcome.is_err() {
            log::warn!("{PANIC_MESSAGE_UPDATE}");
        }
    }

    fn dispose(&self, view: DesktopViewHandle) {
        let ptr = view.into_raw();
        let Some(mtm) = MainThreadMarker::new() else {
            // Releasing an `NSView` off the main thread is exactly what
            // `MainThreadOnly` forbids; leaking one +1 is the lesser failure
            // for a contract violation the host already guards against.
            log::error!(
                "frust-native-widgets: macOS dispose called off the main thread — leaking the \
                 view's retain rather than releasing it off-main"
            );
            return;
        };
        let outcome = catch_unwind(AssertUnwindSafe(|| dispose_control(mtm, ptr)));
        if outcome.is_err() {
            log::warn!("{PANIC_MESSAGE_DISPOSE}");
        }
    }
}

/// Register [`AppKitFactory`] with the desktop platform-view registry under
/// [`VIEW_TYPE`], once per process.
///
/// Idempotent and cheap after the first call (a `Once`). An
/// `AlreadyRegistered` answer is logged at debug, not treated as a failure:
/// the desktop registry is process-global and first-wins, while this `Once`
/// is per crate *instance* — so the one way to reach it is a second,
/// semver-incompatible copy of this crate linked into the same binary, whose
/// own `Once` fires after this one's (or the reverse). Either copy's factory
/// serves the same wire contract; the incumbent keeps it.
pub(crate) fn ensure_registered() {
    static ONCE: Once = Once::new();
    ONCE.call_once(
        || match register_view_factory(VIEW_TYPE, Arc::new(AppKitFactory)) {
            Ok(()) => log::info!(
                "frust-native-widgets: registered the macOS desktop view factory for {VIEW_TYPE:?}"
            ),
            Err(error) => log::debug!(
                "frust-native-widgets: macOS desktop view factory not registered ({error}) — the \
                 incumbent keeps {VIEW_TYPE:?}"
            ),
        },
    );
}

/// The typed half of `create`: dispatch, build, retain, and hand the
/// control's own view back.
///
/// `None` means "report a dead slot" — the caller turns it into
/// [`dead_slot_view`] (module doc's *failure contract*); every `None` path is
/// logged here. No `unwrap`/`expect`: an instance missing right after a
/// successful create is reported like any other failure.
fn create_control(mtm: MainThreadMarker, params: &str) -> Option<Retained<NSView>> {
    let outcome = runtime::with_runtime(|runtime| {
        let mut ctx = NativeCtx::new(mtm);
        let slot_id = runtime.create(&mut ctx, params)?;
        let view = runtime
            .instance(slot_id)
            .map(|instance| Retained::clone(instance.view().view(mtm)))
            .ok_or_else(|| {
                NativeWidgetError::Platform(format!(
                    "slot {slot_id} has no live instance right after a successful create"
                ))
            })?;
        Ok::<_, NativeWidgetError>((slot_id, view))
    })
    // `with_runtime` answers `None` only for a re-entrant call; folding it into
    // the same `Result` keeps it on the one reporting path below.
    .unwrap_or_else(reentrant_create_failure);

    match outcome {
        Ok((slot_id, view)) => {
            log::debug!("frust-native-widgets: macOS created slot {slot_id}");
            Some(view)
        }
        Err(error) => {
            log::warn!("{}", create_failure_message(&error));
            None
        }
    }
}

/// The typed half of `update_params`.
fn update_control(mtm: MainThreadMarker, params: &str) {
    let outcome = runtime::with_runtime(|runtime| {
        let mut ctx = NativeCtx::new(mtm);
        runtime.update_params(&mut ctx, params)
    });
    match outcome {
        Some(Ok(UpdateOutcome::Applied | UpdateOutcome::Unchanged)) => {}
        Some(Ok(UpdateOutcome::UnknownSlot)) => {
            log::debug!("frust-native-widgets: macOS update_params for a slot with no instance");
        }
        Some(Err(error)) => log::warn!("frust-native-widgets: macOS update_params failed: {error}"),
        None => log::debug!("frust-native-widgets: macOS update_params dropped (re-entrant)"),
    }
}

/// The typed half of `dispose`: reclaim the +1, resolve the instance by view
/// identity, tear it down, then release the reclaimed reference.
fn dispose_control(mtm: MainThreadMarker, ptr: NonNull<c_void>) {
    let target = ptr.as_ptr().cast::<NSView>().cast_const();
    // SAFETY: `ptr` is exactly the pointer `create` handed the host with a +1
    // retain (module doc's *Retain accounting*), and the host hands each
    // pointer back through `dispose` at most once — so this reclaims that same
    // retain rather than fabricating one. Only on the main thread (`mtm`).
    let reclaimed = unsafe { Retained::<NSView>::from_raw(ptr.as_ptr().cast()) };

    let taken = runtime::with_runtime(|runtime| {
        runtime.take_matching(|handle| same_view(Retained::as_ptr(handle.view(mtm)), target))
    });
    match taken {
        Some(Some((slot_id, instance))) => {
            let mut ctx = NativeCtx::new(mtm);
            if let Err(error) = instance.dispose(&mut ctx) {
                log::warn!("frust-native-widgets: macOS disposing slot {slot_id}: {error}");
            }
            let live = runtime::with_runtime(|runtime| runtime.live_count()).unwrap_or_default();
            log::debug!(
                "frust-native-widgets: macOS disposed slot {slot_id}; live controls={live}"
            );
        }
        // A dead-slot view, or a late/duplicate dispose (`crate::runtime`'s
        // *Late and duplicate disposal*) — expected, not exceptional.
        Some(None) => {
            log::debug!("frust-native-widgets: macOS dispose for a view with no instance")
        }
        None => log::warn!(
            "frust-native-widgets: macOS dispose dropped (re-entrant runtime) — the instance \
             stays live until its slot is disposed again"
        ),
    }
    // Releases the host's +1 (the last reference for a dead-slot view).
    drop(reclaimed);
}

/// Hand `view` to the host as a +1-retained [`DesktopViewHandle`] (module
/// doc's *Retain accounting*).
fn into_handle(view: Retained<NSView>) -> Option<DesktopViewHandle> {
    let ptr = NonNull::new(Retained::into_raw(view).cast::<c_void>())?;
    // SAFETY: `ptr` is the +1-retained `NSView` produced just above; it stays
    // valid until the host hands this same pointer back through `dispose`, and
    // the host touches it only on the main thread (this call already holds a
    // `MainThreadMarker`).
    Some(unsafe { DesktopViewHandle::from_raw(ptr) })
}

/// The empty view every recoverable create failure answers instead of `None`
/// — module doc's *failure contract*. A bare `NSView` draws nothing and takes
/// no input beyond what the responder chain routes past it; its frame is the
/// host's to set (`crate::appkit::ctx`'s *Frame-setting layout only*).
fn dead_slot_view(mtm: MainThreadMarker) -> Retained<NSView> {
    NSView::new(mtm)
}

/// Object identity for the dispose lookup: the host hands `dispose` the very
/// pointer `create` returned, so a raw pointer comparison *is* the identity
/// check. Never dereferences either pointer.
fn same_view(a: *const NSView, b: *const NSView) -> bool {
    std::ptr::eq(a, b)
}

/// The typed failure [`create_control`] reports for `with_runtime`'s `None`
/// arm (the runtime's `RefCell` already borrowed on this thread).
fn reentrant_create_failure<T>() -> Result<T, NativeWidgetError> {
    Err(NativeWidgetError::Platform(
        "runtime re-entrant — a platform setter fired its own action during create".into(),
    ))
}

/// The warning [`create_control`] logs on a failed create — a pure function
/// so the wording is host-testable. The trailing clause tells a log reader the
/// slot is *dead*, not missing.
fn create_failure_message(error: &NativeWidgetError) -> String {
    format!(
        "frust-native-widgets: macOS create failed: {error} — returning an empty placeholder view \
         (dead slot)"
    )
}

/// Logged when `create` is reached off the main thread (module doc's *The one
/// `None`*).
const OFF_MAIN_CREATE_MESSAGE: &str = "frust-native-widgets: macOS create called off the main \
                                          thread — declining (no NSView can be built off-main)";

/// Caught-panic message for `create`.
const PANIC_MESSAGE_CREATE: &str = "frust-native-widgets: panic caught in macOS create — \
                                    returning an empty placeholder view (dead slot)";

/// Caught-panic message for `update_params`.
const PANIC_MESSAGE_UPDATE: &str =
    "frust-native-widgets: panic caught in macOS update_params — the slot keeps its previous props";

/// Caught-panic message for `dispose`.
const PANIC_MESSAGE_DISPOSE: &str =
    "frust-native-widgets: panic caught in macOS dispose — the slot's native references may leak";

// The ObjC half needs a `MainThreadMarker`, which no `cargo test` worker
// thread ever holds (`MainThreadMarker::new()` is `None` off the process's
// real main thread), so `create`/`update_params`/`dispose` against a real
// `NSView` are compile-checked here and exercised by the macOS playground
// gate — the same split `plugins/video-player/src/macos_view.rs` documents.
// The pure helpers are host-run below.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_view_type_is_the_wire_string_the_builders_publish() {
        assert_eq!(
            VIEW_TYPE,
            "dev.frust.nativewidgets.FrustNativeControlFactory"
        );
    }

    #[test]
    fn create_failure_message_names_the_error_and_the_dead_slot() {
        let message = create_failure_message(&NativeWidgetError::UnknownControl("bogus".into()));
        assert!(message.contains("create failed"));
        assert!(message.contains("bogus"));
        assert!(message.contains("dead slot"));
    }

    #[test]
    fn reentrancy_is_reported_distinctly() {
        let Err(error) = reentrant_create_failure::<()>() else {
            panic!("reentrant_create_failure must always return Err");
        };
        assert!(create_failure_message(&error).contains("re-entrant"));
    }

    #[test]
    fn same_view_is_address_identity_and_never_dereferences() {
        let a = 0x1000 as *const NSView;
        let b = 0x2000 as *const NSView;
        assert!(same_view(a, a));
        assert!(!same_view(a, b));
        assert!(!same_view(a, std::ptr::null()));
    }

    #[test]
    fn create_off_the_main_thread_declines_instead_of_building_a_view() {
        // A `cargo test` worker is off the process's main thread — exactly the
        // one path the failure contract answers with `None`. (A single-threaded
        // harness may run a test on the real main thread instead; there is
        // nothing to assert about the off-main path then.)
        if MainThreadMarker::new().is_some() {
            return;
        }
        assert!(AppKitFactory.create("{}").is_none());
    }
}
