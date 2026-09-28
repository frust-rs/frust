//! `Slider` — a real `android.widget.SeekBar`, built and driven from Rust.
//!
//! Seven properties: value, min, max, enabled, progress tint, thumb tint,
//! accessibility label — all [`Tier::Cheap`](super::Tier::Cheap).
//!
//! # No `setMin`: the range is mapped in Rust
//!
//! `SeekBar.setMin` needs API 26. This mapping was written when frust's floor
//! was `minSdk = 24`, where calling it would have been a `NoSuchMethodError`
//! on two supported API levels. **The floor is now 26, so `setMin` would be
//! available** — the mapping is retained deliberately rather than because the
//! platform still forces it: it is correct, it is covered by tests, and
//! switching to `setMin` would change the wire plan (`Setter::Max` stops being
//! a span, `Setter::Progress` stops being an offset, and `decode_event` stops
//! adding `min` back), which is a behavioural refactor with no user-visible
//! gain. Revisit only with a reason beyond tidiness.
//!
//! The control maps the app's `[min, max]` onto the platform's
//! `[0, max - min]`: the plan emits
//! [`Setter::Max`] as the *span* and [`Setter::Progress`] as the *offset*
//! value, and [`decode_event`]'s event decoding adds `min` back on the way
//! out.
//!
//! [`Setter::Max`] is always planned **before** [`Setter::Progress`], because
//! the platform clamps progress to the current max — raising both in the
//! other order silently truncates the value.
//!
//! # Controlled, exactly like `Switch`
//!
//! The drag reports a *requested* value; `update` writes the app-confirmed one
//! back, with the same two seams (`observed` write-back + the
//! `crate::runtime::with_runtime` re-entrancy drop as the sole echo guard) and
//! the same v1 limitation. See `switch.rs`'s module doc — it is the reference
//! description for both, including why there is no per-instance suppression
//! flag and why that guard is an incidental property of the call site (not a
//! designed invariant): `ProgressBar.setProgress`'s listener notification
//! (`SeekBar`'s `onProgressRefresh` override) is synchronous-only too, on
//! every supported API level.
//!
//! # Drag edges are a two-arm feature
//!
//! [`EVENT_KIND_DRAG_START`]/[`EVENT_KIND_DRAG_END`] reach [`decode_event`]
//! from Android and iOS only. The macOS arm emits value changes alone —
//! AppKit target-action carries no gesture phase, and nothing synthesizes one
//! (this file's macOS `platform` module doc).
//!
//! # No explicit background
//!
//! Unlike `Label`/`ProgressBar`, `SliderProps` deliberately carries no
//! `background_color` field — `AbsSeekBar` (this control's superclass) also
//! draws its default background from
//! `?attr/selectableItemBackgroundBorderless`, so an explicit
//! `View.setBackgroundColor` (the explicit-background-fill added for
//! `Label`/`ProgressBar`, reverted for this control) would replace the
//! Material touch ripple exactly as `switch.rs`'s own *No explicit
//! background* section describes. The progress/thumb tints below already
//! carry the theme with no such tradeoff.

use super::{
    CONTENT_DESCRIPTION, ENABLED, MAX, MIN, PROGRESS_TINT, Plan, Setter, THUMB_TINT, VALUE, color,
    owned_text, slot_of,
};
use crate::NativeWidgetError;
use crate::events::{
    EVENT_KIND_DRAG_END, EVENT_KIND_DRAG_START, EVENT_KIND_VALUE_CHANGED, EventPayload,
    unpack_value_changed,
};
use crate::registry::SlotId;
use crate::runtime::{NativeEvent, Params};

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
    /// listener it attaches.
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
    /// the platform last reported (`observed` — the drag callback's write,
    /// via [`decode_event`]), in the module doc's mandatory
    /// max-before-progress order.
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

