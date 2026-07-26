//! `Slider` — a real `android.widget.SeekBar`, built and driven from Rust.
//!
//! Seven properties: value, min, max, enabled, progress tint, thumb tint,
//! accessibility label — all [`Tier::Cheap`](super::Tier::Cheap).
//!
//! # No `setMin`: the range is mapped in Rust
//!
//! `SeekBar.setMin` needs API 26 and this plugin's floor is 24 (every frust
//! Gradle module pins `minSdk = 24`), so calling it would be a
//! `NoSuchMethodError` on two supported API levels. The control therefore maps
//! the app's `[min, max]` onto the platform's `[0, max - min]`: the plan emits
//! [`Setter::Max`] as the *span* and [`Setter::Progress`] as the *offset*
//! value, and p1-05's event decoding adds `min` back on the way out.
//!
//! [`Setter::Max`] is always planned **before** [`Setter::Progress`], because
//! the platform clamps progress to the current max — raising both in the
//! other order silently truncates the value.
//!
//! # Controlled, exactly like `Switch`
//!
//! The drag reports a *requested* value; `update` writes the app-confirmed one
//! back, with the same two seams (`observed` write-back + `suppress_events`
//! echo guard) and the same v1 limitation. See `switch.rs`'s module doc — it
//! is the reference description for both.

use super::{
    CONTENT_DESCRIPTION, ENABLED, MAX, MIN, PROGRESS_TINT, Plan, Setter, THUMB_TINT, VALUE, color,
    owned_text, slot_of,
};
use crate::NativeWidgetError;
use crate::registry::SlotId;
use crate::runtime::Params;

/// The registered kind string the api layer injects as `__frustControl`.
pub(crate) const KIND: &str = "slider";

/// The platform's own default range ceiling for a fresh `SeekBar`
/// (`ProgressBar`'s `mMax`, documented as 100 since API 1).
pub(crate) const PLATFORM_DEFAULT_MAX: i32 = 100;

/// The marker type registered under [`KIND`]; its
/// [`NativeWidget`](crate::runtime::NativeWidget) impl is the Android half
/// below.
pub(crate) struct Slider;

/// Everything a `Slider` slot can be told, as one Rust-diffed value.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SliderProps {
    /// The differ's slot id — **not a property**; `create` needs it for the
    /// listener p1-05 attaches.
    pub(crate) slot: SlotId,
    /// The app-owned position, in the app's own `[min, max]` (controlled).
    pub(crate) value: i32,
    /// The app-space range floor. Mapped away — see the module doc.
    pub(crate) min: i32,
    /// The app-space range ceiling.
    pub(crate) max: i32,
    /// `View.setEnabled`.
    pub(crate) enabled: bool,
    /// Packed ARGB track tint, or `None` to restore the platform's.
    pub(crate) progress_tint: Option<i32>,
    /// Packed ARGB thumb tint, or `None` to restore the platform's.
    pub(crate) thumb_tint: Option<i32>,
    /// The TalkBack label.
    pub(crate) content_description: Option<String>,
}

impl SliderProps {
    /// The state a freshly constructed `new SeekBar(context)` is already in:
    /// `[0, 100]`, at 0.
    pub(crate) fn platform_default(slot: SlotId) -> Self {
        Self {
            slot,
            value: 0,
            min: 0,
            max: PLATFORM_DEFAULT_MAX,
            enabled: true,
            progress_tint: None,
            thumb_tint: None,
            content_description: None,
        }
    }

    /// Decode a `Slider` slot's params.
    ///
    /// # Errors
    /// [`NativeWidgetError::Params`] when the reserved identity keys are
    /// missing.
    pub(crate) fn decode(params: &Params<'_>) -> Result<Self, NativeWidgetError> {
        Ok(Self {
            slot: slot_of(params)?,
            value: int_or(params, VALUE, 0),
            min: int_or(params, MIN, 0),
            max: int_or(params, MAX, PLATFORM_DEFAULT_MAX),
            enabled: params.flag(ENABLED).unwrap_or(true),
            progress_tint: color(params, PROGRESS_TINT),
            thumb_tint: color(params, THUMB_TINT),
            content_description: owned_text(params, CONTENT_DESCRIPTION),
        })
    }

