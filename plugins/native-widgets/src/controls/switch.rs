//! `Switch` — a real `android.widget.Switch`, built and driven from Rust.
//!
//! Five properties: checked, enabled, thumb tint, track tint, accessibility
//! label. Every one of them is [`Tier::Cheap`](super::Tier::Cheap).
//!
//! # No explicit background (f2-04)
//!
//! Unlike `Label`/`ProgressBar`, `SwitchProps` deliberately carries no
//! `background_color` field. `android.widget.Switch`
//! (`Widget.Material.CompoundButton.Switch`) draws its default background
//! from `?attr/selectableItemBackgroundBorderless`, which paints the
//! Material touch ripple; a flat `View.setBackgroundColor` (the fix f1-01
//! shipped, and this followup reverts for this control) replaces that
//! drawable outright, so a themed `Switch` lost its ripple. The thumb/track
//! tints below already carry the theme with no such tradeoff — see
//! `crate::api::theme`'s module doc's *Explicit backgrounds* section for the
//! full account.
//!
//! # The controlled-component contract, and its two seams
//!
//! A `Switch` is **controlled** (`docs/CODE_STANDARDS.md`'s Interaction
//! Semantics): the platform reports the *requested* value through
//! `OnCheckedChangeListener`, and the app-confirmed value comes back
//! down as props. The platform, unlike a frust widget, flips its own visual
//! state the instant the user touches it — so two things have to be true, and
//! both live in this module:
//!
//! 1. **Write-back.** [`SwitchProps::plan`] takes the value the platform last
//!    reported (`observed`). When it disagrees with the app's value, the
//!    checked setter is planned **even though `old.checked == new.checked`** —
//!    otherwise a props change that touched some other field would leave the
//!    platform's optimistic flip standing.
//! 2. **Echo guard.** `setChecked` notifies the listener, so writing the app's
//!    value would otherwise look like a fresh user toggle. `CompoundButton`'s
//!    AOSP source (every supported API level) invokes `onCheckedChanged`
//!    **synchronously, in the same call stack as `setChecked`** — guarded
//!    only by its own reentrancy flag (`mBroadcasting`), never posted to a
//!    `Handler` or deferred to an animation callback; there is no OEM/vendor
//!    override path either, since `Switch` here is always the framework
//!    class (this module's own `CLASS` const), not a themed subclass. `update`
//!    runs inside `crate::runtime::with_runtime`, so that synchronous echo
//!    re-enters the same thread-local `RefCell` mid-borrow, fails
//!    `try_borrow_mut`, and is dropped **before** `NativeWidget::on_event`/
//!    [`decode_toggled`] ever see it (`crate::runtime`'s re-entrancy
//!    tolerance) — this is the SOLE guard. There is no per-instance
//!    suppression flag, and therefore nothing a panic mid-`update` could
//!    leave latched; the re-entrancy mechanism itself is pinned by
//!    `crate::runtime`'s `the_thread_local_runtime_is_reentrancy_tolerant`.
//!    [`decode_toggled`] has nothing left to suppress: every event it is ever
//!    handed in production is a genuine platform report. **This safety is
//!    incidental, not designed**: it holds only because
//!    `crate::runtime::NativeRuntime::update_params` — which calls `update`
//!    above — is itself always invoked from inside `with_runtime`
//!    (`crate::android`'s `nativeUpdateParams`). A future refactor that moved
//!    `update` outside that borrow would silently remove the only echo
//!    protection this crate has.
//!
//! # The same contract on iOS, with NO iOS-specific machinery
//!
//! Seam 1 (write-back) is platform-neutral — it lives in [`SwitchProps::plan`]
//! above, which both arms call. Seam 2 (the echo guard) is where the two
//! platforms genuinely differ, and the difference is that **iOS is expected to
//! have no echo to guard**:
//!
//! - Apple's UIControl guidance is *"As a rule UIKit does not send events when
//!   programmatic changes are made to controls"*, and `UISwitch` states
//!   explicitly of `setOn(_:animated:)` that setting the switch *"does not
//!   result in an action message being sent"*. React Native's iOS switch
//!   relies on exactly this: `RCTSwitchComponentView.mm`'s `updateProps`
//!   applies values through `setOn:animated:` and its `-onChange:` simply does
//!   not fire (`research/RESEARCH-P2-REFRESH.md` §3–§4).
//! - **Caveat A**: Apple documents that guarantee *explicitly* only for
//!   `setOn(_:animated:)` — which is why the Apple arm below calls exactly
//!   that (with `animated: false`, so the visible result matches the plain
//!   property setter) rather than the bare `isOn` setter, whose no-action
//!   behaviour rests on the general UIControl rule alone.
//! - **Caveat B**: a developer-filed report (Apple Developer Forums thread
//!   108027, report #43955023) claims `setOn:` called *from inside* a
//!   `valueChanged` handler can re-enter that handler — which is precisely
//!   this crate's write-back path. Unconfirmed by Apple's own docs, and
//!   therefore not dismissed.
//!
//! So this arm ships **no iOS-specific echo machinery, and no per-instance
//! suppression flag** (f2-02 deleted Android's; there is nothing to
//! reinstate). Should Caveat B ever bite, the protection is *inherited from
//! the shared runtime*, not built here: a re-entrant callback lands in the
//! same `crate::runtime::with_runtime` borrow and is dropped there, exactly as
//! Android's synchronous echo is. Whether it bites at all is **proven on
//! device in task p2-05**, not asserted here.
//!
//! **v1 limitation, deliberate:** an app that *rejects* a toggle (reports the
//! same value back) produces no props change at all, so `update` never runs
//! and the platform's flip stands until the next differing params.
//! `NativeWidget::on_event` is handed no platform context, so a control cannot
//! revert from the event itself; closing this needs an event-time context in
//! the trait, which Phase 3 decides.

