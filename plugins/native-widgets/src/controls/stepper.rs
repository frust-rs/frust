//! `Stepper` — a real increment/decrement control, built and driven from
//! Rust: `UIStepper` on iOS/iPadOS, `NSStepper` on macOS. **Apple-arm-only**:
//! the second control in [`super::APPLE_KINDS`], beside `Segmented`.
//!
//! Seven properties: value, min, max, step, wraps, enabled, and the
//! accessibility label — every one [`Tier::Cheap`](super::Tier::Cheap), the
//! same shape [`super::segmented`]'s module doc records for its own
//! Apple-only setters (no Android measurement exists; a stepper's `-`/`+`
//! buttons never re-measure the control).
//!
//! # No Android arm (the same shape as decision D2)
//!
//! `android.widget` has no stepper/increment control — the framework's
//! numeric-input widgets are `SeekBar` (already `native_slider`) and
//! `NumberPicker` (a scrolling wheel, a different interaction entirely), and
//! this plugin never assumes an AndroidX/Material dependency the app did not
//! add (`segmented.rs`'s module doc makes the identical argument for
//! `MaterialButtonToggleGroup`). So this module has no
//! `#[cfg(target_os = "android")] mod platform`, `crate::android::
//! register_controls` does not register [`KIND`] (the kind sits in
//! [`super::APPLE_KINDS`], not [`super::SHARED_KINDS`]), and the app-facing
//! builder (`crate::api::builders::NativeStepperView`) renders the existing
//! frust-drawn refusal banner **at compile time** on every non-Apple target
//! instead of publishing a slot no factory could serve. A composite
//! Android arm (two `ImageButton`s plus a `TextView`) is a follow-up plan,
//! not this control's v1.
//!
//! # The `[0, span]` mapping, exactly as `Slider`'s
//!
//! Both `UIStepper.minimumValue`/`maximumValue` and `NSStepper.minValue`/
//! `maxValue` are real, native properties — unlike pre-API-26 `SeekBar`,
//! this control has no platform reason to avoid carrying the app's own
//! `min` natively. It deliberately does so anyway: `Stepper` reuses
//! [`slider`](super::slider)'s exact wire shape — [`Setter::Max`] as the
//! *span* (`max - min`) and [`Setter::Progress`] as the *offset* value,
//! [`decode_event`] adding `min` back on the way out — so it rides the same
//! [`EVENT_KIND_VALUE_CHANGED`]/[`pack_value_changed`]/[`unpack_value_changed`]
//! codec, the same write-back/drift mechanics, and the same two already-planned
//! Apple `Setter` variants, rather than a second, parallel range convention
//! for one more control. [`Setter::Max`] is always planned before
//! [`Setter::Progress`], for the same clamping reason [`slider`](super::slider)'s
//! module doc gives.
//!
//! # `step`/`wraps` ride their own wire keys and setters
//!
//! Unlike `value`/`min`/`max`, a step size has no `min`-offset to undo (a
//! delta is invariant to where the range starts), so [`super::STEP`] crosses
//! the wire as a plain integer and [`Setter::Step`] applies it verbatim.
//! [`super::WRAPS`]/[`Setter::Wraps`] is the same shape, for the boolean
//! wrap-at-the-bounds flag.
//!
//! # Range and step invariants: UIKit's contract, enforced once
//!
//! `UIStepper`'s reference (Apple's UIKit documentation for `UIStepper`)
//! documents `maximumValue` as needing to be "numerically greater than the
//! value of the minimumValue property" and `stepValue` as needing to be
//! "greater than 0" — setting either otherwise raises an
//! `NSInvalidArgumentException`, and an ObjC exception crossing `objc2`'s
//! `msg_send!` aborts the process. `NSStepper` enforces neither at the
//! Cocoa layer, so the fix lives in the shared props layer both Apple arms
//! plan from, not in either `platform` module alone:
//!
//! - **[`StepperProps::decode`] normalizes `step` to `>= 1` once.** A
//!   decoded `step <= 0` is logged (naming the slot and the offending
//!   value) and treated as `1`.
//! - **[`StepperProps::span`] can never return less than `1`.** A
//!   degenerate app range (`max <= min`) still reports a platform span of
//!   `1`, never `0` — `0` is exactly as out-of-contract as a negative span.
//! - **[`StepperProps::plan`] derives the platform-facing `enabled` flag as
//!   `props.enabled && max > min`**, never overwriting the app's own
//!   [`StepperProps::enabled`] field, so a degenerate range disables the
//!   control outright: the user can never tap it, and no value outside the
//!   app's own `[min, max]` is ever reported. A later update that makes the
//!   range non-degenerate again re-enables the control and re-asserts
//!   [`Setter::Max`]/[`Setter::Progress`]/[`Setter::Step`] unconditionally,
//!   rather than relying on the values themselves having also changed.
//!
//! Both Apple `apply` functions additionally `debug_assert!` the contract
//! at the boundary, on top of — never instead of — the decode-time
//! normalization, so a future regression in the shared layer still trips
//! in a debug build before it can reach `UIStepper`.
//!
//! # Controlled, exactly like `Slider`
//!
//! A tap on either button reports a *requested* value; `update` writes the
//! app-confirmed one back, with the same two seams `slider.rs`'s module doc
//! describes (`observed` write-back + `crate::runtime::with_runtime`'s
//! re-entrancy drop as the sole echo guard) and the same v1 limitation.
//!
//! # No drag events
//!
//! Unlike `Slider`, a stepper has no drag gesture to report —
//! [`EVENT_KIND_VALUE_CHANGED`] is the only kind [`decode_event`] ever
//! decodes for this control. A held button autorepeats on both platforms
//! (`UIStepper.autorepeat`/`NSStepper.autorepeat`, both already the
//! platform's own default and not exposed as a builder knob in v1), which
//! surfaces as a stream of ordinary value-changed events, not a distinct
//! gesture-phase pair.
//!
//! # Tint: `UIStepper` only, `NSStepper` documented no-op
//!
//! `UIStepper` inherits `UIView.tintColor`, which recolours its `-`/`+`
//! glyphs and border — folded from the theme's `accent_ink`
//! (`crate::api::theme`'s mapping table). `NSStepper` exposes no tint
//! property at all: [`Setter::StepperTint`]'s macOS `apply` reaches
//! `crate::controls::platform::set_tint`'s generic "this view has no tint
//! property" branch, logged at debug and no-op'd — the same documented-gap
//! shape as `NSSlider`'s missing thumb colour (`slider.rs`'s macOS module
//! doc references the same function).
//!
//! [`EVENT_KIND_VALUE_CHANGED`]: crate::events::EVENT_KIND_VALUE_CHANGED
//! [`pack_value_changed`]: crate::events::pack_value_changed
//! [`unpack_value_changed`]: crate::events::unpack_value_changed

