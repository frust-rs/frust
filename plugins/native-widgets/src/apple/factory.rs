//! The iOS platform-view factory: ONE Rust `define_class!` Objective-C class
//! implementing the embedding's `FrustPlatformViewFactory` protocol — the
//! whole platform surface this plugin will ever need on Apple, however many
//! controls [`crate::runtime`] grows, and **zero Swift** (PLAN.md's Phase 0
//! spike 2, GO).
//!
//! # The frozen ObjC ↔ Rust contract
//!
//! | Protocol method (Swift) | ObjC selector | This module | Contract |
//! |---|---|---|---|
//! | `createView(paramsJson:) -> UIView` (**required**) | `createViewWithParamsJson:` | [`FrustNativeControlFactory::createView_paramsJson`] | Rust builds the real control and returns it; **every** failure path returns an empty placeholder `UIView` instead — see *The failure contract* below |
//! | `updateParams(_:paramsJson:)` (optional) | `updateParams:paramsJson:` | [`FrustNativeControlFactory::update_params`] | Props change, Rust-diffed before any setter runs |
//! | `disposeView(_:)` (optional) | `disposeView:` | [`FrustNativeControlFactory::dispose_view`] | Release everything the control retained |
//!
//! ## Two name pins, both load-bearing (Phase 0 spike 2's findings)
//!
//! 1. **The class's runtime name is pinned to
//!    [`FACTORY_CLASS_NAME`]** via `define_class!`'s `#[name = "…"]`. `#[name]`
//!    is *optional* in objc2 0.6 — omitting it auto-generates a name that
//!    embeds the crate version, and objc2's own docs advise library authors to
//!    omit it precisely so two SemVer-incompatible copies can coexist. **This
//!    crate must set it anyway**: the host resolves a factory by
//!    `NSClassFromString(viewType)` (`FrustViewHost.resolveFactory`) against
//!    the string `crate::api::builders`' iOS `VIEW_TYPE` publishes, so a
//!    version-bearing auto name would break the lookup on every version bump.
//!    A stable, hand-pinned name *is* the discovery mechanism here — do not
//!    "correct" this toward objc2's general advice.
//! 2. **The protocol is pinned to `"FrustPlatformViewFactory"`** via
//!    [`extern_protocol!`]'s own `#[name = "…"]` (objc2 otherwise derives the
//!    runtime name from the *Rust* trait name). Its Swift half is pinned the
//!    same way — `@objc(FrustPlatformViewFactory)` on the `public protocol`
//!    — because Swift would otherwise register it under its mangled name
//!    (`_TtP14FrustEmbedding24FrustPlatformViewFactory_`) and no non-Swift
//!    implementor could ever attach a conformance the host's
//!    `class_conformsToProtocol` check can see. That was spike 2's first
//!    on-device failure (a black hole where the control should be); the Swift
//!    fix shipped with the spike.
//!
//! [`FrustNativeControlFactory::createView_paramsJson`]: FrustNativeControlFactory
//! [`FrustNativeControlFactory::update_params`]: FrustNativeControlFactory
//! [`FrustNativeControlFactory::dispose_view`]: FrustNativeControlFactory
//!
//! ## The failure contract: a dead slot, never a nil return
//!
//! `FrustPlatformViewFactory.swift` declares `createView(paramsJson:) -> UIView`
//! — **non-optional** — and `FrustViewHost.applyCreate` immediately calls
//! `wrapper.addSubview(content)` with no nil check. Swift's non-optional
//! guarantee does **not** hold across an Objective-C-implemented method, which
//! is exactly what this module provides, so returning nil here would be an
//! unchecked implicitly-unwrapped-nil crash at first use.
//!
//! Therefore **every** failure path of [`createViewWithParamsJson:`][cv]
//! returns a plain, empty [`UIView`] ([`dead_slot_view`]) instead: an
//! invisible, zero-content slot the host can position and hide like any other.
//! That covers an unknown control kind, a control's own `create` failing, a
//! re-entrant runtime borrow, malformed params — and a **caught panic**. It is
//! the iOS analogue of the Android arm's "dead slot, not a crashed frame loop"
//! (`crate::android`'s contract table), reached through a different mechanism
//! because iOS has no exception channel to throw into: Android *throws* into
//! `applyCreate`'s `catch (Throwable)`, Apple *returns something harmless*.
//! `FrustPlatformViewFactory.swift`'s doc comment states the same contract; if
//! you change one, change the other (the c1-02 lesson).
//!
//! [cv]: FrustNativeControlFactory
//!
//! ## Which call carries the slot id
//!
//! None of the three, exactly as on Android: the protocol predates this
//! plugin. So `createView`/`updateParams` read the differ's slot id out of
//! `params_json` ([`crate::runtime::SLOT_KEY`], injected by the api layer),
//! and `disposeView` — which has no params at all — resolves the instance by
//! **view object identity** ([`same_view`] over
//! [`NativeRuntime::take_matching`](crate::runtime::NativeRuntime::take_matching)).
//! That is also what makes a late dispose safe: after a replacement `Create`
//! re-used the slot id, the stale dispose names a view no live instance has,
//! finds nothing, and leaves the live control alone (`crate::runtime`'s *late
//! and duplicate disposal*).
//!
//! **Why a scan and not a `ptr -> slot` side table.** A pointer-keyed map
//! would be O(1) instead of O(live slots), but it would be a *second* source
//! of truth that has to be invalidated on every replace, every identity-based
//! take, and every `dispose_slot` — and getting that wrong reintroduces the
//! exact "a late dispose deletes the live, already-replaced control" bug the
//! registry's design memo is built around. `Registry::remove_matching`'s
//! identity semantics are already the contract, already host-tested
//! (`crate::registry`'s tests, `crate::runtime`'s
//! `a_late_dispose_for_a_replaced_view_never_touches_the_live_one`), and the
//! scan is over the handful of native slots on screen. One source of truth
//! wins.
//!
//! # Registration is LAZY — and must be forced
//!
//! objc2 registers a `define_class!` class with the Objective-C runtime
//! lazily, on the first Rust call to `ClassType::class()` (documented as *"May
//! register the class with the runtime if it wasn't already"* — the API
//! promises no more than that, and neither does this module). **Nothing on the
//! Swift side can trigger it**: the host only ever asks for the class *by
//! name*, and `NSClassFromString` returns nil for a class that was never
//! registered. So [`ensure_registered`] exists, and something in live Rust
//! code has to call it.
//!
//! **The ordering contract**: it must run before the host polls the first
//! `Create` command for one of this plugin's slots. The host polls its command
//! backlog *after* a frame, and a slot is only ever published *during* a
//! frame's rebuild/paint — so any call from the rebuild that mounts a control
//! is early enough, with a whole frame of margin.
//! `crate::runtime::ensure_platform_factory` is where that call lives (from
//! `with_identity`, the one call every builder makes on exactly that
//! rebuild); `crate::api::ensure_native_factory_registered` is the explicit,
//! app-callable front door for anyone who wants it earlier still. Both are
//! idempotent — a `Once` plus, underneath it, objc2's own registration.
//!
//! # No `ios_exports!`/`#[used]` static is needed here — deliberately
//!
//! `frust-camera` needs one (`plugins/camera/src/apple.rs`'s
//! `ios_exports!`/`FRUST_CAMERA_IOS_EXPORTS`) because it exports a C function
//! that **no Rust code calls** and Swift links against by symbol name, which
//! `lto = "fat"` happily strips. **This factory has no such symbol.** It is an
//! ObjC class registered into the runtime *from live Rust code*
//! ([`ensure_registered`], reached from the api layer) and then found by name
//! through the ObjC runtime's own class table — nothing crosses the linker as
//! an orphan C export, so there is nothing for LTO to drop. Spike 2 proved
//! exactly this in a signed release build (`lto = "fat"` + `strip =
//! "symbols"`), and PLAN.md records it. Adding a `#[used]` static here would
//! keep alive a symbol that does not exist; do not "fix" this.
//!
//! # No unwind across FFI
//!
//! Every ObjC-entered method body below runs inside
//! `catch_unwind(AssertUnwindSafe(…))` — this crate's half of
//! `docs/CODE_STANDARDS.md`'s no-unwind-across-FFI rule, the Apple mirror of
//! what `jni::EnvUnowned::with_env` does for the Android exports.
//!
//! **One precise exception, stated rather than glossed** (p2-01 validation):
//! `createViewWithParamsJson:` acquires its `MainThreadMarker` *before*
//! entering the guard, because it must return a `UIView` on every path and so
//! its `Err` arm needs a marker as well — acquiring one there instead would
//! re-run the same failing call with nothing left to catch it. Under
//! `debug_assertions`, `MainThreadMarker::from` asserts, so a genuinely
//! off-main-thread call panics outside the guard. That is bounded and
//! deliberate: it can only happen when the host has already violated the
//! `MainThreadOnly` contract (in which case no sound recovery exists — you
//! cannot touch UIKit off the main thread), and the release profile is
//! `panic = "abort"`, so no unwind crosses this boundary in a shipped build.
//! `updateParams:paramsJson:` and `disposeView:` return `()` and therefore
//! acquire their markers INSIDE the guard, with no such exception.
//!
//! A caught
//! panic is logged and resolves to that method's benign default: an empty
//! placeholder view for `createView` (the failure contract above), and nothing
//! at all for the two `void` methods.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Once;

