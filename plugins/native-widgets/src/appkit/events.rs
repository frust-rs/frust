//! macOS target-action: ONE Rust `define_class!` target class, one instance
//! **per attached control**, wired straight into the SAME `(kind, detail)`
//! runtime dispatch Android's `FrustNativeListener` and iOS's
//! `crate::apple::events` feed.
//!
//! # One class, one selector — the AppKit shape
//!
//! UIKit's `addTarget:action:forControlEvents:` lets one target register a
//! different selector per control event, which is why the iOS class carries
//! five. An `NSControl` has exactly ONE `target`/`action` pair and sends it on
//! the control's single "value committed" moment (a button click, a switch
//! flip, a slider move while `continuous` is set). So this class has one
//! action selector, `frustAction:`, and each instance carries the event kind
//! its control reports in its ivars next to the slot id; the action reads
//! whatever payload that kind needs off the `sender` and packs it with the
//! SAME `crate::events` codecs (`EVENT_KIND_*`, `pack_*`) the other two arms
//! use. Adding a control never adds a second class — the crate's "ONE generic
//! factory, ONE generic listener" charter (`crate`'s module doc).
//!
//! Only the click arm ([`EVENT_KIND_CLICK`], `Button`) exists yet; the
//! toggle/value arms join [`detail_for`] when their controls land on this arm.
//!
//! # Kind/detail parity is automatic, not re-derived
//!
//! Every control's macOS `on_event` calls the exact same decode function its
//! Android and iOS counterparts do (`crate::events::decode_click` for
//! `Button`), so parity falls out of one shared decoder per control rather
//! than being asserted here.
//!
//! # Target retention: explicit, per slot, in the control's own `State`
//!
//! `NSControl.target` is a **weak** (unretained) property — AppKit never
//! retains a control's target — so production must retain it explicitly for
//! exactly the slot's lifetime. That retention lives in each control's own
//! `State` (`ButtonState::target`), not in
//! [`crate::registry::appkit::AppKitHandle`], for the reason
//! `crate::apple::events`' module doc gives: `State` is dropped by
//! `Instance::dispose`, which releases the target in the same step as the
//! control's second view reference. Each control's `dispose` additionally
//! calls [`FrustNativeControlTarget::detach`] first — belt-and-braces against
//! an in-flight action reaching a torn-down slot.
//!
//! # No echo guard here
//!
//! AppKit sends a control's action for user interaction, not for a
//! programmatic `setState:`/`setDoubleValue:`, so no action this class ever
//! receives is an echo of `update`; should one ever be, the protection is
//! `crate::runtime::with_runtime`'s re-entrancy drop, not anything built here
//! (`crate::controls`' module doc).
//!
//! # No unwind across FFI
//!
//! The action method runs inside `catch_unwind` — this module's half of
//! `docs/CODE_STANDARDS.md`'s no-unwind-across-FFI rule. A caught panic is
//! logged and the event dropped; the action returns `()`, so there is nothing
//! else to hand back.
//!
//! # Threading
//!
//! AppKit sends target-action on the main thread (the event loop's own), so
//! this class is `#[thread_kind = MainThreadOnly]` and every construction site
//! carries a live [`MainThreadMarker`] — in practice the one a control's
//! `create` already holds via `NativeCtx::mtm`.
//!
//! # No registration step
//!
//! Nothing looks this class up by name, so it needs no `ensure_registered`
//! (unlike the factory, which the desktop host resolves by `view_type`):
//! `objc2` registers a `define_class!` class lazily on the first
//! `Self::alloc`, which `attach` always performs.

use std::panic::{AssertUnwindSafe, catch_unwind};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::NSControl;

use crate::events::EVENT_KIND_CLICK;
use crate::registry::SlotId;
use crate::runtime::{self, NativeEvent};

/// [`FrustNativeControlTarget`]'s ivars: which slot this target reports for,
/// and which `crate::events` kind its control's single action means.
#[derive(Clone, Copy)]
pub(crate) struct TargetIvars {
    /// The slot whose runtime instance receives the event.
    slot: SlotId,
    /// One of `crate::events`' `EVENT_KIND_*` codes.
    kind: i32,
}

define_class!(
    // SAFETY:
    // - `NSObject` has no subclassing requirements.
    // - The ivars are two plain integers with no `Drop` impl, so the macro's
    //   generated `dealloc` has nothing extra to uphold.
    #[unsafe(super(NSObject))]
    // See the module doc's *Threading*: every action fires on the main
    // thread, so construction (`Self::alloc`) must too.
    #[thread_kind = MainThreadOnly]
    #[ivars = TargetIvars]
    pub(crate) struct FrustNativeControlTarget;

    unsafe impl NSObjectProtocol for FrustNativeControlTarget {}

    impl FrustNativeControlTarget {
        /// The one action every attached `NSControl` sends. `sender` is the
        /// control itself (AppKit's documented action shape); it is read only
        /// by kinds whose payload lives on the control.
        #[unsafe(method(frustAction:))]
        fn frust_action(&self, sender: Option<&NSControl>) {
            self.dispatch(sender);
        }
    }
);