use super::{
    CONTENT_DESCRIPTION, ENABLED, MAX, MIN, Plan, STEP, Setter, TINT, VALUE, WRAPS, color,
    owned_text, slot_of,
};
use crate::NativeWidgetError;
use crate::events::{EVENT_KIND_VALUE_CHANGED, EventPayload, unpack_value_changed};
use crate::registry::SlotId;
use crate::runtime::{NativeEvent, Params};

/// The registered kind string the api layer injects as `__frustControl`.
pub(crate) const KIND: &str = "stepper";

/// The platform's own default range ceiling for a fresh `UIStepper`
/// (Apple's documented default: `maximumValue` is `100.0`) — reused as this
/// control's own construction-time-normalization baseline on both Apple
/// arms, the same role [`super::slider::PLATFORM_DEFAULT_MAX`] plays for
/// `Slider`.
pub(crate) const PLATFORM_DEFAULT_MAX: i32 = 100;

/// The platform's own default step size (`UIStepper.stepValue`/
/// `NSStepper.increment`, both documented as `1.0`).
pub(crate) const PLATFORM_DEFAULT_STEP: i32 = 1;

/// The marker type registered under [`KIND`]; its
/// [`NativeWidget`](crate::runtime::NativeWidget) impl is each Apple arm
/// below.
pub(crate) struct Stepper;

/// Everything a `Stepper` slot can be told, as one Rust-diffed value.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StepperProps {
    /// The differ's slot id — **not a property**; `create` needs it for the
    /// target it attaches.
    pub(crate) slot: SlotId,
    /// The app-owned position, in the app's own `[min, max]` (controlled).
    pub(crate) value: i32,
    /// The app-space range floor. Mapped away — see the module doc.
    pub(crate) min: i32,
    /// The app-space range ceiling.
    pub(crate) max: i32,
    /// The increment a tap on either button applies — a plain delta, never
    /// offset by [`Self::min`] (module doc's *`step`/`wraps` ride their own
    /// wire keys*).
    pub(crate) step: i32,
    /// Whether the value wraps from `max` back to `min` (and back) instead
    /// of clamping at the bounds.
    pub(crate) wraps: bool,
    /// `UIControl.enabled` / `NSControl.enabled`.
    pub(crate) enabled: bool,
    /// Packed ARGB tint, or `None` for the platform's own — `UIStepper` only
    /// (module doc's *Tint*).
    pub(crate) tint: Option<i32>,
    /// The VoiceOver label.
    pub(crate) content_description: Option<String>,
}