use objc2::rc::Retained;
use objc2::runtime::{AnyProtocol, NSObject, NSObjectProtocol};
use objc2::{ClassType, MainThreadMarker, MainThreadOnly, define_class, extern_protocol};
use objc2_foundation::NSString;
use objc2_ui_kit::UIView;

use crate::NativeWidgetError;
use crate::apple::NativeCtx;
use crate::runtime::{self, UpdateOutcome};

/// The Objective-C runtime name this crate's factory class registers under.
///
/// **This string and `crate::api::builders`' `#[cfg(target_os = "ios")]
/// VIEW_TYPE` must be byte-identical** — `VIEW_TYPE` is what a builder's
/// `platform_view(...)` slot publishes and what `FrustViewHost.resolveFactory`
/// feeds to `NSClassFromString`. A mismatch is silent: the lookup returns nil,
/// the host takes its unresolvable-factory branch, and every native control on
/// iOS renders nothing at all (`research/RESEARCH-P2-REFRESH.md` §6b). The
/// name matches the Android Kotlin class (`dev.frust.FrustNativeControlFactory`)
/// minus its package, which iOS does not have (`docs/CODE_STANDARDS.md`'s
/// Naming Conventions: bare `@objc(<Name>)` names are the iOS contract).
///
/// Kept as a named constant even though `define_class!`'s `#[name = "…"]`
/// attribute needs a literal and cannot reference it: this is the one place
/// the pin is *explained*, and [`ensure_registered`]'s receipt prints the
/// registered class name so a device gate can compare the two for real.
pub(crate) const FACTORY_CLASS_NAME: &str = "FrustNativeControlFactory";

