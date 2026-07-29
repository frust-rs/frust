//! iOS target-action: ONE Rust `define_class!` target object **per slot**,
//! wired straight into the SAME `(kind, detail)` runtime dispatch Android's
//! `FrustNativeListener` feeds.
//!
//! # One class, five actions — the Android mirror
//!
//! Android's `dev.frust.nativewidgets.FrustNativeListener` implements every
//! listener interface a v1 control needs (`OnClickListener`/
//! `OnCheckedChangeListener`/`OnSeekBarChangeListener`) on ONE class, with one
//! method per callback. [`FrustNativeControlTarget`] is the same shape for
//! UIKit target-action: one class, five action selectors
//! ([`Self::handle_click`], [`Self::handle_switch_value_changed`],
//! [`Self::handle_slider_value_changed`], [`Self::handle_slider_drag_start`],
//! [`Self::handle_slider_drag_end`]), and a control wires only the ones it
//! needs via [`Self::attach_button`]/[`Self::attach_switch`]/
//! [`Self::attach_slider`] — exactly as Android attaches the shared listener
//! only as the one interface a given control implements. Adding a control
//! never adds a second target class, matching the crate's "ONE generic
//! factory, ONE generic listener" charter (`crate`'s module doc).
//!
//! # Kind/detail parity is automatic, not re-derived
//!
//! Every action below packs its payload through the SAME primitive codec
//! Android's listener uses ([`crate::events`]'s `pack_bool`/
//! `pack_value_changed`, and the shared `EVENT_KIND_*` constants), and every
//! control's Apple `on_event` calls the exact SAME decode function its
//! Android counterpart does (`crate::events::decode_click`,
//! `super::decode_toggled`, `super::decode_event` — see `controls/button.rs`/
//! `switch.rs`/`slider.rs`'s Apple `on_event` impls). Parity therefore isn't
//! a property this module has to assert; it falls out of both platforms
//! funnelling into one shared decoder per control.
//!
//! # Target retention: explicit, per slot, in the control's own `State`
//!
//! `UIControl` holds its targets **weakly** (`addTarget:action:forControlEvents:`
//! does not retain `target` — Apple's own documented contract). An earlier
//! prototype leaned on a factory-level cache to keep a target alive, which
//! does not generalize to N independently-created-and-disposed slots;
//! production must retain the target explicitly, for exactly the slot's own
//! lifetime.
//!
//! That retention lives in each control's own `State` (`ButtonState`/
//! `SwitchState`/`SliderState`'s new `target: Retained<FrustNativeControlTarget>`
//! field), **not** in [`crate::registry::apple::AppleHandle`] (the
//! `NativeView` the runtime's registry keys by slot). Android's equivalent
//! secondary retention — the listener object a control's `create` attaches —
//! lives in the registry entry's `extra` list precisely because Android's
//! `NativeView` (`AndroidHandle`) is a bag of `Global<JObject>`s with no
//! typed access back to the control; Apple's registry entry
//! ([`crate::registry::apple::AppleHandle`]) already exists only to answer
//! "which view is this" for identity-based dispose (`crate::apple::factory`'s
//! *Which call carries the slot id*) and is never handed back to a control's
//! own `update`/`dispose` — the control's typed `State` is. Extending
//! `AppleHandle` with a second, untyped ARC slot would duplicate that
//! retention discipline for no behavioral gain: ARC already releases
//! whatever `State` holds the moment `Instance::dispose` drops it
//! (`crate::runtime::Instance::dispose`'s `(vtable.dispose)(ctx, state)` step,
//! immediately before the paired `drop(view)`), and a `State` field is
//! exactly where the Apple arm already keeps its OWN second reference to the
//! view too (`ButtonState`/`SwitchState`/`SliderState`'s `view` field). So the
//! per-slot leak bar this task's acceptance calls for is the same one every
//! other control cycle already satisfies:
//! `crate::runtime::tests`'/`crate::registry::tests`' create→dispose cycles
//! returning [`crate::runtime::NativeRuntime::live_count`] to zero — a target
//! left retained only in `State` is dropped in that same step, with nothing
//! left dangling.
//!
//! Each control's `dispose` additionally calls [`Self::detach_button`]/
//! [`Self::detach_switch`]/[`Self::detach_slider`] before returning, mirroring
//! Android's explicit `setOnClickListener(null)`-style detach: belt-and-braces
//! against a stray in-flight action firing after the slot's instance is gone,
//! even though ARC alone would already release the target once `State` drops.
//!
//! # No echo guard here, either
//!
//! None of the five actions below needs one. Every action UIKit ever sends
//! here is genuinely user-caused: Apple's UIControl guidance is *"As a rule
//! UIKit does not send events when programmatic changes are made to
//! controls"*, and `switch.rs`'s module doc is the full account (the two
//! caveats included) — this module adds no per-instance suppression flag,
//! same as that doc's contract. Should Caveat B (a `setOn:`-from-inside-
//! `valueChanged` re-entrant report) ever fire, the protection is inherited
//! from `crate::runtime::with_runtime`'s re-entrancy drop, not built here.
//!
//! # No unwind across FFI
//!
//! Every action method runs inside `catch_unwind` — this module's half of
//! `docs/CODE_STANDARDS.md`'s no-unwind-across-FFI rule, the same discipline
//! `crate::apple::factory`'s three ObjC-entered methods follow. A caught
//! panic is logged and the event is simply dropped — there is no return
//! value here for a caller to be handed a benign default through (every
//! action is `-> ()`), unlike `createView`'s must-return-something contract.
//!
//! # Threading
//!
//! UIKit dispatches target-action on the thread that sent the triggering
//! touch event, which for every control this crate builds is always the main
//! thread — the same guarantee `crate::apple::factory`'s classes rest on. This
//! class is therefore `#[thread_kind = MainThreadOnly]` too, so every
//! construction site must carry a live [`MainThreadMarker`] (in practice,
//! always the one a control's `create` already holds via `NativeCtx::mtm`).
//!
//! # No separate "ensure registered" contract, unlike the factory
//!
//! `crate::apple::factory::ensure_registered` exists because the HOST looks
//! up that class **by name** (`NSClassFromString`), and nothing on the Swift
//! side can trigger its lazy Objective-C registration. Nothing ever looks
//! this class up by name — it is only ever constructed directly from live
//! Rust code (`Self::new`, reached from `Self::attach_*`), and `objc2`
//! registers a `define_class!` class with the runtime lazily on that first
//! `Self::alloc`/`Self::class()` call regardless, with no separate trigger
//! required.