impl StepperProps {
    /// The state a freshly constructed control is in once its arm's
    /// `create` has normalized it: `[0, 100]` at `0`, a step of `1`, no
    /// wrap, enabled, platform tint, no label — the same numbers
    /// `UIStepper`'s own documented defaults already carry (module doc's
    /// *The `[0, span]` mapping*), pinned explicitly rather than relied
    /// upon (`slider.rs`'s Apple arms' own *Construction-time
    /// normalization* precedent).
    pub(crate) fn platform_default(slot: SlotId) -> Self {
        Self {
            slot,
            value: 0,
            min: 0,
            max: PLATFORM_DEFAULT_MAX,
            step: PLATFORM_DEFAULT_STEP,
            wraps: false,
            enabled: true,
            tint: None,
            content_description: None,
        }
    }

    /// Decode a `Stepper` slot's params — module doc's *Range and step
    /// invariants* for the `step` normalization.
    ///
    /// # Errors
    /// [`NativeWidgetError::Params`] when the reserved identity keys are
    /// missing.
    pub(crate) fn decode(params: &Params<'_>) -> Result<Self, NativeWidgetError> {
        let slot = slot_of(params)?;
        let step = match int_or(params, STEP, PLATFORM_DEFAULT_STEP) {
            step if step >= 1 => step,
            non_positive => {
                log::warn!(
                    "frust-native-widgets: stepper slot {slot} decoded a non-positive step \
                     ({non_positive}) — UIStepper/NSStepper require step > 0; using 1 instead"
                );
                1
            }
        };
        Ok(Self {
            slot,
            value: int_or(params, VALUE, 0),
            min: int_or(params, MIN, 0),
            max: int_or(params, MAX, PLATFORM_DEFAULT_MAX),
            step,
            wraps: params.flag(WRAPS).unwrap_or(false),
            enabled: params.flag(ENABLED).unwrap_or(true),
            tint: color(params, TINT),
            content_description: owned_text(params, CONTENT_DESCRIPTION),
        })
    }

    /// The platform-space span (`max - min`, never less than `1`) this
    /// control hands `setMaximumValue:`/`setMaxValue:` —
    /// [`slider::SliderProps::span`](super::slider::SliderProps::span)'s
    /// shape, floored at `1` rather than `0` (module doc's *Range and step
    /// invariants*: `UIStepper`/`NSStepper` both require a strictly
    /// positive span).
    pub(crate) fn span(&self) -> i32 {
        self.max.saturating_sub(self.min).max(1)
    }

    /// Whether the platform control should accept input: the app's own
    /// [`Self::enabled`] folded with the range being non-degenerate
    /// (module doc's *Range and step invariants*) — a degenerate
    /// `[min, max]` disables the control regardless of what the app asked
    /// for, so [`Self::enabled`] itself is never overwritten by this
    /// derivation.
    pub(crate) fn effective_enabled(&self) -> bool {
        self.enabled && self.max > self.min
    }

    /// The platform-space value (`value - min`, clamped into the range) this
    /// control hands `setValue:`/`setDoubleValue:` —
    /// [`slider::SliderProps::progress`](super::slider::SliderProps::progress),
    /// verbatim.
    pub(crate) fn platform_value(&self) -> i32 {
        self.value
            .clamp(self.min, self.max.max(self.min))
            .saturating_sub(self.min)
    }

    /// The setter-call plan for `old` → `new`, given the platform-space
    /// value the platform last reported (`observed` — the value-changed
    /// callback's write, via [`decode_event`]), in the module doc's
    /// mandatory max-before-value order.
    pub(crate) fn plan<'a>(old: &Self, new: &'a Self, observed: Option<i32>) -> Plan<'a> {
        let mut plan = Plan::new();
        let old_enabled = old.effective_enabled();
        let new_enabled = new.effective_enabled();
        // A degenerate range never let a value/step the platform could
        // trust through while disabled; re-enabling reasserts all three
        // unconditionally rather than relying on them having also changed
        // (module doc's *Range and step invariants*).
        let reenabling = !old_enabled && new_enabled;
        if old.span() != new.span() || reenabling {
            plan.push(Setter::Max(new.span()));
        }
        let drifted = observed.is_some_and(|platform| platform != new.platform_value());
        if old.platform_value() != new.platform_value() || drifted || reenabling {
            plan.push(Setter::Progress(new.platform_value()));
        }
        if old.step != new.step || reenabling {
            plan.push(Setter::Step(new.step));
        }
        if old.wraps != new.wraps {
            plan.push(Setter::Wraps(new.wraps));
        }
        if old_enabled != new_enabled {
            plan.push(Setter::Enabled(new_enabled));
        }
        if old.tint != new.tint {
            plan.push(Setter::StepperTint(new.tint));
        }
        if old.content_description != new.content_description {
            plan.push(Setter::ContentDescription(
                new.content_description.as_deref(),
            ));
        }
        plan
    }
}

