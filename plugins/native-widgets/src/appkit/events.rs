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
//! Five arms ([`detail_for`]) — one per interactive control, except
//! [`EVENT_KIND_VALUE_CHANGED`], which `Slider` and `Stepper` share: both
//! read the sender's `doubleValue` generically off `&NSControl`, with no
//! per-class check, so a value-committing control this arm attaches gets the
//! same read regardless of which one it is:
//!
//! | kind | control | payload read off `sender` |
//! |------|---------|---------------------------|
//! | [`EVENT_KIND_CLICK`] | `NSButton` | none (`0`) |
//! | [`EVENT_KIND_TOGGLED`] | `NSSwitch` | `state` → [`pack_bool`] |
//! | [`EVENT_KIND_VALUE_CHANGED`] | `NSSlider` / `NSStepper` | `doubleValue` → [`pack_value_changed`] |
//! | [`EVENT_KIND_SELECTION`] | `NSSegmentedControl` | `selectedSegment` → [`pack_index`] |
//! | [`EVENT_KIND_DATE`] | `NSDatePicker` | `dateValue` → civil date → [`pack_date`] |
//!
//! A public `NativeComponent` reaches the same class through
//! `FrustNativeControlTarget::attach_view` (called by
//! `crate::component::ComponentCtx::attach_listener`) — one kind per control,
//! the slot id supplied by the component's context and never seen by the
//! component, the target retained in the component's `ListenerHandle`, whose
//! `Drop` clears the control's target/action while they are still its own.
//!
//! # Drag start/end are never emitted on macOS
//!
//! The iOS arm reports a slider's gesture edges from two extra UIKit control
//! events (`TouchDown` → [`EVENT_KIND_DRAG_START`], `TouchUpInside|Outside` →
//! [`EVENT_KIND_DRAG_END`]), and Android from `onStart/StopTrackingTouch`.
//! AppKit target-action has no counterpart: an `NSSlider` with `continuous`
//! set sends its ONE action on every drag step, and the action carries no
//! phase. Recovering the edges would mean inspecting `NSApp.currentEvent`'s
//! type inside the action or overriding `NSSliderCell`'s tracking methods —
//! an `NSEvent` dependency and a second class, neither of which this arm
//! takes on. So **`DragStart`/`DragEnd` are never emitted on macOS**, and
//! never synthesized from the value stream either (a guessed gesture edge is
//! worse than an honest absence); an app that needs them gets them on the
//! two mobile arms only. [`detail_for`] refuses both kinds, so a target
//! mis-wired with one logs and drops rather than reporting a fabricated edge.
//!
//! # Kind/detail parity is automatic, not re-derived
//!
//! Every control's macOS `on_event` calls the exact same decode function its
//! Android and iOS counterparts do (`crate::events::decode_click` for
//! `Button`, `crate::controls::switch::decode_toggled` for `Switch`,
//! `crate::controls::slider::decode_event` for `Slider`,
//! `crate::controls::segmented::decode_event` for the Apple-only
//! `Segmented`, `crate::controls::stepper::decode_event` for the
//! Apple-only `Stepper` (the two Apple-only decoders shared with iOS alone),
//! and `crate::controls::date_picker::decode_event` for `DatePicker`, shared
//! by all three arms),
//! so parity falls out of one shared decoder per control rather than being
//! asserted here.
//!
//! # Target retention: explicit, per slot, in the control's own `State`
//!
//! `NSControl.target` is a **weak** (unretained) property — AppKit never
//! retains a control's target — so production must retain it explicitly for
//! exactly the slot's lifetime. That retention lives in each control's own
//! `State` (`ButtonState`/`SwitchState`/`SliderState`'s `target`), not in
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
//! programmatic `setState:`/`setDoubleValue:` (the action is sent from the
//! control's own mouse/keyboard tracking), so no action this class ever
//! receives is an echo of `update`'s controlled-component write-back
//! (`crate::controls::switch`'s module doc). Should one ever be — a
//! `setState:` issued while `frustAction:` is still on the stack re-entering
//! it — the protection is `crate::runtime::with_runtime`'s re-entrancy drop:
//! `update` runs inside that borrow, so a nested action finds it held, gets
//! `None` back, and is dropped before any `on_event` or app callback runs (the
//! `Ok(None)` arm of [`FrustNativeControlTarget`]'s dispatch). Nothing is
//! built here for it, and there is no per-instance suppression flag to latch.
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
#[cfg(feature = "frust-api")]
use objc2_app_kit::NSView;
use objc2_app_kit::{
    NSControl, NSControlStateValue, NSControlStateValueOn, NSDatePicker, NSSegmentedControl,
    NSSwitch,
};