/// The Objective-C runtime name of the embedding's factory protocol — pinned
/// on both sides (see the module doc's *Two name pins*).
const FACTORY_PROTOCOL_NAME: &std::ffi::CStr = c"FrustPlatformViewFactory";

extern_protocol!(
    /// The FrustEmbedding Swift package's `@objc(FrustPlatformViewFactory)
    /// public protocol` (`platform/ios/FrustEmbedding/Sources/FrustEmbedding/
    /// FrustPlatformViewFactory.swift`), declared here by **runtime name** so
    /// the Rust class below can register a conformance the host's
    /// `class_conformsToProtocol` check actually sees (spike 2's finding 2).
    ///
    /// Only the required method is declared. The two optional ones
    /// (`updateParams:paramsJson:`, `disposeView:`) are implemented as plain
    /// methods on the class with the protocol's selectors — objc2's
    /// `#[optional]` attribute has no effect inside `define_class!` yet, and
    /// the selector is all the ObjC runtime dispatches on.
    ///
    /// # Safety
    /// Selector and types mirror the Swift declaration exactly:
    /// `createView(paramsJson: String) -> UIView` ⇒
    /// `createViewWithParamsJson:` taking an `NSString` and returning a
    /// **nonnull** `UIView` (the module doc's *failure contract* is what keeps
    /// that "nonnull" honest).
    #[name = "FrustPlatformViewFactory"]
    pub(crate) unsafe trait FrustPlatformViewFactory: NSObjectProtocol {
        #[unsafe(method(createViewWithParamsJson:))]
        #[allow(non_snake_case)]
        fn createView_paramsJson(&self, params_json: &NSString) -> Retained<UIView>;
    }
);