/// An integer field with a documented fallback — [`slider::int_or`]'s
/// private twin (`value`/`min`/`max`/`step` all have a meaningful platform
/// default, so a missing one is never an error). Duplicated rather than
/// shared: `slider`'s own copy is private to that module, matching this
/// crate's per-control decode-helper shape.
fn int_or(params: &Params<'_>, key: &str, fallback: i32) -> i32 {
    params
        .int(key)
        .and_then(|raw| i32::try_from(raw).ok())
        .unwrap_or(fallback)
}

/// Decode a value-changed callback firing into the typed vocabulary: a
/// value change reports **platform-space** (module doc's *The `[0, span]`
/// mapping*) until `min` is added back here — [`slider::decode_event`]'s
/// value arm, verbatim, minus the drag-edge arms `Stepper` has no gesture
/// to report.
///
/// Pure and host-testable: no objc2, no `StepperState` — each Apple arm's
/// `on_event` is a two-field forward onto this.
pub(crate) fn decode_event(
    observed: &mut Option<i32>,
    min: i32,
    event: NativeEvent,
) -> Option<EventPayload> {
    match event.kind {
        EVENT_KIND_VALUE_CHANGED => {
            let (platform_value, from_user) = unpack_value_changed(event.detail);
            *observed = Some(platform_value);
            Some(EventPayload::ValueChanged {
                value: platform_value + min,
                from_user,
            })
        }
        _ => None,
    }
}

#[cfg(target_os = "ios")]
pub(crate) mod platform {
    //! The iOS half: build the `UIStepper` and apply the same planned
    //! setters `slider.rs`'s iOS half does, plus `Stepper`'s own three
    //! (`Step`/`Wraps`/`StepperTint`) — no echo guard of its own
    //! (`switch.rs`'s module doc is the reference account, `slider.rs`'s
    //! Apple arms cite it the same way).
    //!
    //! # Construction-time normalization
    //!
    //! A fresh `UIStepper` is already `[0.0, 100.0]` at `0.0` with a
    //! `stepValue` of `1.0` — the exact numbers
    //! [`StepperProps::platform_default`] describes — so this normalization
    //! is technically redundant with Apple's own documented defaults.
    //! `create` pins it explicitly anyway, matching `slider.rs`'s Apple
    //! arms' belt-and-braces shape: a diffed create plan must never depend
    //! on an undocumented-to-stay-that-way platform default, only on the
    //! shared baseline it diffs against.

    use objc2::MainThreadMarker;
    use objc2::rc::Retained;
    use objc2_ui_kit::UIStepper;

    use super::{KIND, Stepper, StepperProps, decode_event};
    use crate::NativeWidgetError;
    use crate::apple::{FrustNativeControlTarget, NativeCtx, NativeView};
    use crate::controls::platform;
    use crate::controls::{Plan, Setter};
    use crate::events::EventPayload;
    use crate::runtime::{NativeEvent, NativeWidget, Params};

    /// A live stepper's retained state — `slider.rs`'s `SliderState` shape.
    pub(crate) struct StepperState {
        view: Retained<UIStepper>,
        /// The target-action object `create` attached as `view`'s
        /// `ValueChanged` action. `UIControl` holds it weakly, so this
        /// field is the only thing keeping it alive for the slot's
        /// lifetime (`crate::apple::events`'s module doc's *Target
        /// retention*).
        target: Retained<FrustNativeControlTarget>,
        /// The app-space range floor as of the last applied props —
        /// `on_event` is never handed `Props`, and [`decode_event`] needs
        /// `min` to map a platform-space report back to app space.
        min: i32,
        /// The **platform-space** value the platform last reported, or
        /// `None` while untouched — [`StepperProps::plan`]'s write-back
        /// drift signal. Written from [`NativeWidget::on_event`] via
        /// [`decode_event`].
        observed: Option<i32>,
    }

    impl NativeWidget for Stepper {
        type Props = StepperProps;
        type State = StepperState;

        fn decode_props(params: &Params<'_>) -> Result<Self::Props, NativeWidgetError> {
            StepperProps::decode(params)
        }

        fn create(
            ctx: &mut NativeCtx<'_, '_>,
            props: &Self::Props,
        ) -> Result<(NativeView, Self::State), NativeWidgetError> {
            let mtm = ctx.mtm();
            let view = UIStepper::new(mtm);
            let default = StepperProps::platform_default(props.slot);
            // Module doc's *Construction-time normalization*.
            view.setMinimumValue(0.0);
            view.setMaximumValue(f64::from(default.span()));
            view.setStepValue(f64::from(default.step));
            view.setValue(f64::from(default.platform_value()));
            view.setWraps(default.wraps);
            let plan = StepperProps::plan(&default, props, None);
            apply_all(mtm, &view, &plan);
            // Attached after the initial plan and the construction-time
            // normalization above, matching `slider.rs`'s create order.
            let target = FrustNativeControlTarget::attach_stepper(mtm, props.slot, &view);
            let handle = NativeView::new(Retained::clone(&view).into_super().into_super(), mtm);
            Ok((
                handle,
                StepperState {
                    view,
                    target,
                    min: props.min,
                    observed: None,
                },
            ))
        }