use std::panic::{AssertUnwindSafe, catch_unwind};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, Sel};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_ui_kit::{UIButton, UIControl, UIControlEvents, UISlider, UISwitch};

use crate::events::{
    EVENT_KIND_CLICK, EVENT_KIND_DRAG_END, EVENT_KIND_DRAG_START, EVENT_KIND_TOGGLED,
    EVENT_KIND_VALUE_CHANGED, pack_bool, pack_value_changed,
};
use crate::registry::SlotId;
use crate::runtime::{self, NativeEvent};

define_class!(
    // SAFETY:
    // - `NSObject` has no subclassing requirements.
    // - The ivars are a plain `SlotId` (`u64`) with no `Drop` impl, so the
    //   macro's generated `dealloc` has nothing extra to uphold.
    #[unsafe(super(NSObject))]
    // See the module doc's *Threading*: every action fires on the main
    // thread, so construction (`Self::alloc`) must too.
    #[thread_kind = MainThreadOnly]
    #[ivars = SlotId]
    pub(crate) struct FrustNativeControlTarget;

    unsafe impl NSObjectProtocol for FrustNativeControlTarget {}

    impl FrustNativeControlTarget {
        /// `Button`'s `TouchUpInside` action → [`EVENT_KIND_CLICK`], `detail`
        /// unused — the same shape as `crate::events::decode_click`'s whole
        /// input.
        #[unsafe(method(handleClick:))]
        fn handle_click(&self, _sender: &UIButton) {
            self.dispatch(EVENT_KIND_CLICK, 0, "handleClick:");
        }

        /// `Switch`'s `ValueChanged` action → [`EVENT_KIND_TOGGLED`]. Reads
        /// the platform's own reported state straight off the sender — there
        /// is nowhere else to read it from, and (module doc's *No echo guard*)
        /// every firing here is a genuine user toggle.
        #[unsafe(method(handleSwitchValueChanged:))]
        fn handle_switch_value_changed(&self, sender: &UISwitch) {
            let detail = pack_bool(sender.isOn());
            self.dispatch(EVENT_KIND_TOGGLED, detail, "handleSwitchValueChanged:");
        }

        /// `Slider`'s `ValueChanged` action → [`EVENT_KIND_VALUE_CHANGED`].
        ///
        /// `sender.value()` is already **platform-space**: `create` pins
        /// `minimumValue` at `0` and `maximumValue` at the app's span
        /// (`crate::controls::slider`'s Apple module doc's *Construction-time
        /// normalization*), mirroring `SeekBar`'s own zero-based range — so
        /// this needs no `min` of its own; `crate::controls::slider::decode_event`
        /// adds it back identically on both platforms. Rounded to the
        /// nearest integer because `UISlider.value` is a continuous
        /// `CGFloat` where `SeekBar.progress` is already an integer —
        /// Android's own wire shape leaves no fractional part to preserve,
        /// so rounding (rather than truncating) is the closer match to a
        /// user's intended stop.
        #[unsafe(method(handleSliderValueChanged:))]
        fn handle_slider_value_changed(&self, sender: &UISlider) {
            // `from_user` is unconditionally `true`: UIKit never sends
            // `ValueChanged` for a programmatic `setValue:` (module doc's
            // *No echo guard*; `switch.rs`'s module doc is the full
            // account), so every firing this handler ever sees is a genuine
            // user drag — unlike Android's wire shape, which carries the bit
            // because `SeekBar` itself reports it (even though a `false`
            // report never reaches this crate's decoder either, dropped one
            // layer up by the re-entrancy guard).
            let detail = pack_value_changed(sender.value().round() as i32, true);
            self.dispatch(
                EVENT_KIND_VALUE_CHANGED,
                detail,
                "handleSliderValueChanged:",
            );
        }

        /// `Slider`'s `TouchDown` action → [`EVENT_KIND_DRAG_START`].
        #[unsafe(method(handleSliderDragStart:))]
        fn handle_slider_drag_start(&self, _sender: &UISlider) {
            self.dispatch(EVENT_KIND_DRAG_START, 0, "handleSliderDragStart:");
        }

        /// `Slider`'s `TouchUpInside`/`TouchUpOutside` action →
        /// [`EVENT_KIND_DRAG_END`] — registered against the OR'd mask of
        /// both events ([`Self::attach_slider`]), so a drag that ends
        /// outside the thumb's current bounds still reports the end.
        #[unsafe(method(handleSliderDragEnd:))]
        fn handle_slider_drag_end(&self, _sender: &UISlider) {
            self.dispatch(EVENT_KIND_DRAG_END, 0, "handleSliderDragEnd:");
        }
    }
);