/// Decode a `SeekBar.OnSeekBarChangeListener` firing into the typed
/// vocabulary: a value change reports **platform-space** (module doc's *no
/// `setMin`*) until `min` is added back here. Every event this decoder is
/// ever handed in production is a genuine platform report — a
/// [`Setter::Progress`] echo is dropped one layer up by
/// `crate::runtime::with_runtime`'s re-entrancy tolerance, before
/// `NativeWidget::on_event` runs (module doc, `switch.rs`'s reference
/// description) — so there is nothing left to suppress here; `DragStart`/
/// `DragEnd` were never suppressed either, being always user-caused
/// (touch-driven, never something one of our own setters triggers).
///
/// Pure and host-testable: no JNI, no `SliderState` — the Android glue
/// (`platform::Slider`'s `on_event`) is a two-field forward onto this.
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
        EVENT_KIND_DRAG_START => Some(EventPayload::DragStart),
        EVENT_KIND_DRAG_END => Some(EventPayload::DragEnd),
        _ => None,
    }
}

#[cfg(target_os = "android")]
pub(crate) mod platform {
    //! The Android half: build the `SeekBar`, apply its planned setters, and
    //! hold the echo guard across the ones that notify a listener.

    use jni::objects::JObject;
    use jni::refs::Global;

    use super::{EventPayload, Slider, SliderProps, decode_event};
    use crate::NativeWidgetError;
    use crate::android::{NativeCtx, NativeView};
    use crate::controls::platform::{FRAME_CAPACITY, apply_all};
    use crate::runtime::{NativeEvent, NativeWidget, Params};

    /// `android.widget.SeekBar` — the framework class (a `ProgressBar`
    /// subclass, which is why the progress/max setters resolve there).
    const CLASS: &str = "android.widget.SeekBar";

    /// A live slider's retained state.
    pub(crate) struct SliderState {
        /// The slider's own global reference (the second one — see
        /// `button.rs`'s note).
        view: Global<JObject<'static>>,
        /// The app-space range floor as of the last successfully applied
        /// props — kept here (rather than re-decoding `Props`, which
        /// `on_event` is never handed) so [`decode_event`] can map a
        /// platform-space `SeekBar` report back to app space (module doc's
        /// *no `setMin`*). Updated by `create` and every successful
        /// `update`.
        min: i32,
        /// The **platform-space** progress the platform last reported, or
        /// `None` while the user has never dragged it. Written from
        /// [`NativeWidget::on_event`] via [`decode_event`];
        /// [`SliderProps::plan`] reads it as the write-back's drift signal.
        pub(crate) observed: Option<i32>,
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
            // The plan runs before the listener is attached — see
            // `switch.rs`'s create for why (the reference description).
            let plan = SliderProps::plan(&SliderProps::platform_default(props.slot), props, None);
            ctx.with_frame(FRAME_CAPACITY, |ctx| apply_all(ctx, &view, &plan))?;
            let listener = ctx.new_listener(props.slot)?;
            ctx.set_on_seek_bar_change_listener(&view, &listener)?;
            let handle = ctx.retain(&view)?;
            let retained = ctx.retain(&view)?;
            let listener_ref = ctx.retain(&listener)?;
            Ok((
                NativeView::with_extra(handle, vec![listener_ref]),
                SliderState {
                    view: retained,
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
            let plan = SliderProps::plan(old, new, state.observed);
            // Any Progress setter here that echoes synchronously re-enters
            // `with_runtime` mid-borrow and is dropped there (module doc's
            // *Controlled* section, `switch.rs`'s Echo guard) — there is no
            // suppression state here to hold across the call or to leave
            // latched by a panic mid-`apply_all`.
            let applied = ctx.with_frame(FRAME_CAPACITY, |ctx| apply_all(ctx, &state.view, &plan));
            if applied.is_ok() {
                state.observed = None;
                state.min = new.min;
            }
            applied
        }

        fn on_event(state: &mut Self::State, event: NativeEvent) -> Option<EventPayload> {
            decode_event(&mut state.observed, state.min, event)
        }

        fn dispose(
            ctx: &mut NativeCtx<'_, '_>,
            state: Self::State,
        ) -> Result<(), NativeWidgetError> {
            // Detach so a stray in-flight drag can't fire after this slot's
            // instance is gone; dropping `state` releases its own global
            // reference either way.
            ctx.set_on_seek_bar_change_listener(&state.view, &JObject::null())
        }
    }
}

#[cfg(target_os = "ios")]
pub(crate) mod platform {
    //! The Apple half: build the `UISlider` and apply the same planned setters
    //! the Android half hands JNI — with no echo guard of its own
    //! (`switch.rs`'s module doc is the reference description for this arm
    //! too, Caveats A/B included).
    //!
    //! # The `[0, span]` mapping still applies, for a different reason
    //!
    //! `UISlider` *does* have a real `minimumValue`, unlike `SeekBar` below
    //! API 26 — so this arm could in principle carry the app's `min`
    //! natively. It deliberately does not: the plan it executes is the shared
    //! one, which already emits [`Setter::Max`] as the *span* and
    //! [`Setter::Progress`] as the *offset* value (module doc's *no `setMin`*).
    //! Re-deriving app space here would mean a second, platform-specific
    //! mapping to keep in step with `decode_event`'s inverse — the exact
    //! duplication the shared-plan design exists to avoid. So the slider is
    //! pinned at `minimumValue = 0` and driven in platform space, and the one
    //! visible consequence is that a value the app placed outside its own
    //! range is clamped identically on both platforms.
    //!
    //! # Construction-time normalization
    //!
    //! A fresh `UISlider` is `[0.0, 1.0]` at `0.0`, while
    //! [`SliderProps::platform_default`] describes the fresh *`SeekBar`* —
    //! `[0, 100]` at 0 — and the create plan diffs against that. A control
    //! whose span happens to be [`super::PLATFORM_DEFAULT_MAX`] plans **no**
    //! [`Setter::Max`] at all, and would silently keep UIKit's 1.0 ceiling. So
    //! `create` normalizes the fresh view into the state the shared default
    //! describes *before* running the plan, rather than special-casing the
    //! plan itself — one extra setter at create, and the diff stays the shared
    //! one (`label.rs`'s module doc names this shape).

