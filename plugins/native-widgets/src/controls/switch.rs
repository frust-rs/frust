//! `Switch` — a real `android.widget.Switch`, built and driven from Rust.
//!
//! Five properties: checked, enabled, thumb tint, track tint, accessibility
//! label. Every one of them is [`Tier::Cheap`](super::Tier::Cheap).
//!
//! # The controlled-component contract, and its two seams
//!
//! A `Switch` is **controlled** (`docs/CODE_STANDARDS.md`'s Interaction
//! Semantics): the platform reports the *requested* value through
//! `OnCheckedChangeListener` (p1-05), and the app-confirmed value comes back
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
//!    value would otherwise look like a fresh user toggle. Two things stop
//!    that, in this order: Android's *synchronous* notification re-enters the
//!    runtime mid-`update` and is dropped there (`crate::runtime`'s
//!    re-entrancy tolerance — free, but it only covers the synchronous case),
//!    and the Android state's `suppress_events` flag, held across the whole
//!    write, which is the explicit check p1-05's `on_event` makes and which
//!    also covers a posted or animation-deferred notification arriving after
//!    `update` returned.
//!
//! **v1 limitation, deliberate:** an app that *rejects* a toggle (reports the
//! same value back) produces no props change at all, so `update` never runs
//! and the platform's flip stands until the next differing params.
//! `NativeWidget::on_event` is handed no platform context, so a control cannot
//! revert from the event itself; closing this needs an event-time context in
//! the trait, which Phase 3 decides.

use super::{
    CHECKED, CONTENT_DESCRIPTION, ENABLED, Plan, Setter, THUMB_TINT, TRACK_TINT, color, owned_text,
    slot_of,
};
use crate::NativeWidgetError;
use crate::registry::SlotId;
use crate::runtime::Params;

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
    /// listener p1-05 attaches.
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
        if old.content_description != new.content_description {
            plan.push(Setter::ContentDescription(
                new.content_description.as_deref(),
            ));
        }
        plan
    }
}

#[cfg(target_os = "android")]
pub(crate) mod platform {
    //! The Android half: build the `Switch`, apply its planned setters, and
    //! hold the echo guard across the ones that notify a listener.

    use jni::objects::JObject;
    use jni::refs::Global;

    use super::{Switch, SwitchProps};
    use crate::NativeWidgetError;
    use crate::android::{NativeCtx, NativeView};
    use crate::controls::platform::{FRAME_CAPACITY, apply_all};
    use crate::runtime::{NativeWidget, Params};

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
        /// `None` while the user has never touched it. **p1-05 writes this**
        /// from `on_event`; [`SwitchProps::plan`] reads it as the
        /// write-back's drift signal (module doc).
        pub(crate) observed: Option<bool>,
        /// Held while `update` writes a value the platform will notify a
        /// listener about — the echo guard p1-05's `on_event` checks before
        /// invoking the app's callback (module doc).
        pub(crate) suppress_events: bool,
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
            // p1-05 attaches `ctx.new_listener(props.slot)` here (as an
            // `OnCheckedChangeListener`) and retains it in the handle's
            // `extra` list. Nothing is attached yet, so the create plan below
            // cannot echo.
            let plan = SwitchProps::plan(&SwitchProps::platform_default(props.slot), props, None);
            ctx.with_frame(FRAME_CAPACITY, |ctx| apply_all(ctx, &view, &plan))?;
            let handle = ctx.retain(&view)?;
            let retained = ctx.retain(&view)?;
            Ok((
                NativeView::new(handle),
                SwitchState {
                    view: retained,
                    observed: None,
                    suppress_events: false,
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
            // The guard is held across the whole plan, not just the checked
            // setter: the flag is what p1-05's listener consults, and a
            // partially-applied plan must not leave it set.
            state.suppress_events = true;
            let applied = ctx.with_frame(FRAME_CAPACITY, |ctx| apply_all(ctx, &state.view, &plan));
            state.suppress_events = false;
            if applied.is_ok() {
                // The platform now matches the app again — including the case
                // where the write-back above reverted a drifted value. A
                // *failed* apply keeps the drift signal, so the next differing
                // params retries the write-back.
                state.observed = None;
            }
            applied
        }

        fn dispose(
            _ctx: &mut NativeCtx<'_, '_>,
            _state: Self::State,
        ) -> Result<(), NativeWidgetError> {
            // p1-05 detaches the listener here; dropping the state releases
            // its global reference either way.
            Ok(())
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
}