define_class!(
    // SAFETY:
    // - `NSObject` has no subclassing requirements.
    // - No ivars, and no `Drop` impl (so the macro generates no `dealloc`).
    #[unsafe(super(NSObject))]
    // The host resolves, instantiates and calls this factory on the main
    // thread (`FrustPlatformViewFactory`'s documented contract, and
    // `FrustViewHost`'s own `CADisplayLink`-driven poll), and every UIKit type
    // a control touches is main-thread-only — so is this class. That is what
    // lets `MainThreadMarker::from(self)` below be a free, compile-time proof
    // rather than a runtime check (PLAN 2.1).
    #[thread_kind = MainThreadOnly]
    // See `FACTORY_CLASS_NAME`'s doc: this literal must equal that constant,
    // and both must equal `api::builders`' iOS `VIEW_TYPE`.
    #[name = "FrustNativeControlFactory"]
    pub(crate) struct FrustNativeControlFactory;

    unsafe impl NSObjectProtocol for FrustNativeControlFactory {}

    // SAFETY: implements the protocol's one required method with the declared
    // signature; the host calls it on the main thread.
    unsafe impl FrustPlatformViewFactory for FrustNativeControlFactory {
        /// `createView(paramsJson:)` → the Rust-built control.
        ///
        /// Returns the control's own retained view on success. On **any**
        /// failure — unknown control kind, the control's own `create` failing,
        /// a re-entrant runtime borrow, unreadable params, or a caught panic
        /// — this returns an empty placeholder [`UIView`] instead of nil, so
        /// the host gets an invisible dead slot rather than an unchecked
        /// implicitly-unwrapped-nil crash in `wrapper.addSubview(content)`.
        /// See the module doc's *failure contract*, and the matching sentence
        /// in `FrustPlatformViewFactory.swift`'s own doc comment.
        #[unsafe(method_id(createViewWithParamsJson:))]
        #[allow(non_snake_case)]
        fn createView_paramsJson(&self, params_json: &NSString) -> Retained<UIView> {
            // Deliberately OUTSIDE the guard, unlike the two methods below.
            // This method must return a `UIView` on every path, so the `Err`
            // arm needs a marker too; acquiring one there instead would mean a
            // second `MainThreadMarker::from` AFTER the first already panicked
            // — and that one would be uncaught, turning a caught panic into an
            // unwind across ObjC. Failing fast here is the safer shape. The
            // residual is bounded: `MainThreadMarker::from`'s assertion exists
            // only under `debug_assertions`, and the release profile is
            // `panic = "abort"`, so no unwind can cross this boundary in a
            // shipped build. See the module doc's no-unwind section.
            let mtm = MainThreadMarker::from(self);
            let created = catch_unwind(AssertUnwindSafe(|| {
                create_control(mtm, &params_json.to_string())
            }));
            match created {
                Ok(Some(view)) => view,
                // The failure was already reported by `create_control`.
                Ok(None) => dead_slot_view(mtm),
                Err(_) => {
                    log::warn!("{}", PANIC_MESSAGE_CREATE);
                    dead_slot_view(mtm)
                }
            }
        }
    }

    impl FrustNativeControlFactory {
        /// `updateParams(_:paramsJson:)` → a Rust-diffed props update.
        ///
        /// The `view` argument is part of the protocol but unused: the slot id
        /// in `paramsJson` is authoritative (module doc's *Which call carries
        /// the slot id*), and a params payload naming a slot with no live
        /// instance is tolerated rather than an error.
        #[unsafe(method(updateParams:paramsJson:))]
        fn update_params(&self, _view: &UIView, params_json: &NSString) {
            let outcome = catch_unwind(AssertUnwindSafe(|| {
                // Marker acquired INSIDE the guard, unlike `createView` above:
                // this method returns `()`, so nothing after the guard needs a
                // marker, and `MainThreadMarker::from`'s own debug assertion is
                // therefore caught here rather than unwinding past ObjC.
                let mtm = MainThreadMarker::from(self);
                update_control(mtm, &params_json.to_string());
            }));
            if outcome.is_err() {
                log::warn!("{}", PANIC_MESSAGE_UPDATE);
            }
        }

        /// `disposeView(_:)` → release everything the control retained.
        ///
        /// Resolution is by view object identity, never by "whichever instance
        /// holds that slot now" — see the module doc's *Which call carries the
        /// slot id*.
        #[unsafe(method(disposeView:))]
        fn dispose_view(&self, view: &UIView) {
            // Marker acquired inside the guard, for the reason given in
            // `updateParams:paramsJson:` above.
            let outcome = catch_unwind(AssertUnwindSafe(|| {
                dispose_control(MainThreadMarker::from(self), view)
            }));
            if outcome.is_err() {
                log::warn!("{}", PANIC_MESSAGE_DISPOSE);
            }
        }
    }
);