use super::{
    CHECKED, CONTENT_DESCRIPTION, ENABLED, Plan, Setter, THUMB_TINT, TRACK_TINT, TYPEFACE, color,
    owned_text, slot_of,
};
use crate::NativeWidgetError;
use crate::controls::typeface::{self, Typeface};
use crate::events::{EVENT_KIND_TOGGLED, EventPayload, unpack_bool};
use crate::registry::SlotId;
use crate::runtime::{NativeEvent, Params};

/// The registered kind string the api layer injects as `__frustControl`.
pub(crate) const KIND: &str = "switch";

/// The marker type registered under [`KIND`]; its
/// [`NativeWidget`](crate::runtime::NativeWidget) impl is the Android half
/// below.
pub(crate) struct Switch;

/// Everything a `Switch` slot can be told, as one Rust-diffed value.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SwitchProps {
    /// The differ's slot id — **not a property**; `create` needs it for the
    /// listener it attaches.
    pub(crate) slot: SlotId,
    /// The app-owned checked state (controlled — see the module doc).
    pub(crate) checked: bool,
    /// `View.setEnabled`.
    pub(crate) enabled: bool,
    /// Packed ARGB thumb tint, or `None` to restore the platform's.
    pub(crate) thumb_tint: Option<i32>,
    /// Packed ARGB track tint, or `None` to restore the platform's.
    pub(crate) track_tint: Option<i32>,
    /// The TalkBack label.
    pub(crate) content_description: Option<String>,
    /// Theme ladder L3 (p1-08): the resolved
    /// [`crate::api::theme::ResolvedTheme::body_typeface`], or
    /// [`Typeface::System`] when absent. `Switch` never sets on/off text
    /// through this plugin today, but it's a `TextView` subclass under the
    /// hood, so this still costs a real (if today invisible) `setTypeface`
    /// call — see `crate::api::theme`'s module doc.
    pub(crate) typeface: Typeface,
}

impl SwitchProps {
    /// The state a freshly constructed `new Switch(context)` is already in.
    pub(crate) fn platform_default(slot: SlotId) -> Self {
        Self {
            slot,
            checked: false,
            enabled: true,
            thumb_tint: None,
            track_tint: None,
            content_description: None,
            typeface: Typeface::System,
        }
    }