    /// The platform-space span (`max - min`, never negative) this control
    /// hands `setMax`.
    pub(crate) fn span(&self) -> i32 {
        self.max.saturating_sub(self.min).max(0)
    }

    /// The platform-space progress (`value - min`, clamped into the range)
    /// this control hands `setProgress`.
    pub(crate) fn progress(&self) -> i32 {
        self.value
            .clamp(self.min, self.max.max(self.min))
            .saturating_sub(self.min)
    }

    /// The setter-call plan for `old` → `new`, given the platform-space value
    /// the platform last reported (`observed` — p1-05's drag callback), in
    /// the module doc's mandatory max-before-progress order.
    pub(crate) fn plan<'a>(old: &Self, new: &'a Self, observed: Option<i32>) -> Plan<'a> {
        let mut plan = Plan::new();
        if old.span() != new.span() {
            plan.push(Setter::Max(new.span()));
        }
        let drifted = observed.is_some_and(|platform| platform != new.progress());
        if old.progress() != new.progress() || drifted {
            plan.push(Setter::Progress(new.progress()));
        }
        if old.enabled != new.enabled {
            plan.push(Setter::Enabled(new.enabled));
        }
        if old.progress_tint != new.progress_tint {
            plan.push(Setter::ProgressTint(new.progress_tint));
        }
        if old.thumb_tint != new.thumb_tint {
            plan.push(Setter::ThumbTint(new.thumb_tint));
        }
        if old.content_description != new.content_description {
            plan.push(Setter::ContentDescription(
                new.content_description.as_deref(),
            ));
        }
        plan
    }
}

/// An integer field with a documented fallback (`value`/`min`/`max` all have
/// a meaningful platform default, so a missing one is never an error).
fn int_or(params: &Params<'_>, key: &str, fallback: i32) -> i32 {
    params
        .int(key)
        .and_then(|raw| i32::try_from(raw).ok())
        .unwrap_or(fallback)
}

#[cfg(target_os = "android")]
pub(crate) mod platform {
    //! The Android half: build the `SeekBar`, apply its planned setters, and
    //! hold the echo guard across the ones that notify a listener.

    use jni::objects::JObject;
    use jni::refs::Global;

    use super::{Slider, SliderProps};
    use crate::NativeWidgetError;
    use crate::android::{NativeCtx, NativeView};
    use crate::controls::platform::{FRAME_CAPACITY, apply_all};
    use crate::runtime::{NativeWidget, Params};

    /// `android.widget.SeekBar` — the framework class (a `ProgressBar`
    /// subclass, which is why the progress/max setters resolve there).
    const CLASS: &str = "android.widget.SeekBar";

    /// A live slider's retained state.
    pub(crate) struct SliderState {
        /// The slider's own global reference (the second one — see
        /// `button.rs`'s note).
        view: Global<JObject<'static>>,
        /// The **platform-space** progress the platform last reported, or
        /// `None` while the user has never dragged it. p1-05 writes it from
        /// `on_event`; [`SliderProps::plan`] reads it as the write-back's
        /// drift signal.
        pub(crate) observed: Option<i32>,
        /// Held while `update` writes a value the platform will notify a
        /// listener about — p1-05's `on_event` drops an event that arrives
        /// while it is set.
        pub(crate) suppress_events: bool,
    }

    impl NativeWidget for Slider {
        type Props = SliderProps;
        type State = SliderState;

        fn decode_props(params: &Params<'_>) -> Result<Self::Props, NativeWidgetError> {
            SliderProps::decode(params)
        }