/// Force the lazy Objective-C-runtime registration of the factory class, so
/// the host's `NSClassFromString([`FACTORY_CLASS_NAME`])` can find it.
///
/// **Ordering contract**: call this before the host polls the first `Create`
/// command for one of this plugin's slots. The host polls its backlog *after*
/// a frame and a slot is only published *during* one, so any call from the
/// rebuild that mounts a control has a full frame of margin — see the module
/// doc's *Registration is LAZY*. Idempotent and cheap after the first call
/// (a `Once`, and `ClassType::class()` is itself documented only as *"may
/// register the class with the runtime if it wasn't already"* — this function
/// promises no more than that either).
///
/// Prints a one-line receipt to **stderr**, once per process — the crate's
/// one deliberate exception to its own "backends log, never print" rule
/// (`Cargo.toml`'s comment on the `log` dependency). `eprintln!` rather than
/// `log::info!` because a `--release` build carries the `lean` log ceiling
/// (`log/release_max_level_warn`), which compiles info-level logs out
/// entirely — and this receipt is *the* artifact task p2-05's bar 2 collects
/// ("`devicectl … launch --console` capture of the registration eprintln, as
/// the spike did") to prove the zero-Swift factory really did resolve in a
/// signed release build. Stderr reaches both `xcrun devicectl … process
/// launch --console` and `simctl launch --console-pty`.
pub(crate) fn ensure_registered() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let class = FrustNativeControlFactory::class();
        let protocol = AnyProtocol::get(FACTORY_PROTOCOL_NAME);
        let conforms = protocol.is_some_and(|protocol| class.conforms_to(protocol));
        eprintln!(
            "frust-native-widgets: registered ObjC class {:?} (expected {FACTORY_CLASS_NAME:?}); \
             protocol {FACTORY_PROTOCOL_NAME:?} found: {}; conforms: {conforms}",
            class.name(),
            protocol.is_some(),
        );
        if !conforms {
            // Spike 2's finding 1, should it ever regress: without
            // `@objc(FrustPlatformViewFactory)` on the Swift protocol, Swift
            // registers it under a mangled name and no Rust-attached
            // conformance is visible to `class_conformsToProtocol` — so the
            // host rejects this factory and every native control on iOS
            // renders nothing.
            log::warn!(
                "frust-native-widgets: the ObjC factory class does NOT conform to \
                 {FACTORY_PROTOCOL_NAME:?} — FrustViewHost will reject it and every native \
                 control will render nothing; check that FrustPlatformViewFactory.swift still \
                 carries its explicit @objc(FrustPlatformViewFactory) name"
            );
        }
    });
}