    use objc2::MainThreadMarker;
    use objc2::rc::Retained;
    use objc2_ui_kit::UISlider;

    use super::{KIND, Slider, SliderProps, decode_event};
    use crate::NativeWidgetError;
    use crate::apple::{FrustNativeControlTarget, NativeCtx, NativeView};
    use crate::controls::platform;
    use crate::controls::{Plan, Setter};
    use crate::events::EventPayload;
    use crate::runtime::{NativeEvent, NativeWidget, Params};

    /// A live slider's retained state.
    pub(crate) struct SliderState {
        view: Retained<UISlider>,
        /// The target-action object `create` attached as `view`'s
        /// `ValueChanged`/`TouchDown`/`TouchUpInside|TouchUpOutside` actions.
        /// `UIControl` holds it weakly, so this field is the
        /// only thing keeping it alive for the slot's lifetime
        /// (`crate::apple::events`'s module doc's *Target retention*) —
        /// dropped alongside the rest of `State` when `Instance::dispose`
        /// tears this slot down.
        target: Retained<FrustNativeControlTarget>,
        /// The app-space range floor as of the last applied props — kept for
        /// the same reason the Android state keeps it: `on_event` is never
        /// handed `Props`, and [`decode_event`] needs `min` to map a
        /// platform-space report back to app space.
        min: i32,
        /// The **platform-space** progress the platform last reported — see
        /// `switch.rs`'s Apple `SwitchState::observed` for the same meaning.
        /// Written from [`NativeWidget::on_event`] via [`decode_event`].
        observed: Option<i32>,
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
            let mtm = ctx.mtm();
            let view = UISlider::new(mtm);
            let default = SliderProps::platform_default(props.slot);
            // Module doc's *Construction-time normalization*: bring the fresh
            // UIKit control into the state `SliderProps::platform_default`
            // describes, so the shared create plan diffs against the truth.
            // The ceiling is read off that default rather than named again
            // here, so a change to it cannot leave the normalization and the
            // diff baseline disagreeing; the floor is structurally 0, because
            // this arm runs entirely in platform space (module doc's *The
            // `[0, span]` mapping*) and never carries the app's own `min`.
            view.setMinimumValue(0.0);
            view.setMaximumValue(default.span() as f32);
            let plan = SliderProps::plan(&default, props, None);
            apply_all(mtm, &view, &plan);
            // Attached after the initial plan and the construction-time
            // normalization above, matching the Android arm's create order —
            // a `Progress`/`Max` setter never echoes on this arm at all
            // (`switch.rs`'s module doc's *The same contract on iOS*), but
            // the same order keeps the three controls predictable.
            let target = FrustNativeControlTarget::attach_slider(mtm, props.slot, &view);
            let handle = NativeView::new(Retained::clone(&view).into_super().into_super(), mtm);
            Ok((
                handle,
                SliderState {
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
            let plan = SliderProps::plan(old, new, state.observed);
            apply_all(ctx.mtm(), &state.view, &plan);
            // Cleared unconditionally — see `switch.rs`'s Apple `update` for
            // why this arm has no `is_ok()` gate.
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
            // Detach so a stray in-flight drag can't fire after this slot's
            // instance is gone, mirroring the Android arm's explicit
            // `setOnSeekBarChangeListener(null)`. Dropping `state` afterwards
            // releases the target's own retain.
            state.target.detach_slider(&state.view);
            Ok(())
        }
    }

    /// Execute a whole [`Plan`], front to back — max before progress, the
    /// order the shared plan already guarantees (module doc).
    fn apply_all(mtm: MainThreadMarker, view: &UISlider, plan: &Plan<'_>) {
        for setter in plan {
            apply(mtm, view, setter);
        }
    }

    /// Execute one planned property write against `view`.
    fn apply(mtm: MainThreadMarker, view: &UISlider, setter: &Setter<'_>) {
        match *setter {
            Setter::Max(span) => view.setMaximumValue(span as f32),
            // The plain `setValue:`, not `setValue:animated:` — the animated
            // spelling is documented as a *transition*, which would make a
            // controlled write-back visibly lag the app's own value. Its
            // no-action behaviour rests on the general UIControl rule either
            // way (`switch.rs`'s Caveat A).
            Setter::Progress(progress) => view.setValue(progress as f32),
            Setter::Enabled(enabled) => view.setEnabled(enabled),
            // Android's `progressTint` colours the filled part of the track,
            // which is `minimumTrackTintColor` here; the unfilled part
            // (`maximumTrackTintColor`) has no `Props` field on either arm.
            Setter::ProgressTint(argb) => {
                view.setMinimumTrackTintColor(platform::optional_ui_color(argb).as_deref());
            }
            Setter::ThumbTint(argb) => {
                view.setThumbTintColor(platform::optional_ui_color(argb).as_deref());
            }
            Setter::ContentDescription(label) => {
                platform::set_accessibility_label(view, label, mtm);
            }
            ref other => platform::warn_unexpected_setter(KIND, other),
        }
    }
}

#[cfg(target_os = "macos")]
pub(crate) mod platform {
    //! The macOS half: build an `NSSlider` and apply the same planned setters
    //! the Android and iOS halves do — controlled exactly like `Switch`, with
    //! no echo guard of its own (`switch.rs`'s module doc, *And on macOS*).
    //!
    //! # Platform space, like iOS
    //!
    //! `NSSlider` has a real `minValue`, as `UISlider` has `minimumValue`, and
    //! this arm declines it for the iOS arm's reason (that module's *The
    //! `[0, span]` mapping still applies*): the shared plan already emits
    //! [`Setter::Max`] as the span and [`Setter::Progress`] as the offset, and
    //! [`decode_event`] adds `min` back. So the slider is pinned at
    //! `minValue = 0`, `maxValue = span`, and its `doubleValue` is
    //! platform-space by construction — which is why
    //! `crate::appkit::events`' value arm subtracts nothing.
    //!
    //! # Construction-time normalization
    //!
    //! A fresh `NSSlider` is `[0.0, 1.0]` at `0.0`, not the `[0, 100]` at 0
    //! [`SliderProps::platform_default`] describes — the iOS arm's exact
    //! problem, with the same fix: `create` normalizes the range before running
    //! the shared create plan, so a span of exactly
    //! [`super::PLATFORM_DEFAULT_MAX`] (which plans no [`Setter::Max`]) does not
    //! keep AppKit's 1.0 ceiling. It also pins the two behaviours this arm
    //! depends on rather than inheriting them: `continuous` (the action fires on
    //! every drag step, not only on mouse-up — the value stream the app's
    //! controlled write-back runs on) and horizontal orientation (a zero-frame
    //! slider's orientation is otherwise inferred from a frame the host sets
    //! only later).
    //!
    //! # Which event kinds this arm emits
    //!
    //! [`EVENT_KIND_VALUE_CHANGED`] only, on every drag step, with `from_user`
    //! always `true`. **`DragStart`/`DragEnd` are never emitted on macOS**:
    //! AppKit target-action carries no gesture phase, and this arm does not
    //! synthesize one (`crate::appkit::events`' *Drag start/end are never
    //! emitted on macOS*). [`decode_event`] still decodes both kinds, for the
    //! other two arms.
    //!
    //! # Tints
    //!
    //! Both route to the theme ladder's shared placeholder
    //! (`crate::controls::platform::set_tint`, TODO m1-04): `NSSlider` has a
    //! `trackFillColor` (the progress tint's natural home) but no thumb tint,
    //! and the ladder owns that mapping.