use crate::controls::date_picker::foundation::civil_date;
use crate::events::{
    EVENT_KIND_CLICK, EVENT_KIND_DATE, EVENT_KIND_SELECTION, EVENT_KIND_TOGGLED,
    EVENT_KIND_VALUE_CHANGED, pack_bool, pack_date, pack_index, pack_value_changed,
};
#[cfg(doc)]
use crate::events::{EVENT_KIND_DRAG_END, EVENT_KIND_DRAG_START};
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

    /// [`Self::attach`] for the public `NativeComponent` path
    /// (`crate::component::ComponentCtx::attach_listener`): `view` is any view
    /// the component built, checked to be an `NSControl` (the only AppKit class
    /// with a target/action pair) — and, for [`EVENT_KIND_TOGGLED`], an
    /// `NSSwitch`, since [`detail_for`] reads the toggled payload off one and
    /// would drop every event from anything else. `slot` comes from the
    /// component's context, so the component never holds it.
    ///
    /// Answers the target (the caller's to retain — `NSControl.target` is
    /// weak) and the control, retained so the caller can detach later
    /// ([`Self::detach_if_current`]); a mismatch answers `Err` naming it, with
    /// nothing attached.
    #[cfg(feature = "frust-api")]
    pub(crate) fn attach_view(
        mtm: MainThreadMarker,
        view: &NSView,
        slot: SlotId,
        kind: i32,
    ) -> Result<(Retained<Self>, Retained<NSControl>), String> {
        use objc2::Message as _;

        let any: &AnyObject = view;
        let control = any.downcast_ref::<NSControl>().ok_or_else(|| {
            "macOS attach_listener: the view is not an NSControl, so it has no target/action \
             pair to attach to"
                .to_string()
        })?;
        if kind == EVENT_KIND_TOGGLED && any.downcast_ref::<NSSwitch>().is_none() {
            return Err(
                "macOS attach_listener: TOGGLED reads an NSSwitch's state, and this control is \
                 no NSSwitch"
                    .into(),
            );
        }
        let target = Self::attach(mtm, control, slot, kind);
        Ok((target, control.retain()))
    }

    /// Clear `control`'s target/action **only while they are still this
    /// target's** — the detach a component's `ListenerHandle` runs on drop.
    ///
    /// Unlike the six controls' unconditional [`Self::detach`], a component
    /// may re-attach the same control (a fresh handle replacing an old one in
    /// its state); dropping the old handle must not clear the new target, so
    /// this compares identities first.
    #[cfg(feature = "frust-api")]
    pub(crate) fn detach_if_current(&self, control: &NSControl) {
        let current = control.target();
        let ours: &AnyObject = self;
        if current
            .as_deref()
            .is_some_and(|target| std::ptr::eq(target, ours))
        {
            self.detach(control);
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
                     this arm does not encode (or whose sender is not the control that kind reads) \
                     — event dropped"
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
/// encode (the drag edges — module doc), or when the payload's source is
/// missing (a nil `sender`, or a toggle/selection/date whose sender is not
/// an `NSSwitch`/`NSSegmentedControl`/`NSDatePicker`, or a date outside the
/// representable 0001-9999 range).
///
/// A click carries none (`crate::events::decode_click` reads only the kind),
/// so `sender` is unused for it; the toggle/value/selection/date kinds read
/// the sender's `state`/`doubleValue`/`selectedSegment`/`dateValue` through
/// the same `crate::events` `pack_*` codecs the Android listener and the iOS
/// target use.
fn detail_for(kind: i32, sender: Option<&NSControl>) -> Option<i64> {
    match kind {
        EVENT_KIND_CLICK => Some(0),
        EVENT_KIND_TOGGLED => {
            // `state` is `NSSwitch`'s own member (AppKit keeps it on the
            // cell-backed classes, not on `NSControl`), so the sender is
            // downcast rather than messaged blind.
            let any: &AnyObject = sender?;
            let switch = any.downcast_ref::<NSSwitch>()?;
            Some(pack_bool(is_on(switch.state())))
        }
        // `from_user` is unconditionally `true`, for the reason the iOS arm's
        // `handleSliderValueChanged:` gives: AppKit never sends the action for
        // a programmatic `setDoubleValue:` (module doc's *No echo guard*), so
        // every firing here is a genuine user drag.
        EVENT_KIND_VALUE_CHANGED => Some(pack_value_changed(
            platform_value(sender?.doubleValue()),
            true,
        )),
        EVENT_KIND_SELECTION => {
            // `selectedSegment` is `NSSegmentedControl`'s own member, so the
            // sender is downcast like the toggle's. Packed signed and
            // verbatim: a `-1` (no segment) report crosses the wire and is
            // refused by the shared decoder, not here.
            let any: &AnyObject = sender?;
            let segmented = any.downcast_ref::<NSSegmentedControl>()?;
            Some(pack_index(segmented.selectedSegment()))
        }
        EVENT_KIND_DATE => {
            // `dateValue` is `NSDatePicker`'s own member; converted through
            // the SAME Gregorian mapping the control's setters and the iOS
            // target use (`crate::controls::date_picker::foundation`).
            let any: &AnyObject = sender?;
            let picker = any.downcast_ref::<NSDatePicker>()?;
            Some(pack_date(civil_date(&picker.dateValue())?))
        }
        _ => None,
    }
}

/// An `NSSwitch` `state` as the toggled payload's boolean: on is
/// `NSControlStateValueOn`; anything else (off, and the mixed state an
/// `NSSwitch` never takes) is off.
fn is_on(state: NSControlStateValue) -> bool {
    state == NSControlStateValueOn
}

/// An `NSSlider` `doubleValue` as the **platform-space** integer
/// [`pack_value_changed`] carries.
///
/// Already platform-space: the slider's `create` pins `minValue` at `0` and
/// `maxValue` at the app's span (`crate::controls::slider`'s macOS module
/// doc), so no `min` is subtracted here — `crate::controls::slider::decode_event`
/// adds it back identically on all three arms. Rounded to the nearest integer
/// (not truncated), matching the iOS arm: a continuous slider stops between
/// integers, and the nearest one is the user's intended stop. The `as` cast
/// saturates and maps NaN to `0`, so no AppKit value can wrap.
fn platform_value(double_value: f64) -> i32 {
    double_value.round() as i32
}

// This module is `#[cfg(target_os = "macos")]`-gated (via `crate::appkit`),
// and constructing the target (or any `NSControl` sender) needs a
// `MainThreadMarker`, which no `cargo test` worker thread ever holds — so the
// ObjC half is compile-checked (`cargo check --target aarch64-apple-darwin`)
// and exercised by the macOS playground gate. The pure functions here are
// host-run below (on a macOS host, where this module compiles).
#[cfg(test)]
mod tests {
    use objc2_app_kit::{NSControlStateValueMixed, NSControlStateValueOff};

    use super::*;
    use crate::events::{
        EVENT_KIND_DRAG_END, EVENT_KIND_DRAG_START, decode_click, unpack_bool, unpack_value_changed,
    };

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
    fn the_drag_edges_are_refused_never_synthesized() {
        // Module doc's *Drag start/end are never emitted on macOS*.
        assert_eq!(detail_for(EVENT_KIND_DRAG_START, None), None);
        assert_eq!(detail_for(EVENT_KIND_DRAG_END, None), None);
    }

    #[test]
    fn a_payload_kind_with_no_sender_is_refused_not_zeroed() {
        // A zeroed toggle would read as "off" and a zeroed value as "0" —
        // both plausible, both fabricated.
        assert_eq!(detail_for(EVENT_KIND_TOGGLED, None), None);
        assert_eq!(detail_for(EVENT_KIND_VALUE_CHANGED, None), None);
        assert_eq!(detail_for(EVENT_KIND_SELECTION, None), None);
        assert_eq!(detail_for(EVENT_KIND_DATE, None), None);
    }

    #[test]
    fn only_the_on_state_is_on() {
        assert!(unpack_bool(pack_bool(is_on(NSControlStateValueOn))));
        assert!(!is_on(NSControlStateValueOff));
        assert!(!is_on(NSControlStateValueMixed));
    }

    #[test]
    fn a_slider_value_rounds_to_the_nearest_platform_space_integer() {
        assert_eq!(platform_value(0.0), 0);
        assert_eq!(platform_value(41.49), 41);
        assert_eq!(platform_value(41.5), 42);
        assert_eq!(platform_value(99.9), 100);
        assert_eq!(platform_value(f64::NAN), 0, "NaN never wraps");
        let packed = pack_value_changed(platform_value(7.6), true);
        assert_eq!(unpack_value_changed(packed), (8, true));
    }
}