/// The typed half of `createViewWithParamsJson:`: dispatch, build, retain, and
/// hand the control's own view back to the host.
///
/// `None` means "report a dead slot" — the caller turns it into
/// [`dead_slot_view`] rather than nil (module doc's *failure contract*). Every
/// `None` path is logged here, so the caller adds nothing but the placeholder.
///
/// No create-rollback branch, unlike `crate::android`'s counterpart: handing
/// the view back there means allocating a fresh JNI local reference, which can
/// fail *after* the instance is already retained (and then nothing would ever
/// dispose it, so it rolls the slot back by hand). Here "handing it back" is a
/// `Retained` clone — an infallible ARC retain — so a successful
/// `NativeRuntime::create` is always a successful `createView`.
fn create_control(mtm: MainThreadMarker, params: &str) -> Option<Retained<UIView>> {
    let outcome = runtime::with_runtime(|runtime| {
        let mut ctx = NativeCtx::new(mtm);
        let slot_id = runtime.create(&mut ctx, params)?;
        let view = runtime
            .instance(slot_id)
            .expect("a successful create retained its instance")
            .view()
            .view(mtm)
            .clone();
        Ok::<_, NativeWidgetError>((slot_id, view))
    })
    // `with_runtime` returns `None` only when the runtime's `RefCell` is
    // already borrowed on this thread (a re-entrant call — a platform setter
    // fired its own action back into create). It logs that at the point of
    // detection; folding it into the same `Result` shape here is what makes it
    // fall through the one reporting path below instead of silently returning
    // a placeholder with no explanation (the Android arm's
    // `c1-02-create-contract-honesty.md` lesson, applied before it could
    // happen here).
    .unwrap_or_else(reentrant_create_failure);

    match outcome {
        Ok((slot_id, view)) => {
            log::debug!("frust-native-widgets: created slot {slot_id}");
            Some(view)
        }
        Err(error) => {
            log::warn!("{}", create_failure_message(&error));
            None
        }
    }
}

/// The typed half of `updateParams:paramsJson:`.
fn update_control(mtm: MainThreadMarker, params: &str) {
    runtime::with_runtime(|runtime| {
        let mut ctx = NativeCtx::new(mtm);
        match runtime.update_params(&mut ctx, params) {
            Ok(UpdateOutcome::Applied | UpdateOutcome::Unchanged) => {}
            Ok(UpdateOutcome::UnknownSlot) => {
                log::debug!("frust-native-widgets: updateParams for a slot with no instance");
            }
            Err(e) => log::warn!("frust-native-widgets: updateParams failed: {e}"),
        }
    });
}

/// The typed half of `disposeView:`.
///
/// Identity is resolved first, against every live instance's retained view;
/// only then is the teardown context built. A dispose naming a view no live
/// instance holds is expected, not exceptional (`crate::runtime`'s *late and
/// duplicate disposal*).
fn dispose_control(mtm: MainThreadMarker, view: &UIView) {
    let target: *const UIView = view;
    let taken = runtime::with_runtime(|runtime| {
        runtime.take_matching(|retained| same_view(Retained::as_ptr(retained.view(mtm)), target))
    });

    let Some((slot_id, instance)) = taken.flatten() else {
        log::debug!("frust-native-widgets: disposeView for an unknown view — ignored");
        return;
    };

    let mut ctx = NativeCtx::new(mtm);
    if let Err(e) = instance.dispose(&mut ctx) {
        log::warn!("frust-native-widgets: disposing slot {slot_id}: {e}");
    }
    let live = runtime::with_runtime(|runtime| runtime.live_count()).unwrap_or_default();
    log::debug!("frust-native-widgets: disposed slot {slot_id}; live controls={live}");
}

/// The empty placeholder every failure path of `createViewWithParamsJson:`
/// returns instead of nil — the module doc's *failure contract*.
///
/// A bare `UIView` has a nil `backgroundColor` and no subviews, so the host
/// positions and (until the first `update` says otherwise) hides an invisible,
/// inert rectangle: a dead slot, exactly like Android's, rather than an
/// implicitly-unwrapped-nil crash inside `FrustViewHost.applyCreate`. Its
/// frame is never set here — the host owns slot geometry (`crate::apple::ctx`'s
/// *Frame-setting layout only*).
fn dead_slot_view(mtm: MainThreadMarker) -> Retained<UIView> {
    UIView::new(mtm)
}