        fn create(
            ctx: &mut NativeCtx<'_, '_>,
            props: &Self::Props,
        ) -> Result<(NativeView, Self::State), NativeWidgetError> {
            let view = ctx.new_view(CLASS)?;
            // p1-05 attaches `ctx.new_listener(props.slot)` here (as an
            // `OnSeekBarChangeListener`) and retains it in the handle's
            // `extra` list.
            let plan = SliderProps::plan(&SliderProps::platform_default(props.slot), props, None);
            ctx.with_frame(FRAME_CAPACITY, |ctx| apply_all(ctx, &view, &plan))?;
            let handle = ctx.retain(&view)?;
            let retained = ctx.retain(&view)?;
            Ok((
                NativeView::new(handle),
                SliderState {
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
            let plan = SliderProps::plan(old, new, state.observed);
            state.suppress_events = true;
            let applied = ctx.with_frame(FRAME_CAPACITY, |ctx| apply_all(ctx, &state.view, &plan));
            state.suppress_events = false;
            if applied.is_ok() {
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

    fn decode(body: &str) -> SliderProps {
        let raw = with_identity(KIND, 5, body);
        SliderProps::decode(&Params::new(&raw)).expect("decodes")
    }

    #[test]
    fn absent_fields_decode_to_the_platform_defaults_and_plan_nothing() {
        let props = decode("");
        assert_eq!(props, SliderProps::platform_default(5));
        assert!(SliderProps::plan(&SliderProps::platform_default(5), &props, None).is_empty());
    }

    #[test]
    fn a_non_zero_min_is_mapped_onto_the_platforms_zero_based_range() {
        let props = decode("\"value\":15,\"min\":10,\"max\":20");
        assert_eq!(props.span(), 10);
        assert_eq!(props.progress(), 5);
        assert_eq!(
            SliderProps::plan(&SliderProps::platform_default(5), &props, None),
            vec![Setter::Max(10), Setter::Progress(5)],
            "max is always planned before progress — the platform clamps"
        );
    }

    #[test]
    fn a_value_outside_the_range_is_clamped_rather_than_rejected() {
        assert_eq!(decode("\"value\":99,\"min\":0,\"max\":10").progress(), 10);
        assert_eq!(decode("\"value\":-5,\"min\":0,\"max\":10").progress(), 0);
        // An inverted range degrades to an empty one instead of a negative
        // max the platform would throw on.
        let inverted = decode("\"value\":3,\"min\":10,\"max\":2");
        assert_eq!(inverted.span(), 0);
        assert_eq!(inverted.progress(), 0);
    }

    #[test]
    fn the_create_plan_sets_exactly_the_non_default_fields() {
        let props = decode(
            "\"value\":25,\"max\":50,\"enabled\":false,\"progressTint\":1,\"thumbTint\":2,\
             \"contentDescription\":\"volume\"",
        );
        assert_eq!(
            SliderProps::plan(&SliderProps::platform_default(5), &props, None),
            vec![
                Setter::Max(50),
                Setter::Progress(25),
                Setter::Enabled(false),
                Setter::ProgressTint(Some(1)),
                Setter::ThumbTint(Some(2)),
                Setter::ContentDescription(Some("volume")),
            ]
        );
    }

    #[test]
    fn a_drag_the_app_confirmed_writes_the_value_back_once() {
        let before = decode("\"value\":0,\"max\":10");
        let after = decode("\"value\":7,\"max\":10");
        assert_eq!(
            SliderProps::plan(&before, &after, Some(7)),
            vec![Setter::Progress(7)]
        );
    }

    #[test]
    fn a_platform_drift_is_written_back_even_when_the_props_value_did_not_change() {
        let old = decode("\"value\":3,\"max\":10");
        let new = decode("\"value\":3,\"max\":10,\"enabled\":false");
        assert_eq!(
            SliderProps::plan(&old, &new, Some(9)),
            vec![Setter::Progress(3), Setter::Enabled(false)],
            "the app rejected the drag: the confirmed value is re-asserted"
        );
        assert_eq!(
            SliderProps::plan(&old, &new, None),
            vec![Setter::Enabled(false)]
        );
    }

    #[test]
    fn a_range_change_replans_both_range_setters_in_order() {
        let old = decode("\"value\":5,\"min\":0,\"max\":10");
        let new = decode("\"value\":5,\"min\":5,\"max\":25");
        assert_eq!(
            SliderProps::plan(&old, &new, None),
            vec![Setter::Max(20), Setter::Progress(0)]
        );
    }
}