impl FrustNativeControlTarget {
    /// A target for `slot`, not yet wired to any control —
    /// [`Self::attach_button`]/[`Self::attach_switch`]/[`Self::attach_slider`]
    /// are the whole public construction surface.
    fn new(mtm: MainThreadMarker, slot: SlotId) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(slot);
        // SAFETY: `NSObject`'s designated initializer, called on a freshly
        // allocated instance whose ivars are already set (mirrors
        // `crate::apple` camera's — `plugins/camera/src/apple.rs`'s
        // `PhotoCaptureDelegate::new` — identical idiom).
        unsafe { msg_send![super(this), init] }
    }

    /// The slot this target reports events for.
    fn slot(&self) -> SlotId {
        *self.ivars()
    }

    /// Decode `(kind, detail)` and forward to
    /// [`crate::runtime::NativeRuntime::on_event`] — every action method's
    /// whole body, modulo which kind/detail it packed.
    ///
    /// `which` names the firing selector for the caught-panic/re-entrancy log
    /// lines only; it carries no dispatch meaning.
    fn dispatch(&self, kind: i32, detail: i64, which: &'static str) {
        let slot = self.slot();
        let event = NativeEvent { kind, detail };
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            runtime::with_runtime(|runtime| runtime.on_event(slot, event))
        }));
        match outcome {
            // Mirrors `crate::android`'s `Java_dev_frust_nativewidgets_FrustNativeListener_nativeOnEvent`
            // exactly: `with_runtime` returning `None` means a re-entrant
            // call (module doc's *No echo guard* — the shared re-entrancy
            // drop, not anything this module built).
            Ok(delivered) => {
                if delivered.is_none() {
                    log::debug!("frust-native-widgets: event for slot {slot} dropped (re-entrant)");
                }
            }
            Err(_) => {
                log::warn!(
                    "frust-native-widgets: panic caught in {which} for slot {slot} — event dropped"
                );
            }
        }
    }

    /// `control.addTarget(self, action, for: events)` — the one call every
    /// `attach_*` below makes, once per event mask.
    fn add(&self, control: &UIControl, action: Sel, events: UIControlEvents) {
        let target: &AnyObject = self;
        // SAFETY: `target` is `self`, a live `FrustNativeControlTarget`
        // implementing exactly the action selectors this module ever passes
        // here, and `control` is the live `UIControl` subclass instance the
        // caller just built (or is tearing down, for `Self::remove` below).
        unsafe { control.addTarget_action_forControlEvents(Some(target), action, events) };
    }

    /// `control.removeTarget(self, action, for: events)` — [`Self::add`]'s
    /// inverse.
    fn remove(&self, control: &UIControl, action: Sel, events: UIControlEvents) {
        let target: &AnyObject = self;
        // SAFETY: see `Self::add` — removing a target/action pair that was
        // never added (or already removed) is a documented UIKit no-op, not
        // a precondition this call must uphold.
        unsafe { control.removeTarget_action_forControlEvents(Some(target), Some(action), events) };
    }

    /// Build a target for `slot` and wire it as `view`'s `TouchUpInside`
    /// action — `Button`'s whole click-attach.
    pub(crate) fn attach_button(
        mtm: MainThreadMarker,
        slot: SlotId,
        view: &UIButton,
    ) -> Retained<Self> {
        let target = Self::new(mtm, slot);
        target.add(view, sel!(handleClick:), UIControlEvents::TouchUpInside);
        target
    }

    /// [`Self::attach_button`]'s inverse — called from `Button::dispose`.
    pub(crate) fn detach_button(&self, view: &UIButton) {
        self.remove(view, sel!(handleClick:), UIControlEvents::TouchUpInside);
    }

    /// Build a target for `slot` and wire it as `view`'s `ValueChanged`
    /// action — `Switch`'s whole toggle-attach.
    pub(crate) fn attach_switch(
        mtm: MainThreadMarker,
        slot: SlotId,
        view: &UISwitch,
    ) -> Retained<Self> {
        let target = Self::new(mtm, slot);
        target.add(
            view,
            sel!(handleSwitchValueChanged:),
            UIControlEvents::ValueChanged,
        );
        target
    }

    /// [`Self::attach_switch`]'s inverse — called from `Switch::dispose`.
    pub(crate) fn detach_switch(&self, view: &UISwitch) {
        self.remove(
            view,
            sel!(handleSwitchValueChanged:),
            UIControlEvents::ValueChanged,
        );
    }

    /// Build a target for `slot` and wire it as `view`'s `ValueChanged`,
    /// `TouchDown`, and `TouchUpInside|TouchUpOutside` actions — `Slider`'s
    /// whole value/drag-attach.
    pub(crate) fn attach_slider(
        mtm: MainThreadMarker,
        slot: SlotId,
        view: &UISlider,
    ) -> Retained<Self> {
        let target = Self::new(mtm, slot);
        target.add(
            view,
            sel!(handleSliderValueChanged:),
            UIControlEvents::ValueChanged,
        );
        target.add(
            view,
            sel!(handleSliderDragStart:),
            UIControlEvents::TouchDown,
        );
        target.add(
            view,
            sel!(handleSliderDragEnd:),
            UIControlEvents::TouchUpInside | UIControlEvents::TouchUpOutside,
        );
        target
    }

    /// [`Self::attach_slider`]'s inverse — called from `Slider::dispose`.
    pub(crate) fn detach_slider(&self, view: &UISlider) {
        self.remove(
            view,
            sel!(handleSliderValueChanged:),
            UIControlEvents::ValueChanged,
        );
        self.remove(
            view,
            sel!(handleSliderDragStart:),
            UIControlEvents::TouchDown,
        );
        self.remove(
            view,
            sel!(handleSliderDragEnd:),
            UIControlEvents::TouchUpInside | UIControlEvents::TouchUpOutside,
        );
    }
}

// This module is `#[cfg(target_os = "ios")]`-gated (via `crate::apple`) and
// this repo has no iOS test runner, so nothing here can run on a host —
// `cargo check --target aarch64-apple-ios-sim -p frust-native-widgets --tests`
// is the documented compile-gated equivalent every other Apple-arm module in
// this crate carries (`crate::apple::factory`'s own tests module doc says the
// same). The kind/detail values and decode logic this module feeds ARE
// host-run, in `crate::events`'s and each control's own shared (platform-
// agnostic) test modules.