    use objc2::rc::Retained;
    use objc2_app_kit::NSSlider;

    use super::{KIND, Slider, SliderProps, decode_event};
    use crate::NativeWidgetError;
    use crate::appkit::{FrustNativeControlTarget, NativeCtx, NativeView};
    use crate::controls::platform;
    use crate::controls::{Plan, Setter};
    use crate::events::{EVENT_KIND_VALUE_CHANGED, EventPayload};
    use crate::runtime::{NativeEvent, NativeWidget, Params};

    /// A live slider's retained state — the iOS arm's shape.
    pub(crate) struct SliderState {
        /// The slider, kept typed for `NSSlider`'s own range setters.
        view: Retained<NSSlider>,
        /// The target `create` attached — its only retain (`NSControl.target`
        /// is weak; `crate::appkit::events`' *Target retention*).
        target: Retained<FrustNativeControlTarget>,
        /// The app-space range floor as of the last applied props — `on_event`
        /// is never handed `Props`, and [`decode_event`] needs `min` to map a
        /// platform-space report back to app space.
        min: i32,
        /// The **platform-space** value the platform last reported, or `None`
        /// while the user has never dragged it — [`SliderProps::plan`]'s
        /// write-back drift signal. Written from [`NativeWidget::on_event`]
        /// via [`decode_event`].
        observed: Option<i32>,
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
            let mtm = ctx.mtm();
            let view = NSSlider::new(mtm);
            let default = SliderProps::platform_default(props.slot);
            // Module doc's *Construction-time normalization*. The ceiling is
            // read off the shared default so the normalization and the diff
            // baseline cannot disagree; the floor is structurally 0 (platform
            // space).
            view.setVertical(false);
            view.setContinuous(true);
            view.setMinValue(0.0);
            view.setMaxValue(f64::from(default.span()));
            view.setDoubleValue(f64::from(default.progress()));
            let plan = SliderProps::plan(&default, props, None);
            apply_all(&view, &plan);
            // Attached after the normalization and the initial plan, matching
            // the other two arms' create order.
            let target =
                FrustNativeControlTarget::attach(mtm, &view, props.slot, EVENT_KIND_VALUE_CHANGED);
            let handle = NativeView::new(Retained::clone(&view).into_super().into_super(), mtm);
            Ok((
                handle,
                SliderState {
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
            // `observed` feeds the drift branch: a drag the app refused plans
            // `Setter::Progress` even when the app's value did not change, and
            // `setDoubleValue:` below drives the knob back.
            let plan = SliderProps::plan(old, new, state.observed);
            apply_all(&state.view, &plan);
            // Cleared unconditionally — see `switch.rs`'s macOS `update`.
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
            // Detach so a stray in-flight drag can't reach a torn-down slot;
            // dropping `state` afterwards releases the target's only retain.
            state.target.detach(&state.view);
            Ok(())
        }
    }

    /// Execute a whole [`Plan`], front to back — max before progress, the
    /// order the shared plan already guarantees (module doc, top of file).
    fn apply_all(view: &NSSlider, plan: &Plan<'_>) {
        for setter in plan {
            apply(view, setter);
        }
    }

    /// Execute one planned property write against `view`.
    fn apply(view: &NSSlider, setter: &Setter<'_>) {
        match *setter {
            Setter::Max(span) => view.setMaxValue(f64::from(span)),
            // `setDoubleValue:` sends no action (`crate::appkit::events`' *No
            // echo guard*) — the write-back is a plain property write.
            Setter::Progress(progress) => view.setDoubleValue(f64::from(progress)),
            Setter::Enabled(enabled) => platform::set_enabled(view, enabled),
            Setter::ProgressTint(argb) => platform::set_tint(view, "ProgressTint", argb),
            Setter::ThumbTint(argb) => platform::set_tint(view, "ThumbTint", argb),
            Setter::ContentDescription(label) => platform::set_accessibility_label(view, label),
            ref other => platform::warn_unexpected_setter(KIND, other),
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

    // `SliderProps` carries no `background_color` field at all
    // (module doc's *No explicit background* section) — there is no
    // wire-shape left to plan a `Setter::BackgroundColor` for, so the two
    // background-setter tests that used to live here (mirroring
    // `label.rs`/`progress.rs`) are deleted rather than updated.
    // `backgroundColor` sent by an old client is simply an unrecognized key
    // `Params` ignores, per `crate::controls`'s degrade-don't-fail rule.

    // --- events / echo guard --------------------------------------------

    fn value_changed_event(platform_value: i32, from_user: bool) -> NativeEvent {
        NativeEvent {
            kind: EVENT_KIND_VALUE_CHANGED,
            detail: crate::events::pack_value_changed(platform_value, from_user),
        }
    }

    #[test]
    fn a_user_drag_decodes_to_app_space_and_updates_the_drift_signal() {
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

    // There is no per-instance suppression parameter to test an echo
    // against anymore. `ProgressBar.setProgress`'s listener notification is
    // synchronous-only (same AOSP-source basis as `switch.rs`'s), so the ONLY
    // guard against a `Setter::Progress` echo is
    // `crate::runtime::with_runtime`'s re-entrancy drop, one layer up. There
    // is no per-instance flag left for this decoder to consult or for a panic
    // mid-`apply_all` to latch; the guard mechanism itself is pinned by
    // `runtime::tests::the_thread_local_runtime_is_reentrancy_tolerant`.

    #[test]
    fn drag_start_and_end_decode_unconditionally_and_do_not_touch_observed() {
        let mut observed = Some(3);
        let start = NativeEvent {
            kind: EVENT_KIND_DRAG_START,
            detail: 0,
        };
        let end = NativeEvent {
            kind: EVENT_KIND_DRAG_END,
            detail: 0,
        };
        assert_eq!(
            decode_event(&mut observed, 0, start),
            Some(EventPayload::DragStart),
            "a drag gesture is always user-caused"
        );
        assert_eq!(
            decode_event(&mut observed, 0, end),
            Some(EventPayload::DragEnd)
        );
        assert_eq!(observed, Some(3), "neither kind touches the drift signal");
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
}
