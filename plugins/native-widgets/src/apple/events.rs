//! iOS target-action: ONE Rust `define_class!` target object **per slot**,
//! wired straight into the SAME `(kind, detail)` runtime dispatch Android's
//! `FrustNativeListener` feeds.
//!
//! # One class, eight actions — the Android mirror
//!
//! Android's `dev.frust.nativewidgets.FrustNativeListener` implements every
//! listener interface a v1 control needs (`OnClickListener`/
//! `OnCheckedChangeListener`/`OnSeekBarChangeListener`/
//! `OnDateChangedListener`) on ONE class, with one
//! method per callback. [`FrustNativeControlTarget`] is the same shape for
//! UIKit target-action: one class, eight action selectors
//! ([`Self::handle_click`], [`Self::handle_switch_value_changed`],
//! [`Self::handle_slider_value_changed`], [`Self::handle_slider_drag_start`],
//! [`Self::handle_slider_drag_end`], [`Self::handle_segment_value_changed`],
//! [`Self::handle_stepper_value_changed`],
//! [`Self::handle_date_value_changed`]), and a control wires only the ones
//! it needs via [`Self::attach_button`]/[`Self::attach_switch`]/
//! [`Self::attach_slider`]/[`Self::attach_segmented`]/[`Self::attach_stepper`]/
//! [`Self::attach_date_picker`]
//! — exactly as Android attaches the shared listener
//! only as the one interface a given control implements. Adding a control
//! never adds a second target class, matching the crate's "ONE generic
//! factory, ONE generic listener" charter (`crate`'s module doc).
//!
//! [`Self::handle_stepper_value_changed`] is its own action rather than a
//! reuse of [`Self::handle_slider_value_changed`], even though both report
//! [`EVENT_KIND_VALUE_CHANGED`] off a `value`-named property: `UISlider.value`
//! is a `float`-returning selector and `UIStepper.value` a `double`-returning
//! one, and the Rust binding [`Self::handle_slider_value_changed`] is typed
//! against reads that return type off the wire via `msg_send!`'s ABI —
//! calling it against a real `UIStepper` sender would read the wrong bit
//! width off the return register. Same wire shape, same kind, genuinely
//! different sender type — `crate::controls::stepper`'s module doc names the
//! shared shape.
//!
//! A public `NativeComponent` reaches the same class through
//! `FrustNativeControlTarget::attach_component` (called by
//! `crate::component::ComponentCtx::attach_listener`), which wires the same
//! selector/event pairs for whichever families the component asks for, onto
//! any control it built. The component's context supplies the slot id, so a
//! component never holds one; its target lives in the component's
//! `ListenerHandle`, whose `Drop` runs `detach_component` — the same
//! retention and detach discipline as below, one owner over.
//!
//! # Kind/detail parity is automatic, not re-derived
//!
//! Every action below packs its payload through the SAME primitive codec
//! Android's listener uses ([`crate::events`]'s `pack_bool`/
//! `pack_value_changed`, and the shared `EVENT_KIND_*` constants) — bit-packed
//! `i32`/`i64` primitives, not JSON, on this hot path (unlike `create`/
//! `update`'s `params_json`). Every control's Apple `on_event` calls the
//! exact SAME decode function its Android counterpart does
//! (`crate::events::decode_click`, `super::decode_toggled`,
//! `super::decode_event` — see `controls/button.rs`/`switch.rs`/`slider.rs`'s
//! Apple `on_event` impls). Parity therefore isn't a property this module has
//! to assert; it falls out of both platforms funnelling into one shared
//! decoder per control.
//!
//! # Target retention: explicit, per slot, in the control's own `State`
//!
//! `UIControl` holds its targets **weakly** (`addTarget:action:forControlEvents:`
//! does not retain `target` — Apple's own documented contract), so production
//! must retain the target explicitly, for exactly the slot's own lifetime —
//! this is the confined `addTarget:action:` attach/detach unsafe site named in
//! `docs/CODE_STANDARDS.md`'s Language Idioms.
//!
//! That retention lives in each control's own `State` (`ButtonState`/
//! `SwitchState`/`SliderState`'s `target: Retained<FrustNativeControlTarget>`
//! field), **not** in [`crate::registry::apple::AppleHandle`] — that entry
//! exists only to answer "which view is this" for identity-based dispose
//! (`crate::apple::factory`'s *Which call carries the slot id*) and is never
//! handed back to a control's own `update`/`dispose`, the typed `State` is.
//! ARC already releases whatever `State` holds the moment
//! `Instance::dispose` drops it, and a `State` field is exactly where the
//! Apple arm already keeps its OWN second reference to the view too
//! (`ButtonState`/`SwitchState`/`SliderState`'s `view` field) — so a target
//! retained only in `State` is dropped in that same step, with nothing left
//! dangling, verified by `crate::runtime::tests`'/`crate::registry::tests`'
//! create→dispose cycles returning
//! [`crate::runtime::NativeRuntime::live_count`] to zero. (Android's
//! equivalent secondary retention lives in the registry entry's `extra` list
//! instead, because its `AndroidHandle` is an untyped bag of
//! `Global<JObject>`s with no typed access back to the control — Apple's
//! typed `State` needs no such indirection.)
//!
//! Each control's `dispose` additionally calls [`Self::detach_button`]/
//! [`Self::detach_switch`]/[`Self::detach_slider`] before returning, mirroring
//! Android's explicit `setOnClickListener(null)`-style detach: belt-and-braces
//! against a stray in-flight action firing after the slot's instance is gone,
//! even though ARC alone would already release the target once `State` drops.
//!
//! # No echo guard here, either
//!
//! None of the six actions below needs one. Every action UIKit ever sends
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
#[cfg(feature = "frust-api")]
use objc2_ui_kit::UIView;
use objc2_ui_kit::{
    UIButton, UIControl, UIControlEvents, UIDatePicker, UISegmentedControl, UISlider, UIStepper,
    UISwitch,
};