    /// Decode a `Switch` slot's params.
    ///
    /// # Errors
    /// [`NativeWidgetError::Params`] when the reserved identity keys are
    /// missing.
    pub(crate) fn decode(params: &Params<'_>) -> Result<Self, NativeWidgetError> {
        Ok(Self {
            slot: slot_of(params)?,
            checked: params.flag(CHECKED).unwrap_or(false),
            enabled: params.flag(ENABLED).unwrap_or(true),
            thumb_tint: color(params, THUMB_TINT),
            track_tint: color(params, TRACK_TINT),
            content_description: owned_text(params, CONTENT_DESCRIPTION),
            typeface: typeface::decode(params, TYPEFACE),
        })
    }

    /// The setter-call plan for `old` → `new`, given the value the platform
    /// last reported through its own listener (`observed`, `None` until the
    /// user has touched the control).
    ///
    /// The checked setter is planned when the app's value changed **or** when
    /// the platform has drifted away from it — the write-back half of the
    /// module doc's controlled-component contract.
    pub(crate) fn plan<'a>(old: &Self, new: &'a Self, observed: Option<bool>) -> Plan<'a> {
        let mut plan = Plan::new();
        let drifted = observed.is_some_and(|platform| platform != new.checked);
        if old.checked != new.checked || drifted {
            plan.push(Setter::Checked(new.checked));
        }
        if old.enabled != new.enabled {
            plan.push(Setter::Enabled(new.enabled));
        }
        if old.thumb_tint != new.thumb_tint {
            plan.push(Setter::ThumbTint(new.thumb_tint));
        }
        if old.track_tint != new.track_tint {
            plan.push(Setter::TrackTint(new.track_tint));
        }
        if old.typeface != new.typeface {
            plan.push(Setter::Typeface(new.typeface));
        }
        if old.content_description != new.content_description {
            plan.push(Setter::ContentDescription(
                new.content_description.as_deref(),
            ));
        }
        plan
    }
}

/// Decode a `CompoundButton.OnCheckedChangeListener` firing into the typed
/// vocabulary: updates the write-back drift signal (`observed`) and always
/// decodes to an [`EventPayload::Toggled`] — a `Toggled` `update`'s own
/// [`Setter::Checked`] echo never reaches this decoder at all in production
/// (module doc's *Echo guard*: `crate::runtime::with_runtime`'s re-entrancy
/// tolerance drops it one layer up, before `NativeWidget::on_event` runs).
///
/// `None` when `event.kind` is not the toggled kind — defensive, since
/// `Switch`'s listener is only ever attached as an
/// `OnCheckedChangeListener` and so can only ever report this one kind.
///
/// Pure and host-testable: no JNI, no `SwitchState` — the Android glue
/// (`platform::Switch`'s `on_event`) is a one-field forward onto this.
pub(crate) fn decode_toggled(
    observed: &mut Option<bool>,
    event: NativeEvent,
) -> Option<EventPayload> {
    if event.kind != EVENT_KIND_TOGGLED {
        return None;
    }
    let checked = unpack_bool(event.detail);
    *observed = Some(checked);
    Some(EventPayload::Toggled(checked))
}

#[cfg(target_os = "android")]
pub(crate) mod platform {
    //! The Android half: build the `Switch`, apply its planned setters, and
    //! hold the echo guard across the ones that notify a listener.

    use jni::objects::JObject;
    use jni::refs::Global;

    use super::{EventPayload, Switch, SwitchProps, decode_toggled};
    use crate::NativeWidgetError;
    use crate::android::{NativeCtx, NativeView};
    use crate::controls::platform::{FRAME_CAPACITY, apply_all};
    use crate::runtime::{NativeEvent, NativeWidget, Params};

    /// `android.widget.Switch` — the *framework* switch, not
    /// `SwitchCompat`/`SwitchMaterial`: this plugin never assumes an
    /// AndroidX or Material dependency the app did not add.
    const CLASS: &str = "android.widget.Switch";

    /// A live switch's retained state.
    pub(crate) struct SwitchState {
        /// The switch's own global reference (the second one — see
        /// `button.rs`'s note).
        view: Global<JObject<'static>>,
        /// The value the platform last reported through its listener, or
        /// `None` while the user has never touched it. Written from
        /// [`NativeWidget::on_event`] via [`decode_toggled`];
        /// [`SwitchProps::plan`] reads it as the write-back's drift signal
        /// (module doc).
        pub(crate) observed: Option<bool>,
    }