        fn update(
            ctx: &mut NativeCtx<'_, '_>,
            state: &mut Self::State,
            old: &Self::Props,
            new: &Self::Props,
        ) -> Result<(), NativeWidgetError> {
            let plan = StepperProps::plan(old, new, state.observed);
            apply_all(ctx.mtm(), &state.view, &plan);
            // Cleared unconditionally — see `switch.rs`'s Apple `update` for
            // why this arm has no `is_ok()` gate; `slider.rs`'s Apple arms
            // do the same.
            state.observed = None;
            state.min = new.min;
            Ok(())
        }

        fn on_event(state: &mut Self::State, event: NativeEvent) -> Option<EventPayload> {
            decode_event(&mut state.observed, state.min, event)
        }

        fn dispose(
            _ctx: &mut NativeCtx<'_, '_>,
            state: Self::State,
        ) -> Result<(), NativeWidgetError> {
            // Detach so a stray in-flight tap can't fire after this slot's
            // instance is gone, mirroring the other controls' explicit
            // detach; dropping `state` afterwards releases the target's own
            // retain.
            state.target.detach_stepper(&state.view);
            Ok(())
        }
    }

    /// Execute a whole [`Plan`], front to back — max before value, the order
    /// the shared plan already guarantees (module doc).
    fn apply_all(mtm: MainThreadMarker, view: &UIStepper, plan: &Plan<'_>) {
        for setter in plan {
            apply(mtm, view, setter);
        }
    }

    /// Execute one planned property write against `view`.
    fn apply(mtm: MainThreadMarker, view: &UIStepper, setter: &Setter<'_>) {
        match *setter {
            Setter::Max(span) => {
                // Belt-and-braces on top of `StepperProps::span` (module
                // doc's *Range and step invariants*): `UIStepper` aborts the
                // process if `maximumValue` isn't strictly greater than
                // `minimumValue` (always `0.0` here).
                debug_assert!(span >= 1, "UIStepper requires maximumValue > minimumValue");
                view.setMaximumValue(f64::from(span));
            }
            Setter::Progress(value) => view.setValue(f64::from(value)),
            Setter::Step(step) => {
                // Belt-and-braces on top of `StepperProps::decode` (module
                // doc's *Range and step invariants*): `UIStepper` aborts the
                // process if `stepValue` isn't strictly positive.
                debug_assert!(step >= 1, "UIStepper requires stepValue > 0");
                view.setStepValue(f64::from(step));
            }
            Setter::Wraps(wraps) => view.setWraps(wraps),
            Setter::Enabled(enabled) => view.setEnabled(enabled),
            Setter::StepperTint(argb) => set_tint_color(view, argb),
            Setter::ContentDescription(label) => {
                platform::set_accessibility_label(view, label, mtm);
            }
            ref other => platform::warn_unexpected_setter(KIND, other),
        }
    }

    /// `UIView.tintColor` (module doc's *Tint*) — [`Setter::StepperTint`].
    fn set_tint_color(view: &UIStepper, argb: Option<i32>) {
        let color = platform::optional_ui_color(argb);
        // SAFETY: objc2 marks `setTintColor:` unsafe only because the
        // header leaves the argument's nullability unannotated; passing
        // `None` is `UIView`'s own documented "restore the inherited tint"
        // behaviour, exactly `Setter::StepperTint`'s `None`-clears meaning
        // (the same closed question `crate::controls::platform::
        // set_image_tint` documents for `UIImageView.tintColor`).
        unsafe { view.setTintColor(color.as_deref()) };
    }
}

#[cfg(target_os = "macos")]
pub(crate) mod platform {
    //! The macOS half: build an `NSStepper` and apply the same planned
    //! setters the iOS half does — controlled exactly like `Slider`, with no
    //! echo guard of its own (`switch.rs`'s module doc, *And on macOS*).
    //!
    //! # Construction-time normalization
    //!
    //! A fresh `NSStepper`'s own documented defaults are not pinned the way
    //! `UIStepper`'s are, so `create` normalizes it explicitly before
    //! running the shared create plan — `slider.rs`'s macOS arm's exact
    //! shape (a span of exactly [`super::PLATFORM_DEFAULT_MAX`] plans no
    //! [`Setter::Max`] at all, so relying on an unconfirmed native default
    //! would risk silently keeping it).
    //!
    //! # Tint: none
    //!
    //! `NSStepper` exposes no tint property at all — module doc's *Tint*
    //! section: [`Setter::StepperTint`]'s `apply` reaches
    //! `crate::controls::platform::set_tint`'s generic "this view has no
    //! tint property" branch (logged at debug, not a failure), the same
    //! shape as `NSSlider`'s missing thumb colour.