/// Object identity for the dispose lookup: iOS hands `disposeView:` back the
/// very view `createView` returned, so a raw pointer comparison *is* the
/// identity check (the Apple counterpart of Android's `Env::is_same_object`,
/// with no JNI round trip and no chance of a Java `equals` override getting
/// involved). Never dereferences either pointer.
fn same_view(a: *const UIView, b: *const UIView) -> bool {
    std::ptr::eq(a, b)
}

/// The typed failure [`create_control`] reports for `with_runtime`'s `None`
/// arm (the runtime's `RefCell` already borrowed on this thread). Kept as its
/// own function, mirroring the Android arm's identically-named helper, so the
/// message is testable without an ObjC runtime.
fn reentrant_create_failure<T>() -> Result<T, NativeWidgetError> {
    Err(NativeWidgetError::Platform(
        "runtime re-entrant — a platform setter fired its own action during create".into(),
    ))
}

/// Builds the warning [`create_control`] logs on a failed create — factored
/// out as a pure function so the message itself is testable, mirroring the
/// Android arm's `create_failure_message`. The trailing clause is the whole
/// point: it tells whoever reads a device log that the slot is *dead*, not
/// missing, which is what distinguishes this from a slot that never mounted.
fn create_failure_message(error: &NativeWidgetError) -> String {
    format!(
        "frust-native-widgets: createView failed: {error} — returning an empty placeholder view \
         (dead slot)"
    )
}

/// Caught-panic message for `createViewWithParamsJson:` — a const rather than
/// a `format!` so the panic path allocates nothing.
const PANIC_MESSAGE_CREATE: &str = "frust-native-widgets: panic caught in createView — returning \
                                    an empty placeholder view (dead slot)";

/// Caught-panic message for `updateParams:paramsJson:`.
const PANIC_MESSAGE_UPDATE: &str =
    "frust-native-widgets: panic caught in updateParams — the slot keeps its previous props";

/// Caught-panic message for `disposeView:`.
const PANIC_MESSAGE_DISPOSE: &str =
    "frust-native-widgets: panic caught in disposeView — the slot's native references may leak";

// These exercise the pure helpers above the way `crate::android`'s
// `create_failure_message_tests` exercises its own: the module is
// `#[cfg(target_os = "ios")]`-gated and this repo has no iOS test runner, so
// they cannot RUN on a host — `cargo check --target aarch64-apple-ios-sim -p
// frust-native-widgets --tests` is the documented compile-gated equivalent,
// the same limitation every other platform-gated test in this crate carries.
// The runtime-level logic these helpers feed (identity-based dispose, the
// diff gate, late/duplicate dispose) IS host-run, in `crate::runtime`'s and
// `crate::registry`'s own test modules.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_class_name_pin_is_spelled_once() {
        // `define_class!`'s `#[name = "…"]` needs a literal, so the constant
        // cannot feed it directly; this is the tripwire for the two drifting
        // apart, and `ensure_registered`'s receipt prints both for the device
        // gate to compare at runtime.
        assert_eq!(FACTORY_CLASS_NAME, "FrustNativeControlFactory");
        assert_eq!(FACTORY_PROTOCOL_NAME, c"FrustPlatformViewFactory");
    }

    #[test]
    fn create_failure_message_names_the_error_and_the_dead_slot() {
        let message = create_failure_message(&NativeWidgetError::UnknownControl("bogus".into()));
        assert!(message.contains("createView failed"));
        assert!(message.contains("bogus"));
        assert!(message.contains("dead slot"));
    }

    #[test]
    fn reentrancy_is_reported_distinctly() {
        let Err(error) = reentrant_create_failure::<()>() else {
            panic!("reentrant_create_failure must always return Err");
        };
        let message = create_failure_message(&error);
        assert!(message.contains("createView failed"));
        assert!(message.contains("re-entrant"));
    }

    #[test]
    fn same_view_is_address_identity_and_never_dereferences() {
        // Fabricated addresses: `same_view` compares them, nothing more, so a
        // test needs no live UIView (which no host process could make anyway).
        let a = 0x1000 as *const UIView;
        let b = 0x2000 as *const UIView;
        assert!(same_view(a, a));
        assert!(!same_view(a, b));
        assert!(!same_view(a, std::ptr::null()));
    }
}