use crate::controls::date_picker::foundation::civil_date;
use crate::events::{
    EVENT_KIND_CLICK, EVENT_KIND_DATE, EVENT_KIND_DRAG_END, EVENT_KIND_DRAG_START,
    EVENT_KIND_SELECTION, EVENT_KIND_TOGGLED, EVENT_KIND_VALUE_CHANGED, pack_bool, pack_date,
    pack_index, pack_value_changed,
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
        /// input. Typed `UIControl` rather than `UIButton` because a
        /// component may click-attach any control
        /// ([`Self::attach_component`]); the sender is never read, and an
        /// object argument's encoding is the same either way.
        #[unsafe(method(handleClick:))]
        fn handle_click(&self, _sender: &UIControl) {
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

        /// `Segmented`'s `ValueChanged` action → [`EVENT_KIND_SELECTION`].
        /// Reads the requested segment straight off the sender's
        /// `selectedSegmentIndex` and packs it with [`pack_index`] — the
        /// signed `NSInteger` verbatim, so a `UISegmentedControlNoSegment`
        /// (`-1`) report survives the wire and decodes to no event
        /// (`crate::controls::segmented::decode_event`). Every firing is a
        /// genuine user tap (module doc's *No echo guard*).
        #[unsafe(method(handleSegmentValueChanged:))]
        fn handle_segment_value_changed(&self, sender: &UISegmentedControl) {
            let detail = pack_index(sender.selectedSegmentIndex());
            self.dispatch(EVENT_KIND_SELECTION, detail, "handleSegmentValueChanged:");
        }

        /// `Stepper`'s `ValueChanged` action → [`EVENT_KIND_VALUE_CHANGED`].
        ///
        /// `sender.value()` is already **platform-space**, the identical
        /// shape [`Self::handle_slider_value_changed`] documents
        /// (`crate::controls::stepper`'s module doc's *The `[0, span]`
        /// mapping, exactly as `Slider`'s*): `create` pins `minimumValue` at
        /// `0` and `maximumValue` at the app's span, so no `min` of its own;
        /// `crate::controls::stepper::decode_event` adds it back identically
        /// on both Apple arms. A genuinely separate action from the slider's
        /// (module doc's top-level note) because `UIStepper.value` returns a
        /// `double`, not the `float` `UISlider.value` returns — reusing the
        /// slider's binding would read the wrong return width. `from_user` is
        /// unconditionally `true` for the same reason
        /// [`Self::handle_slider_value_changed`] gives: UIKit never sends
        /// `ValueChanged` for a programmatic `setValue:`.
        #[unsafe(method(handleStepperValueChanged:))]
        fn handle_stepper_value_changed(&self, sender: &UIStepper) {
            let detail = pack_value_changed(sender.value().round() as i32, true);
            self.dispatch(
                EVENT_KIND_VALUE_CHANGED,
                detail,
                "handleStepperValueChanged:",
            );
        }

        /// `DatePicker`'s `ValueChanged` action → [`EVENT_KIND_DATE`].
        ///
        /// Reads the requested date straight off the sender's `date`,
        /// converts it to a civil date through the shared Gregorian mapping
        /// (`crate::controls::date_picker::foundation`, the one the control's
        /// own setters use) and packs it with [`pack_date`]. A date outside
        /// the representable range (a BC era, a year past 9999) has no
        /// civil date to report and is dropped with a debug line rather than
        /// fabricated. Every firing is a genuine user pick (module doc's *No
        /// echo guard*).
        #[unsafe(method(handleDateValueChanged:))]
        fn handle_date_value_changed(&self, sender: &UIDatePicker) {
            let Some(date) = civil_date(&sender.date()) else {
                log::debug!(
                    "frust-native-widgets: slot {} picked a date outside 0001-9999 — dropped",
                    self.slot()
                );
                return;
            };
            self.dispatch(EVENT_KIND_DATE, pack_date(date), "handleDateValueChanged:");
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

    /// Build a target for `slot` and wire it as `view`'s `ValueChanged`
    /// action — `Segmented`'s whole selection-attach (Apple-arm-only control,
    /// `crate::controls::APPLE_KINDS`).
    pub(crate) fn attach_segmented(
        mtm: MainThreadMarker,
        slot: SlotId,
        view: &UISegmentedControl,
    ) -> Retained<Self> {
        let target = Self::new(mtm, slot);
        target.add(
            view,
            sel!(handleSegmentValueChanged:),
            UIControlEvents::ValueChanged,
        );
        target
    }

    /// [`Self::attach_segmented`]'s inverse — called from `Segmented::dispose`.
    pub(crate) fn detach_segmented(&self, view: &UISegmentedControl) {
        self.remove(
            view,
            sel!(handleSegmentValueChanged:),
            UIControlEvents::ValueChanged,
        );
    }

    /// Build a target for `slot` and wire it as `view`'s `ValueChanged`
    /// action — `Stepper`'s whole value-attach (Apple-arm-only control,
    /// `crate::controls::APPLE_KINDS`). No drag-edge pair, unlike
    /// [`Self::attach_slider`]: a stepper has no drag gesture to report
    /// (`crate::controls::stepper`'s module doc's *No drag events*).
    pub(crate) fn attach_stepper(
        mtm: MainThreadMarker,
        slot: SlotId,
        view: &UIStepper,
    ) -> Retained<Self> {
        let target = Self::new(mtm, slot);
        target.add(
            view,
            sel!(handleStepperValueChanged:),
            UIControlEvents::ValueChanged,
        );
        target
    }

    /// [`Self::attach_stepper`]'s inverse — called from `Stepper::dispose`.
    pub(crate) fn detach_stepper(&self, view: &UIStepper) {
        self.remove(
            view,
            sel!(handleStepperValueChanged:),
            UIControlEvents::ValueChanged,
        );
    }

    /// Build a target for `slot` and wire it as `view`'s `ValueChanged`
    /// action — `DatePicker`'s whole date-attach (a shared control; its
    /// Android twin is `FrustNativeListener`'s `OnDateChangedListener`).
    pub(crate) fn attach_date_picker(
        mtm: MainThreadMarker,
        slot: SlotId,
        view: &UIDatePicker,
    ) -> Retained<Self> {
        let target = Self::new(mtm, slot);
        target.add(
            view,
            sel!(handleDateValueChanged:),
            UIControlEvents::ValueChanged,
        );
        target
    }

    /// [`Self::attach_date_picker`]'s inverse — called from
    /// `DatePicker::dispose`.
    pub(crate) fn detach_date_picker(&self, view: &UIDatePicker) {
        self.remove(
            view,
            sel!(handleDateValueChanged:),
            UIControlEvents::ValueChanged,
        );
    }

    /// Build a target for `slot` and wire it onto `view` for whichever of the
    /// three families are asked — the public `NativeComponent` path's attach
    /// (`crate::component::ComponentCtx::attach_listener`), which supplies
    /// `slot` from the component's context so the component never sees it.
    ///
    /// Each family is wired exactly as the matching built-in control wires
    /// itself: `click` → [`Self::attach_button`]'s `TouchUpInside`,
    /// `toggled` → [`Self::attach_switch`]'s `ValueChanged`,
    /// `value_changed` → [`Self::attach_slider`]'s three pairs.
    ///
    /// `view` is checked **before** anything is attached, because two of the
    /// action methods read their payload straight off the sender: `toggled`
    /// needs a `UISwitch` (`isOn`) and `value_changed` a `UISlider`
    /// (`value`), and `click` needs at least a `UIControl` to add a target
    /// to. A mismatch answers `Err` naming it, with nothing wired.
    ///
    /// Answers the target (the caller's to retain — targets are held weakly)
    /// and the control, retained so the caller can detach later
    /// ([`Self::detach_component`]).
    #[cfg(feature = "frust-api")]
    pub(crate) fn attach_component(
        mtm: MainThreadMarker,
        slot: SlotId,
        view: &UIView,
        click: bool,
        toggled: bool,
        value_changed: bool,
    ) -> Result<(Retained<Self>, Retained<UIControl>), String> {
        use objc2::Message as _;

        let any: &AnyObject = view;
        let control = any.downcast_ref::<UIControl>().ok_or_else(|| {
            "iOS attach_listener: the view is not a UIControl, so no target-action can be \
             attached to it"
                .to_string()
        })?;
        if toggled && any.downcast_ref::<UISwitch>().is_none() {
            return Err(
                "iOS attach_listener: TOGGLED reads a UISwitch's isOn, and this control is no \
                 UISwitch"
                    .into(),
            );
        }
        if value_changed && any.downcast_ref::<UISlider>().is_none() {
            return Err(
                "iOS attach_listener: VALUE_CHANGED reads a UISlider's value, and this control \
                 is no UISlider"
                    .into(),
            );
        }
        let target = Self::new(mtm, slot);
        target.wire(control, click, toggled, value_changed, true);
        Ok((target, control.retain()))
    }

    /// [`Self::attach_component`]'s inverse: remove every target-action pair
    /// it added for the same families. Removing a pair that is not there is a
    /// documented UIKit no-op, so this is safe to run twice.
    #[cfg(feature = "frust-api")]
    pub(crate) fn detach_component(
        &self,
        control: &UIControl,
        click: bool,
        toggled: bool,
        value_changed: bool,
    ) {
        self.wire(control, click, toggled, value_changed, false);
    }

    /// Add (`attach`) or remove every `(selector, events)` pair the requested
    /// families mean — the one table [`Self::attach_component`] and
    /// [`Self::detach_component`] share, so the two can never disagree.
    #[cfg(feature = "frust-api")]
    fn wire(
        &self,
        control: &UIControl,
        click: bool,
        toggled: bool,
        value_changed: bool,
        attach: bool,
    ) {
        let mut pairs: Vec<(Sel, UIControlEvents)> = Vec::with_capacity(5);
        if click {
            pairs.push((sel!(handleClick:), UIControlEvents::TouchUpInside));
        }
        if toggled {
            pairs.push((
                sel!(handleSwitchValueChanged:),
                UIControlEvents::ValueChanged,
            ));
        }
        if value_changed {
            pairs.push((
                sel!(handleSliderValueChanged:),
                UIControlEvents::ValueChanged,
            ));
            pairs.push((sel!(handleSliderDragStart:), UIControlEvents::TouchDown));
            pairs.push((
                sel!(handleSliderDragEnd:),
                UIControlEvents::TouchUpInside | UIControlEvents::TouchUpOutside,
            ));
        }
        for (action, events) in pairs {
            if attach {
                self.add(control, action, events);
            } else {
                self.remove(control, action, events);
            }
        }
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