    use objc2::rc::Retained;
    use objc2_app_kit::NSStepper;

    use super::{KIND, Stepper, StepperProps, decode_event};
    use crate::NativeWidgetError;
    use crate::appkit::{FrustNativeControlTarget, NativeCtx, NativeView};
    use crate::controls::platform;
    use crate::controls::{Plan, Setter};
    use crate::events::{EVENT_KIND_VALUE_CHANGED, EventPayload};
    use crate::runtime::{NativeEvent, NativeWidget, Params};

    /// A live stepper's retained state — the iOS arm's shape.
    pub(crate) struct StepperState {
        /// The stepper, kept typed for its own range/value setters.
        view: Retained<NSStepper>,
        /// The target `create` attached — its only retain (`NSControl.target`
        /// is weak; `crate::appkit::events`'s *Target retention*).
        target: Retained<FrustNativeControlTarget>,
        /// The app-space range floor as of the last applied props —
        /// `on_event` is never handed `Props`, and [`decode_event`] needs
        /// `min` to map a platform-space report back to app space.
        min: i32,
        /// The **platform-space** value the platform last reported, or
        /// `None` while untouched — [`StepperProps::plan`]'s write-back
        /// drift signal. Written from [`NativeWidget::on_event`] via
        /// [`decode_event`].
        observed: Option<i32>,
    }

    impl NativeWidget for Stepper {
        type Props = StepperProps;
        type State = StepperState;

        fn decode_props(params: &Params<'_>) -> Result<Self::Props, NativeWidgetError> {
            StepperProps::decode(params)
        }

        fn create(
            ctx: &mut NativeCtx<'_, '_>,
            props: &Self::Props,
        ) -> Result<(NativeView, Self::State), NativeWidgetError> {
            let mtm = ctx.mtm();
            let view = NSStepper::new(mtm);
            let default = StepperProps::platform_default(props.slot);
            // Module doc's *Construction-time normalization*.
            view.setMinValue(0.0);
            view.setMaxValue(f64::from(default.span()));
            view.setIncrement(f64::from(default.step));
            view.setValueWraps(default.wraps);
            view.setDoubleValue(f64::from(default.platform_value()));
            let plan = StepperProps::plan(&default, props, None);
            apply_all(&view, &plan);
            // Attached after the normalization and the initial plan,
            // matching the other controls' create order.
            let target =
                FrustNativeControlTarget::attach(mtm, &view, props.slot, EVENT_KIND_VALUE_CHANGED);
            let handle = NativeView::new(Retained::clone(&view).into_super().into_super(), mtm);
            Ok((
                handle,
                StepperState {
                    view,
                    target,
                    min: props.min,
                    observed: None,
                },
            ))
        }

        fn update(
            _ctx: &mut NativeCtx<'_, '_>,
            state: &mut Self::State,
            old: &Self::Props,
            new: &Self::Props,
        ) -> Result<(), NativeWidgetError> {
            let plan = StepperProps::plan(old, new, state.observed);
            apply_all(&state.view, &plan);
            // Cleared unconditionally — see `switch.rs`'s macOS `update`;
            // `slider.rs`'s macOS arm does the same.
            state.observed = None;
            state.min = new.min;
            Ok(())
        }

        fn on_event(state: &mut Self::State, event: NativeEvent) -> Option<EventPayload> {
            decode_event(&mut state.observed, state.min, event)
        }

        fn dispose(
            _ctx: &mut NativeCtx<'_, '_>,
            state: Self::State,
        ) -> Result<(), NativeWidgetError> {
            // Detach so a stray in-flight tap can't reach a torn-down slot;
            // dropping `state` afterwards releases the target's only retain.
            state.target.detach(&state.view);
            Ok(())
        }
    }

    /// Execute a whole [`Plan`], front to back — max before value, the order
    /// the shared plan already guarantees (module doc, top of file).
    fn apply_all(view: &NSStepper, plan: &Plan<'_>) {
        for setter in plan {
            apply(view, setter);
        }
    }