    impl NativeWidget for Switch {
        type Props = SwitchProps;
        type State = SwitchState;

        fn decode_props(params: &Params<'_>) -> Result<Self::Props, NativeWidgetError> {
            SwitchProps::decode(params)
        }

        fn create(
            ctx: &mut NativeCtx<'_, '_>,
            props: &Self::Props,
        ) -> Result<(NativeView, Self::State), NativeWidgetError> {
            let view = ctx.new_view(CLASS)?;
            // The plan runs before the listener is attached, so an initial
            // props value that differs from the platform default (e.g.
            // `checked: true` from the start) can never echo back into the
            // runtime at all — stronger than the re-entrancy tolerance
            // `update`'s own writes rely on.
            let plan = SwitchProps::plan(&SwitchProps::platform_default(props.slot), props, None);
            ctx.with_frame(FRAME_CAPACITY, |ctx| apply_all(ctx, &view, &plan))?;
            let listener = ctx.new_listener(props.slot)?;
            ctx.set_on_checked_change_listener(&view, &listener)?;
            let handle = ctx.retain(&view)?;
            let retained = ctx.retain(&view)?;
            let listener_ref = ctx.retain(&listener)?;
            Ok((
                NativeView::with_extra(handle, vec![listener_ref]),
                SwitchState {
                    view: retained,
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
            let plan = SwitchProps::plan(old, new, state.observed);
            // Any Checked setter here that echoes synchronously re-enters
            // `with_runtime` mid-borrow and is dropped there (module doc's
            // *Echo guard*) — there is no suppression state here to hold
            // across the call or to leave latched by a panic mid-`apply_all`.
            let applied = ctx.with_frame(FRAME_CAPACITY, |ctx| apply_all(ctx, &state.view, &plan));
            if applied.is_ok() {
                // The platform now matches the app again — including the case
                // where the write-back above reverted a drifted value. A
                // *failed* apply keeps the drift signal, so the next differing
                // params retries the write-back.
                state.observed = None;
            }
            applied
        }

        fn on_event(state: &mut Self::State, event: NativeEvent) -> Option<EventPayload> {
            decode_toggled(&mut state.observed, event)
        }

        fn dispose(
            ctx: &mut NativeCtx<'_, '_>,
            state: Self::State,
        ) -> Result<(), NativeWidgetError> {
            // Detach so a stray in-flight toggle can't fire after this
            // slot's instance is gone; dropping `state` releases its own
            // global reference either way.
            ctx.set_on_checked_change_listener(&state.view, &JObject::null())
        }
    }
}

#[cfg(target_os = "ios")]
pub(crate) mod platform {
    //! The Apple half: build the `UISwitch` and apply the same planned setters
    //! the Android half hands JNI — with no echo guard of its own (module
    //! doc's *The same contract on iOS*).
    //!
    //! # The two tint setters do not line up one-to-one
    //!
    //! Android's `Switch` has an independent thumb tint and track tint, both
    //! state-list-valued. `UISwitch` has `thumbTintColor` and `onTintColor` —
    //! and `onTintColor` colours the track **only while the switch is on**;
    //! UIKit exposes no setter for the off-state track at all. So
    //! [`Setter::TrackTint`](crate::controls::Setter::TrackTint) maps to
    //! `onTintColor` as the closest available analogue, and an app that tints
    //! the track sees it on the on-state only. That is a UIKit surface limit,
    //! not a mapping choice this module could make differently.
    //!
    //! # `thumbTintColor` is silently ignored on iOS 26 — a platform bug
    //!
    //! Found by the p2-05 device gate on iOS 26.5.2: the `UISwitch` thumb
    //! renders the system white **whatever** `Setter::ThumbTint` assigns,
    //! while the `onTintColor` set on the very next line still works. The
    //! theme's `accent_ink` therefore does not reach the thumb on that OS, and
    //! the control carries its theme through the track alone.
    //!
    //! **Not our bug, and the evidence is on-device rather than inferred**:
    //! `Slider`'s Apple arm sets the *same* `thumbTintColor` property, through
    //! the *same* `apply` path, under the *same* `overrideUserInterfaceStyle`
    //! pin — and its thumb tints correctly (brown in light, amber in dark,
    //! tracking `accent_ink` exactly). Same code, same property, different
    //! UIKit class ⇒ the difference belongs to `UISwitch`. That also rules out
    //! the tempting explanation that pinning the interface style wipes the
    //! tint; it would have wiped the slider's too. Corroborated externally by
    //! several independent reproductions against iOS 18.6, where the same
    //! assignment works; Apple's docs carry no deprecation and no release
    //! note, so it reads as an unacknowledged iOS 26 "Liquid Glass" rendering
    //! regression.
    //!
    //! **The assignment below is deliberately KEPT.** It is correct on every
    //! OS that honours it and starts working again for free if Apple fixes
    //! it; deleting it would code around someone else's bug while silently
    //! dropping the theme on the OSes where it does work. Do not "fix" the
    //! white thumb here — the only workaround found is a non-native custom
    //! thumb, which forfeits the point of this plugin.

    use objc2::MainThreadMarker;
    use objc2::rc::Retained;
    use objc2_ui_kit::UISwitch;

    use super::{KIND, Switch, SwitchProps, decode_toggled};
    use crate::NativeWidgetError;
    use crate::apple::{FrustNativeControlTarget, NativeCtx, NativeView};
    use crate::controls::platform;
    use crate::controls::{Plan, Setter};
    use crate::events::EventPayload;
    use crate::runtime::{NativeEvent, NativeWidget, Params};

    /// A live switch's retained state.
    pub(crate) struct SwitchState {
        view: Retained<UISwitch>,
        /// The target-action object `create` attached as `view`'s
        /// `ValueChanged` action (task p2-03). `UIControl` holds it weakly,
        /// so this field is the only thing keeping it alive for the slot's
        /// lifetime (`crate::apple::events`'s module doc's *Target
        /// retention*) — dropped alongside the rest of `State` when
        /// `Instance::dispose` tears this slot down.
        target: Retained<FrustNativeControlTarget>,
        /// The value the platform last reported through its action, or `None`
        /// while the user has never touched it — [`SwitchProps::plan`]'s
        /// write-back drift signal, identical in meaning to the Android
        /// state's field of the same name.
        ///
        /// Written from [`NativeWidget::on_event`] via [`decode_toggled`],
        /// exactly like the Android arm's own field of this name — the
        /// *write-back seam* was already shared before this task (`plan`
        /// takes `observed` on both arms); this task wires the write side of
        /// it in for the first time on this arm.
        observed: Option<bool>,
    }

    impl NativeWidget for Switch {
        type Props = SwitchProps;
        type State = SwitchState;

        fn decode_props(params: &Params<'_>) -> Result<Self::Props, NativeWidgetError> {
            SwitchProps::decode(params)
        }

        fn create(
            ctx: &mut NativeCtx<'_, '_>,
            props: &Self::Props,
        ) -> Result<(NativeView, Self::State), NativeWidgetError> {
            let mtm = ctx.mtm();
            let view = UISwitch::new(mtm);
            let plan = SwitchProps::plan(&SwitchProps::platform_default(props.slot), props, None);
            apply_all(mtm, &view, &plan);
            // Attached after the initial plan, matching the Android arm's
            // create order — a `Checked` setter never echoes on this arm at
            // all (module doc's *The same contract on iOS*), but the same
            // order keeps the three controls predictable.
            let target = FrustNativeControlTarget::attach_switch(mtm, props.slot, &view);
            let handle = NativeView::new(Retained::clone(&view).into_super().into_super(), mtm);
            Ok((
                handle,
                SwitchState {
                    view,
                    target,
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
            let plan = SwitchProps::plan(old, new, state.observed);
            apply_all(ctx.mtm(), &state.view, &plan);
            // The platform now matches the app again — including the case
            // where the write-back above reverted a drifted value. Cleared
            // unconditionally, unlike the Android arm's `if applied.is_ok()`,
            // because nothing on this arm can fail (`crate::controls`'s Apple
            // `platform` module doc) and so there is no half-applied plan to
            // keep a drift signal alive for.
            state.observed = None;
            Ok(())
        }

        fn on_event(state: &mut Self::State, event: NativeEvent) -> Option<EventPayload> {
            decode_toggled(&mut state.observed, event)
        }

        fn dispose(
            _ctx: &mut NativeCtx<'_, '_>,
            state: Self::State,
        ) -> Result<(), NativeWidgetError> {
            // Detach so a stray in-flight toggle can't fire after this
            // slot's instance is gone, mirroring the Android arm's explicit
            // `setOnCheckedChangeListener(null)`. Dropping `state` afterwards
            // releases the target's own retain.
            state.target.detach_switch(&state.view);
            Ok(())
        }
    }

    /// Execute a whole [`Plan`], front to back.
    fn apply_all(mtm: MainThreadMarker, view: &UISwitch, plan: &Plan<'_>) {
        for setter in plan {
            apply(mtm, view, setter);
        }
    }

    /// Execute one planned property write against `view`.
    fn apply(mtm: MainThreadMarker, view: &UISwitch, setter: &Setter<'_>) {
        match *setter {
            // `setOn:animated:` rather than the plain `isOn` setter — the
            // module doc's Caveat A: this is the one spelling Apple documents
            // as sending no action message. `animated: false` keeps the
            // visible result identical to a property write.
            Setter::Checked(checked) => view.setOn_animated(checked, false),
            Setter::Enabled(enabled) => view.setEnabled(enabled),
            Setter::ThumbTint(argb) => {
                view.setThumbTintColor(platform::optional_ui_color(argb).as_deref());
            }
            Setter::TrackTint(argb) => {
                view.setOnTintColor(platform::optional_ui_color(argb).as_deref());
            }
            // A `UISwitch` renders no text at all on iOS (its `title` property
            // is unavailable here), so there is no font to set — silently, not
            // via `platform::resolve_font`'s degrade warning (theme ladder L3,
            // p2-04), because nothing is being degraded. The field exists in
            // the shared `Props` because Android's `Switch` IS a `TextView`
            // (see `SwitchProps::typeface`).
            Setter::Typeface(_) => {}
            Setter::ContentDescription(label) => {
                platform::set_accessibility_label(view, label, mtm);
            }
            ref other => platform::warn_unexpected_setter(KIND, other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::with_identity;

    fn decode(body: &str) -> SwitchProps {
        let raw = with_identity(KIND, 11, body);
        SwitchProps::decode(&Params::new(&raw)).expect("decodes")
    }

    #[test]
    fn absent_fields_decode_to_the_platform_defaults_and_plan_nothing() {
        let props = decode("");
        assert_eq!(props, SwitchProps::platform_default(11));
        assert!(SwitchProps::plan(&SwitchProps::platform_default(11), &props, None).is_empty());
    }

    #[test]
    fn the_create_plan_sets_exactly_the_non_default_fields() {
        let props = decode(
            "\"checked\":true,\"enabled\":false,\"thumbTint\":16,\"trackTint\":32,\
             \"contentDescription\":\"wifi\"",
        );
        assert_eq!(
            SwitchProps::plan(&SwitchProps::platform_default(11), &props, None),
            vec![
                Setter::Checked(true),
                Setter::Enabled(false),
                Setter::ThumbTint(Some(16)),
                Setter::TrackTint(Some(32)),
                Setter::ContentDescription(Some("wifi")),
            ]
        );
    }

    #[test]
    fn a_confirmed_toggle_writes_the_app_value_back_exactly_once() {
        let off = decode("\"checked\":false");
        let on = decode("\"checked\":true");
        assert_eq!(
            SwitchProps::plan(&off, &on, Some(true)),
            vec![Setter::Checked(true)],
            "the app confirmed what the platform reported: one idempotent set"
        );
    }

    #[test]
    fn a_platform_drift_is_written_back_even_when_the_props_did_not_change_it() {
        // The user flipped the switch on; the app kept `checked: false` and
        // changed something else. Without the write-back the control would
        // stay visually ON while the app model says OFF.
        let old = decode("\"checked\":false");
        let new = decode("\"checked\":false,\"enabled\":false");
        assert_eq!(
            SwitchProps::plan(&old, &new, Some(true)),
            vec![Setter::Checked(false), Setter::Enabled(false)]
        );
        // With no drift reported, the same diff plans only the real change.
        assert_eq!(
            SwitchProps::plan(&old, &new, None),
            vec![Setter::Enabled(false)]
        );
        assert_eq!(
            SwitchProps::plan(&old, &new, Some(false)),
            vec![Setter::Enabled(false)]
        );
    }

    #[test]
    fn clearing_a_tint_plans_the_nullable_setter() {
        let tinted = decode("\"thumbTint\":7,\"trackTint\":8");
        let plain = decode("");
        assert_eq!(
            SwitchProps::plan(&tinted, &plain, None),
            vec![Setter::ThumbTint(None), Setter::TrackTint(None)],
            "unlike a text colour, a tint list is clearable (setter takes null)"
        );
    }

    // f2-04: `SwitchProps` carries no `background_color` field at all (module
    // doc's *No explicit background* section) — there is no wire-shape left
    // to plan a `Setter::BackgroundColor` for, so the two background-setter
    // tests that used to live here (mirroring `label.rs`/`progress.rs`) are
    // deleted rather than updated. `backgroundColor` sent by an old client is
    // simply an unrecognized key `Params` ignores, per `crate::controls`'s
    // degrade-don't-fail rule.

    // --- theme ladder L3: the typeface setter, and Props gating ------------

    #[test]
    fn typeface_defaults_to_system_when_absent() {
        let props = decode("");
        assert_eq!(props.typeface, Typeface::System);
        assert_eq!(props, SwitchProps::platform_default(11));
    }

    #[test]
    fn a_typeface_change_alone_plans_exactly_one_setter() {
        let old = decode("\"typeface\":\"system\"");
        let new = decode("\"typeface\":\"glyphPlex\"");
        assert_eq!(
            SwitchProps::plan(&old, &new, None),
            vec![Setter::Typeface(Typeface::GlyphPlex)]
        );
        assert_eq!(
            SwitchProps::plan(&new, &new, None),
            vec![],
            "an unchanged typeface plans nothing — the zero-FFI property"
        );
    }

    // --- events / echo guard --------------------------------------------

    fn toggled_event(checked: bool) -> NativeEvent {
        NativeEvent {
            kind: EVENT_KIND_TOGGLED,
            detail: crate::events::pack_bool(checked),
        }
    }

    #[test]
    fn a_user_toggle_decodes_and_updates_the_drift_signal() {
        let mut observed = None;
        let payload = decode_toggled(&mut observed, toggled_event(true));
        assert_eq!(payload, Some(EventPayload::Toggled(true)));
        assert_eq!(observed, Some(true));
    }

    // f2-02: there is no per-instance suppression parameter to test an echo
    // against anymore. `CompoundButton.setChecked`'s listener notification is
    // synchronous-only on every supported Android version (AOSP source: the
    // call happens in the same stack frame as `setChecked`, guarded only by
    // `CompoundButton`'s own `mBroadcasting` reentrancy flag, never posted or
    // animation-deferred), so the ONLY guard against a `Setter::Checked` echo
    // is `crate::runtime::with_runtime`'s re-entrancy drop — one layer up,
    // before this decoder is ever called. `decode_toggled` has nothing left
    // to suppress, and there is no per-instance flag a panic mid-`apply_all`
    // could leave latched; the guard mechanism itself is pinned by
    // `runtime::tests::the_thread_local_runtime_is_reentrancy_tolerant`.

    #[test]
    fn a_misrouted_kind_decodes_to_nothing_and_leaves_observed_untouched() {
        let mut observed = Some(false);
        let click = NativeEvent {
            kind: crate::events::EVENT_KIND_CLICK,
            detail: 0,
        };
        assert_eq!(decode_toggled(&mut observed, click), None);
        assert_eq!(observed, Some(false));
    }
}