impl FrustNativeControlTarget {
    /// A target for `slot` reporting `kind`, not yet wired to any control —
    /// [`Self::attach`] is the whole public construction surface.
    fn new(mtm: MainThreadMarker, slot: SlotId, kind: i32) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(TargetIvars { slot, kind });
        // SAFETY: `NSObject`'s designated initializer, sent once to a freshly
        // allocated instance whose ivars are already set (the same idiom as
        // `crate::apple::events`' `FrustNativeControlTarget::new`).
        unsafe { msg_send![super(this), init] }
    }

    /// Build a target for `slot` and wire it as `control`'s target/action —
    /// the attach every interactive macOS control's `create` makes, once.
    ///
    /// The returned `Retained` is the ONLY strong reference to the target
    /// (`NSControl.target` is weak — module doc's *Target retention*): the
    /// caller must keep it in its `State` for the slot's lifetime.
    pub(crate) fn attach(
        mtm: MainThreadMarker,
        control: &NSControl,
        slot: SlotId,
        kind: i32,
    ) -> Retained<Self> {
        let this = Self::new(mtm, slot, kind);
        let target: &AnyObject = &this;
        // SAFETY: `target` is a live `FrustNativeControlTarget` implementing
        // `frustAction:` (the action set on the next line), retained by the
        // caller's `State` for as long as it stays attached; `control` is the
        // live control the caller just built. objc2 marks both setters
        // `unsafe` only because the header leaves weak-target lifetime and the
        // selector's signature to the caller, which this paragraph closes.
        unsafe {
            control.setTarget(Some(target));
            control.setAction(Some(sel!(frustAction:)));
        }
        this
    }

    /// [`Self::attach`]'s inverse — clears `control`'s target/action, called
    /// from the control's `dispose` before `State` (and with it this target)
    /// drops.
    pub(crate) fn detach(&self, control: &NSControl) {
        // SAFETY: clearing both to nil is the documented "no action" state of
        // an `NSControl`; nothing is dereferenced.
        unsafe {
            control.setTarget(None);
            control.setAction(None);
        }
    }

    /// Pack this target's kind + the sender's payload and forward to
    /// [`crate::runtime::NativeRuntime::on_event`] — the action's whole body.
    fn dispatch(&self, sender: Option<&NSControl>) {
        let TargetIvars { slot, kind } = *self.ivars();
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            let Some(detail) = detail_for(kind, sender) else {
                log::warn!(
                    "frust-native-widgets: macOS action for slot {slot} carries kind {kind}, which \
                     this arm does not encode yet — event dropped"
                );
                return Some(());
            };
            let event = NativeEvent { kind, detail };
            runtime::with_runtime(|runtime| {
                runtime.on_event(slot, event);
            })
        }));
        match outcome {
            // `with_runtime` answering `None` is a re-entrant call — dropped
            // by the shared re-entrancy guard (module doc's *No echo guard*).
            Ok(Some(())) => {}
            Ok(None) => {
                log::debug!("frust-native-widgets: event for slot {slot} dropped (re-entrant)");
            }
            Err(_) => {
                log::warn!(
                    "frust-native-widgets: panic caught in frustAction: for slot {slot} — event \
                     dropped"
                );
            }
        }
    }
}

/// The `detail` payload for one firing of `kind`, read off `sender` where the
/// kind's payload lives on the control — `None` for a kind this arm does not
/// encode yet.
///
/// A click carries none (`crate::events::decode_click` reads only the kind),
/// so `sender` is unused for it; the toggle/value kinds read the sender's
/// `state`/`doubleValue` through the same `crate::events` `pack_*` codecs.
fn detail_for(kind: i32, sender: Option<&NSControl>) -> Option<i64> {
    let _ = sender;
    match kind {
        EVENT_KIND_CLICK => Some(0),
        _ => None,
    }
}

// This module is `#[cfg(target_os = "macos")]`-gated (via `crate::appkit`),
// and constructing the target needs a `MainThreadMarker`, which no `cargo
// test` worker thread ever holds — so the ObjC half is compile-checked
// (`cargo check --target aarch64-apple-darwin`) and exercised by the macOS
// playground gate. The one pure function here is host-run below.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{EVENT_KIND_TOGGLED, decode_click};

    #[test]
    fn a_click_packs_no_detail_and_decodes_through_the_shared_decoder() {
        let detail = detail_for(EVENT_KIND_CLICK, None);
        assert_eq!(detail, Some(0));
        assert_eq!(
            decode_click(NativeEvent {
                kind: EVENT_KIND_CLICK,
                detail: 0,
            }),
            Some(crate::events::EventPayload::Click)
        );
    }

    #[test]
    fn a_kind_this_arm_does_not_encode_yet_is_refused_not_zeroed() {
        assert_eq!(detail_for(EVENT_KIND_TOGGLED, None), None);
    }
}