    /// Execute one planned property write against `view`.
    fn apply(view: &NSStepper, setter: &Setter<'_>) {
        match *setter {
            Setter::Max(span) => {
                // `NSStepper` doesn't throw on a non-positive span the way
                // `UIStepper` does, but the invariant is shared — belt-and-
                // braces on top of `StepperProps::span` (module doc's *Range
                // and step invariants*).
                debug_assert!(span >= 1, "NSStepper's span must stay >= 1");
                view.setMaxValue(f64::from(span));
            }
            Setter::Progress(value) => view.setDoubleValue(f64::from(value)),
            Setter::Step(step) => {
                debug_assert!(step >= 1, "NSStepper's increment must stay >= 1");
                view.setIncrement(f64::from(step));
            }
            Setter::Wraps(wraps) => view.setValueWraps(wraps),
            Setter::Enabled(enabled) => platform::set_enabled(view, enabled),
            Setter::StepperTint(argb) => platform::set_tint(view, "StepperTint", argb),
            Setter::ContentDescription(label) => platform::set_accessibility_label(view, label),
            ref other => platform::warn_unexpected_setter(KIND, other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::with_identity;

    fn decode(body: &str) -> StepperProps {
        let raw = with_identity(KIND, 5, body);
        StepperProps::decode(&Params::new(&raw)).expect("decodes")
    }

    #[test]
    fn absent_fields_decode_to_the_platform_defaults_and_plan_nothing() {
        let props = decode("");
        assert_eq!(props, StepperProps::platform_default(5));
        assert!(StepperProps::plan(&StepperProps::platform_default(5), &props, None).is_empty());
    }

    #[test]
    fn a_non_zero_min_is_mapped_onto_the_platforms_zero_based_range() {
        let props = decode("\"value\":15,\"min\":10,\"max\":20");
        assert_eq!(props.span(), 10);
        assert_eq!(props.platform_value(), 5);
        assert_eq!(
            StepperProps::plan(&StepperProps::platform_default(5), &props, None),
            vec![Setter::Max(10), Setter::Progress(5)],
            "max is always planned before value — the platform clamps"
        );
    }

    #[test]
    fn a_value_outside_the_range_is_clamped_rather_than_rejected() {
        assert_eq!(
            decode("\"value\":99,\"min\":0,\"max\":10").platform_value(),
            10
        );
        assert_eq!(
            decode("\"value\":-5,\"min\":0,\"max\":10").platform_value(),
            0
        );
        // An inverted range degrades to a span of exactly 1 — never 0 or
        // negative, both out of `UIStepper`/`NSStepper`'s contract (module
        // doc's *Range and step invariants*).
        let inverted = decode("\"value\":3,\"min\":10,\"max\":2");
        assert_eq!(inverted.span(), 1);
        assert_eq!(inverted.platform_value(), 0);
    }

    #[test]
    fn step_defaults_to_the_platform_default_and_never_offsets_by_min() {
        let props = decode("\"min\":10,\"max\":20");
        assert_eq!(props.step, PLATFORM_DEFAULT_STEP);
        let stepped = decode("\"min\":10,\"max\":20,\"step\":5");
        assert_eq!(stepped.step, 5, "a step size rides the wire unmapped");
    }

    #[test]
    fn a_non_positive_or_missing_step_never_decodes_below_one() {
        assert_eq!(decode("\"step\":0").step, 1, "zero normalizes to 1");
        assert_eq!(decode("\"step\":-4").step, 1, "negative normalizes to 1");
        assert_eq!(
            decode("").step,
            PLATFORM_DEFAULT_STEP,
            "a missing step falls back to the platform default, not the \
             non-positive branch"
        );
    }

    #[test]
    fn a_degenerate_range_max_equal_min_spans_one_and_disables_the_control() {
        let props = decode("\"value\":5,\"min\":5,\"max\":5");
        assert_eq!(props.span(), 1, "UIStepper requires max > min");
        assert_eq!(props.platform_value(), 0);
        assert!(
            !props.effective_enabled(),
            "a single-point range can never be stepped"
        );
    }

    #[test]
    fn a_degenerate_range_max_less_than_min_spans_one_and_disables_the_control() {
        let props = decode("\"value\":5,\"min\":10,\"max\":2");
        assert_eq!(props.span(), 1);
        assert_eq!(props.platform_value(), 0);
        assert!(!props.effective_enabled());
    }

    #[test]
    fn the_create_plan_sets_exactly_the_non_default_fields() {
        let props = decode(
            "\"value\":25,\"max\":50,\"step\":5,\"wraps\":true,\"enabled\":false,\"tint\":1,\
             \"contentDescription\":\"count\"",
        );
        assert_eq!(
            StepperProps::plan(&StepperProps::platform_default(5), &props, None),
            vec![
                Setter::Max(50),
                Setter::Progress(25),
                Setter::Step(5),
                Setter::Wraps(true),
                Setter::Enabled(false),
                Setter::StepperTint(Some(1)),
                Setter::ContentDescription(Some("count")),
            ]
        );
    }

    #[test]
    fn a_tap_the_app_confirmed_writes_the_value_back_once() {
        let before = decode("\"value\":0,\"max\":10");
        let after = decode("\"value\":1,\"max\":10");
        assert_eq!(
            StepperProps::plan(&before, &after, Some(1)),
            vec![Setter::Progress(1)]
        );
    }

    #[test]
    fn a_platform_drift_is_written_back_even_when_the_props_value_did_not_change() {
        let old = decode("\"value\":3,\"max\":10");
        let new = decode("\"value\":3,\"max\":10,\"enabled\":false");
        assert_eq!(
            StepperProps::plan(&old, &new, Some(9)),
            vec![Setter::Progress(3), Setter::Enabled(false)],
            "the app rejected the tap: the confirmed value is re-asserted"
        );
        assert_eq!(
            StepperProps::plan(&old, &new, None),
            vec![Setter::Enabled(false)]
        );
    }

    #[test]
    fn a_range_change_replans_both_range_setters_in_order() {
        let old = decode("\"value\":5,\"min\":0,\"max\":10");
        let new = decode("\"value\":5,\"min\":5,\"max\":25");
        assert_eq!(
            StepperProps::plan(&old, &new, None),
            vec![Setter::Max(20), Setter::Progress(0)]
        );
    }

    #[test]
    fn a_range_becoming_non_degenerate_reenables_and_reasserts_max_progress_step() {
        // The platform-space numbers happen not to change at all (span 1 →
        // span 1, platform-value 0 → 0, step unchanged) — proving the
        // re-enable transition forces the reassertion rather than
        // piggy-backing on some other field having also changed.
        let old = decode("\"value\":5,\"min\":5,\"max\":5,\"step\":3");
        let new = decode("\"value\":5,\"min\":5,\"max\":6,\"step\":3");
        assert_eq!(old.span(), 1);
        assert_eq!(
            new.span(),
            1,
            "the newly valid range still spans exactly one step"
        );
        assert!(!old.effective_enabled());
        assert!(new.effective_enabled());
        assert_eq!(
            StepperProps::plan(&old, &new, None),
            vec![
                Setter::Max(1),
                Setter::Progress(0),
                Setter::Step(3),
                Setter::Enabled(true),
            ],
            "re-enabling must reassert every range setter, not just the ones \
             that changed"
        );
    }

    #[test]
    fn a_range_becoming_degenerate_disables_without_a_forced_reassertion() {
        let old = decode("\"value\":5,\"min\":0,\"max\":10,\"step\":2");
        let new = decode("\"value\":5,\"min\":5,\"max\":5,\"step\":2");
        assert_eq!(
            StepperProps::plan(&old, &new, None),
            vec![Setter::Max(1), Setter::Progress(0), Setter::Enabled(false)],
            "going disabled needs the usual diffed setters, no forced replay"
        );
    }

    #[test]
    fn clearing_the_tint_plans_the_nullable_setter() {
        let tinted = decode("\"tint\":7");
        let plain = decode("");
        assert_eq!(
            StepperProps::plan(&tinted, &plain, None),
            vec![Setter::StepperTint(None)]
        );
    }

    #[test]
    fn every_planned_setter_reports_the_cheap_tier() {
        use crate::controls::Tier;

        let props =
            decode("\"value\":4,\"max\":10,\"step\":2,\"wraps\":true,\"enabled\":false,\"tint\":1");
        for setter in StepperProps::plan(&StepperProps::platform_default(5), &props, None) {
            assert_eq!(setter.tier(), Tier::Cheap, "{setter:?}");
        }
    }

    // --- events / echo guard --------------------------------------------

    fn value_changed_event(platform_value: i32, from_user: bool) -> NativeEvent {
        NativeEvent {
            kind: EVENT_KIND_VALUE_CHANGED,
            detail: crate::events::pack_value_changed(platform_value, from_user),
        }
    }

    #[test]
    fn a_user_tap_decodes_to_app_space_and_updates_the_drift_signal() {
        // Platform-space 5 with an app-space min of 10 is app-space 15.
        let mut observed = None;
        let payload = decode_event(&mut observed, 10, value_changed_event(5, true));
        assert_eq!(
            payload,
            Some(EventPayload::ValueChanged {
                value: 15,
                from_user: true,
            })
        );
        assert_eq!(observed, Some(5), "observed stays platform-space");
    }

    #[test]
    fn a_misrouted_kind_decodes_to_nothing() {
        let mut observed = Some(1);
        let click = NativeEvent {
            kind: crate::events::EVENT_KIND_CLICK,
            detail: 0,
        };
        assert_eq!(decode_event(&mut observed, 0, click), None);
        assert_eq!(observed, Some(1));
    }

    #[test]
    fn drag_edge_kinds_decode_to_nothing_this_control_has_none_to_report() {
        // Unlike `Slider`, `Stepper` never emits (and never decodes) a drag
        // edge — module doc's *No drag events*.
        let mut observed = None;
        let start = NativeEvent {
            kind: crate::events::EVENT_KIND_DRAG_START,
            detail: 0,
        };
        assert_eq!(decode_event(&mut observed, 0, start), None);
        assert_eq!(observed, None);
    }
}
