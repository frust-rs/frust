//! `Segmented` — a real segmented control, built and driven from Rust:
//! `UISegmentedControl` on iOS/iPadOS, `NSSegmentedControl` (select-one
//! tracking) on macOS, and Material's `MaterialButtonToggleGroup` on Android.
//! In [`super::APPLE_KINDS`] (the first control there) and, for Android, in
//! [`super::MATERIAL_KINDS`]; on Linux/Windows/web the builder renders the
//! frust-drawn refusal banner at compile time.
//!
//! Six properties: the segment labels, the selected segment, enabled,
//! momentary, the selected-segment tint, and the accessibility label. Every
//! one is [`Tier::Cheap`](super::Tier::Cheap) except
//! [`Setter::Segments`] ([`Tier::Relayout`](super::Tier::Relayout): a label
//! list replace re-measures every segment).
//!
//! # The Android arm (decision C7) — Material, through Kotlin statics
//!
//! Android's framework has no segmented control; the stock one is
//! `com.google.android.material.button.MaterialButtonToggleGroup`, which the
//! plugin's Gradle module already depends on (`api`) and which needs a
//! Material3 app theme. The group (single selection, selection required)
//! holds one outlined `MaterialButton` per segment. Its listener is a
//! Material interface that cannot be built from raw JNI and must not be a
//! supertype of `FrustNativeListener`, so the Android `platform` module
//! drives `@JvmStatic` helpers on `FrustNativeControlFactory` (no new JNI
//! export) and the Kotlin listener reports [`EVENT_KIND_SELECTION`] through
//! `FrustNativeListener.reportSelection`. The group is built under the
//! Activity's manifest theme (the Kotlin-internal `FrustMaterialContext.wrap`),
//! not the raw context. Under a non-Material theme
//! construction throws, is logged, and the slot takes the host's existing
//! dead-slot path — never a crash.
//!
//! What Material does not give us: `enabled` is applied per segment (the
//! group's own flag does not reach its buttons); the tint replaces the
//! *checked* background of each segment (there is no single selected-tint
//! property) and leaves unchecked segments transparent; `momentary` has no
//! native mode, so a tap reports and the group then clears its check
//! (selection is not required while momentary); while momentary the
//! platform ignores the planned selection, so no check is ever drawn.
//!
//! # The labels ride flat params
//!
//! `crate::runtime::Params` reads flat objects of strings, numbers and
//! booleans only — no arrays — so the label list crosses the wire as
//! [`SEGMENT_COUNT`] plus one [`segment_key`] string per segment
//! (`"segmentCount":2,"segment0":"Day","segment1":"Week"`). The count is
//! clamped to [`MAX_SEGMENTS`] on decode; a missing label decodes to an empty
//! one (degrade, never a dead slot — `crate::controls`' rule).
//!
//! # The controlled-component contract — `Switch`'s, index-valued
//!
//! `Segmented` is **controlled** (`docs/CODE_STANDARDS.md`'s Interaction
//! Semantics) exactly the way `Switch` is (`switch.rs`'s module doc is the
//! reference account): the platform reports the *requested* segment through
//! [`crate::events::EVENT_KIND_SELECTION`], and the app-confirmed index comes
//! back down as props.
//!
//! 1. **Write-back.** [`SegmentedProps::plan`] takes the index the platform
//!    last reported (`observed`) and re-plans [`Setter::SelectedSegment`]
//!    when it disagrees with the app's value, even though the props' own
//!    `selected` did not change — so a rejected tap snaps back on the next
//!    differing params (the same v1 limitation `switch.rs` documents applies:
//!    an app that reports the identical props back produces no `update`).
//!    A **momentary** control has no persistent selection to drift from, so
//!    its plan ignores `observed`.
//! 2. **Echo guard.** Neither UIKit nor AppKit sends a control's action for a
//!    programmatic `setSelectedSegmentIndex:`/`setSelectedSegment:` (the rule
//!    `crate::apple::events`' and `crate::appkit::events`' *No echo guard*
//!    sections cite), so those arms carry none. Android's
//!    `MaterialButtonToggleGroup` DOES notify for `check`/`clearChecked`, and
//!    additionally for the button being unchecked, so the Kotlin listener
//!    reports only `isChecked == true` and every programmatic mutation runs
//!    muted (`SegmentedHolder.muted`) — the app's write-back never reports
//!    itself. `crate::runtime::with_runtime`'s re-entrancy drop remains the
//!    backstop one layer up.
//!
//! # Replacing the segments resets the selection
//!
//! Both platforms clear their selection when the segment list is rebuilt
//! (`removeAllSegments`, or a `segmentCount` change), so a plan that emits
//! [`Setter::Segments`] always follows it with [`Setter::SelectedSegment`],
//! even when `selected` itself is unchanged. [`Setter::Momentary`] is planned
//! before both, because switching an `NSSegmentedControl`'s tracking mode can
//! clear its selection too.
//!
//! # Out-of-range selections are no selection
//!
//! A `selected` index outside the label list decodes to `None` (no segment
//! selected) rather than reaching the platform: an out-of-range
//! `selectedSegmentIndex` is an Objective-C range exception on UIKit, and an
//! exception unwinding into Rust is undefined behaviour. [`platform_index`]
//! re-checks against the live segment count at apply time, belt and braces.
//!
//! # Theme: the selected segment wears `accent_fill`
//!
//! `crate::api::theme` folds `accent_fill` into [`super::TINT`], applied as
//! `UISegmentedControl.selectedSegmentTintColor` on iOS and
//! `NSSegmentedControl.selectedSegmentBezelColor` on macOS (both nil-clearable
//! back to the platform's own colour). L1 brightness rides the raw wire like
//! every other control. No typeface is folded: UIKit exposes a segment's font
//! only through `setTitleTextAttributes:forState:` (an attributes
//! dictionary), deliberately left for a later task.

use super::{CONTENT_DESCRIPTION, ENABLED, Plan, Setter, TINT, color, owned_text, slot_of};
use crate::NativeWidgetError;
use crate::events::{EVENT_KIND_SELECTION, EventPayload, unpack_index};
use crate::registry::SlotId;
use crate::runtime::{NativeEvent, Params};

/// The registered kind string the api layer injects as `__frustControl`.
pub(crate) const KIND: &str = "segmented";

/// `"segmentCount"` — how many `segment<i>` label keys follow.
pub(crate) const SEGMENT_COUNT: &str = "segmentCount";
/// `"segment"` — the prefix of each label key ([`segment_key`]).
pub(crate) const SEGMENT_PREFIX: &str = "segment";
/// `"selected"` — the app-owned selected index (controlled).
pub(crate) const SELECTED: &str = "selected";
/// `"momentary"` — whether a tap selects persistently (`false`, the default)
/// or only flashes the segment.
pub(crate) const MOMENTARY: &str = "momentary";

/// The most segments one control decodes — a guard against a malformed
/// `segmentCount` driving a huge decode loop, far above any usable control.
pub(crate) const MAX_SEGMENTS: usize = 64;

/// The platforms' shared "no segment selected" sentinel
/// (`UISegmentedControlNoSegment`, and `NSSegmentedControl`'s `-1`).
pub(crate) const NO_SEGMENT: isize = -1;

/// The wire key of label `index`: `"segment0"`, `"segment1"`, …
pub(crate) fn segment_key(index: usize) -> String {
    format!("{SEGMENT_PREFIX}{index}")
}

/// The platform index to write for `selected` against a control currently
/// holding `segment_count` segments: the index itself when it is in range,
/// else [`NO_SEGMENT`] — never an out-of-range index (module doc's
/// *Out-of-range selections*).
pub(crate) fn platform_index(selected: Option<usize>, segment_count: usize) -> isize {
    selected
        .filter(|&index| index < segment_count)
        .and_then(|index| isize::try_from(index).ok())
        .unwrap_or(NO_SEGMENT)
}

/// The marker type registered under [`KIND`] — by the Android arm and the two
/// Apple arms.
pub(crate) struct Segmented;

/// Everything a `Segmented` slot can be told, as one Rust-diffed value.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SegmentedProps {
    /// The differ's slot id — **not a property**; `create` needs it for the
    /// target it attaches.
    pub(crate) slot: SlotId,
    /// The segment titles, in order.
    pub(crate) labels: Vec<String>,
    /// The app-owned selected segment (controlled), `None` for no selection.
    /// Always `< labels.len()` when `Some` (normalized on decode).
    pub(crate) selected: Option<usize>,
    /// `UIControl.enabled` / `NSControl.enabled`.
    pub(crate) enabled: bool,
    /// Momentary tracking: a tap flashes its segment instead of selecting it.
    pub(crate) momentary: bool,
    /// Packed ARGB selected-segment tint, or `None` for the platform's own.
    pub(crate) tint: Option<i32>,
    /// The VoiceOver label.
    pub(crate) content_description: Option<String>,
}

impl SegmentedProps {
    /// The state a freshly constructed control is in once its arm's `create`
    /// has normalized it: no segments, nothing selected, enabled, select-one
    /// tracking, platform tint, no label.
    pub(crate) fn platform_default(slot: SlotId) -> Self {
        Self {
            slot,
            labels: Vec::new(),
            selected: None,
            enabled: true,
            momentary: false,
            tint: None,
            content_description: None,
        }
    }

    /// Decode a `Segmented` slot's params (module doc's *The labels ride flat
    /// params*).
    ///
    /// # Errors
    /// [`NativeWidgetError::Params`] when the reserved identity keys are
    /// missing.
    pub(crate) fn decode(params: &Params<'_>) -> Result<Self, NativeWidgetError> {
        let count = params
            .int(SEGMENT_COUNT)
            .and_then(|count| usize::try_from(count).ok())
            .unwrap_or(0)
            .min(MAX_SEGMENTS);
        let labels: Vec<String> = (0..count)
            .map(|index| owned_text(params, &segment_key(index)).unwrap_or_default())
            .collect();
        let selected = params
            .int(SELECTED)
            .and_then(|index| usize::try_from(index).ok())
            .filter(|&index| index < labels.len());
        Ok(Self {
            slot: slot_of(params)?,
            labels,
            selected,
            enabled: params.flag(ENABLED).unwrap_or(true),
            momentary: params.flag(MOMENTARY).unwrap_or(false),
            tint: color(params, TINT),
            content_description: owned_text(params, CONTENT_DESCRIPTION),
        })
    }

    /// The setter-call plan for `old` → `new`, given the index the platform
    /// last reported through its action (`observed`, `None` until the user
    /// has touched the control).
    ///
    /// Order is load-bearing (module doc's *Replacing the segments resets the
    /// selection*): momentary, then the segment list, then the selection —
    /// which is re-planned whenever either of the first two changed, whenever
    /// the app's value changed, or whenever the platform drifted from it.
    pub(crate) fn plan<'a>(old: &Self, new: &'a Self, observed: Option<usize>) -> Plan<'a> {
        let mut plan = Plan::new();
        let momentary_changed = old.momentary != new.momentary;
        if momentary_changed {
            plan.push(Setter::Momentary(new.momentary));
        }
        let segments_changed = old.labels != new.labels;
        if segments_changed {
            plan.push(Setter::Segments(&new.labels));
        }
        let drifted =
            !new.momentary && observed.is_some_and(|platform| Some(platform) != new.selected);
        if segments_changed || momentary_changed || old.selected != new.selected || drifted {
            plan.push(Setter::SelectedSegment(new.selected));
        }
        if old.enabled != new.enabled {
            plan.push(Setter::Enabled(new.enabled));
        }
        if old.tint != new.tint {
            plan.push(Setter::SegmentTint(new.tint));
        }
        if old.content_description != new.content_description {
            plan.push(Setter::ContentDescription(
                new.content_description.as_deref(),
            ));
        }
        plan
    }
}

/// Decode an [`EVENT_KIND_SELECTION`] firing into the typed vocabulary:
/// records the write-back drift signal (`observed`) and decodes to an
/// [`EventPayload::Selected`].
///
/// `None` when `event.kind` is not the selection kind (defensive — the target
/// is only ever attached for this one kind), or when the platform reported no
/// segment (a negative index, `crate::events::unpack_index`): there is no
/// requested segment to deliver, and `observed` is left untouched.
///
/// Pure and host-testable, and the ONE decoder every arm's `on_event` calls
/// (Android's `MaterialButtonToggleGroup` listener packs the same
/// [`crate::events::pack_index`] detail) — kind/detail parity by construction.
pub(crate) fn decode_event(
    observed: &mut Option<usize>,
    event: NativeEvent,
) -> Option<EventPayload> {
    if event.kind != EVENT_KIND_SELECTION {
        return None;
    }
    let index = unpack_index(event.detail)?;
    *observed = Some(index);
    Some(EventPayload::Selected(index))
}

#[cfg(target_os = "android")]
pub(crate) mod platform {
    //! The Android half: a `MaterialButtonToggleGroup` built and driven
    //! through `FrustNativeControlFactory`'s `@JvmStatic` segmented helpers
    //! (module doc's *The Android arm*).
    //!
    //! A fresh group is [`SegmentedProps::platform_default`] — no segments,
    //! nothing checked, enabled, persistent selection, the theme's own fill —
    //! so no construction-time normalization precedes the diffed create plan.
    //! The listener is attached inside the Kotlin `segmentedCreate`, before
    //! the plan runs; that is safe because every plan write runs muted
    //! on the Kotlin side, so an initial selection never reaches the runtime
    //! as an event.

    use jni::objects::{JObject, JValue};
    use jni::refs::Global;
    use jni::signature::MethodSignature;
    use jni::strings::JNIStr;
    use jni::{jni_sig, jni_str};

    use super::{KIND, Segmented, SegmentedProps, decode_event, platform_index};
    use crate::NativeWidgetError;
    use crate::android::{NativeCtx, NativeView};
    use crate::controls::platform::{FRAME_CAPACITY, apply as apply_shared};
    use crate::controls::{Plan, Setter};
    use crate::events::EventPayload;
    use crate::runtime::{NativeEvent, NativeWidget, Params};

    /// The factory class that hosts the `@JvmStatic` helpers (binary name).
    const FACTORY_CLASS: &str = "dev.frust.nativewidgets.FrustNativeControlFactory";

    /// A live segmented control's retained state.
    pub(crate) struct SegmentedState {
        /// The group's own global reference (the second one — see
        /// `button.rs`'s note).
        view: Global<JObject<'static>>,
        /// The index the platform last reported, `None` while untouched —
        /// [`SegmentedProps::plan`]'s write-back drift signal.
        observed: Option<usize>,
    }

    impl NativeWidget for Segmented {
        type Props = SegmentedProps;
        type State = SegmentedState;

        fn decode_props(params: &Params<'_>) -> Result<Self::Props, NativeWidgetError> {
            SegmentedProps::decode(params)
        }

        fn create(
            ctx: &mut NativeCtx<'_, '_>,
            props: &Self::Props,
        ) -> Result<(NativeView, Self::State), NativeWidgetError> {
            let slot = i64::try_from(props.slot).unwrap_or(i64::MAX);
            let view = ctx.with_frame(FRAME_CAPACITY, |ctx| {
                let class = ctx.class(FACTORY_CLASS)?;
                let activity = ctx.activity()?;
                let qualified = ctx.context()?;
                let view = ctx.run_jni("FrustNativeControlFactory.segmentedCreate", |env| {
                    env.call_static_method(
                        &class,
                        jni_str!("segmentedCreate"),
                        jni_sig!(
                            "(Landroid/app/Activity;Landroid/content/Context;J)Landroid/view/View;"
                        ),
                        &[
                            JValue::Object(activity),
                            JValue::Object(qualified),
                            JValue::Long(slot),
                        ],
                    )?
                    .l()
                })?;
                ctx.retain(&view)
            })?;
            let plan =
                SegmentedProps::plan(&SegmentedProps::platform_default(props.slot), props, None);
            ctx.with_frame(FRAME_CAPACITY, |ctx| apply_all(ctx, &view, &plan))?;
            let handle = ctx.retain(&view)?;
            Ok((
                NativeView::with_extra(handle, Vec::new()),
                SegmentedState {
                    view,
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
            let plan = SegmentedProps::plan(old, new, state.observed);
            let applied = ctx.with_frame(FRAME_CAPACITY, |ctx| apply_all(ctx, &state.view, &plan));
            if applied.is_ok() {
                // The platform matches the app again; a failed apply keeps the
                // drift signal so the next differing params retries it
                // (`Switch`'s Android arm).
                state.observed = None;
            }
            applied
        }

        fn on_event(state: &mut Self::State, event: NativeEvent) -> Option<EventPayload> {
            decode_event(&mut state.observed, event)
        }

        fn dispose(
            ctx: &mut NativeCtx<'_, '_>,
            state: Self::State,
        ) -> Result<(), NativeWidgetError> {
            // Detach and disarm so a stray in-flight selection can't fire
            // after this slot's instance is gone; dropping `state` releases
            // its global reference either way.
            call_static(
                ctx,
                jni_str!("segmentedDispose"),
                jni_sig!("(Landroid/view/View;)V"),
                &[JValue::Object(&state.view)],
            )
        }
    }

    /// `FrustNativeControlFactory.<name>(group, ...)` — a `void` static
    /// helper; `signature` and `args` carry the group first.
    fn call_static<'sig, 'sig_args>(
        ctx: &mut NativeCtx<'_, '_>,
        name: &JNIStr,
        signature: impl AsRef<MethodSignature<'sig, 'sig_args>>,
        args: &[JValue<'_>],
    ) -> Result<(), NativeWidgetError> {
        let class = ctx.class(FACTORY_CLASS)?;
        ctx.run_jni(&format!("FrustNativeControlFactory.{name}"), |env| {
            env.call_static_method(&class, name, signature, args)?.v()
        })
    }

    /// Execute a whole [`Plan`], front to back.
    fn apply_all(
        ctx: &mut NativeCtx<'_, '_>,
        view: &JObject<'_>,
        plan: &Plan<'_>,
    ) -> Result<(), NativeWidgetError> {
        for setter in plan {
            apply(ctx, view, setter)?;
        }
        Ok(())
    }

    /// Execute one planned property write — this control's own setters
    /// here, `ContentDescription` through the shared apply.
    fn apply(
        ctx: &mut NativeCtx<'_, '_>,
        view: &JObject<'_>,
        setter: &Setter<'_>,
    ) -> Result<(), NativeWidgetError> {
        match *setter {
            Setter::Segments(labels) => {
                call_static(
                    ctx,
                    jni_str!("segmentedClear"),
                    jni_sig!("(Landroid/view/View;)V"),
                    &[JValue::Object(view)],
                )?;
                for label in labels {
                    add_segment(ctx, view, label)?;
                }
                Ok(())
            }
            // `check`/`clearChecked` run muted Kotlin-side (module doc's echo
            // guard), so the write-back's snap-back is a plain write.
            Setter::SelectedSegment(selected) => {
                let count = child_count(ctx, view)?;
                let index = i32::try_from(platform_index(selected, count)).unwrap_or(-1);
                call_static(
                    ctx,
                    jni_str!("segmentedSelect"),
                    jni_sig!("(Landroid/view/View;I)V"),
                    &[JValue::Object(view), JValue::Int(index)],
                )
            }
            Setter::Momentary(momentary) => call_static(
                ctx,
                jni_str!("segmentedSetMomentary"),
                jni_sig!("(Landroid/view/View;Z)V"),
                &[JValue::Object(view), JValue::Bool(momentary)],
            ),
            Setter::Enabled(enabled) => call_static(
                ctx,
                jni_str!("segmentedSetEnabled"),
                jni_sig!("(Landroid/view/View;Z)V"),
                &[JValue::Object(view), JValue::Bool(enabled)],
            ),
            Setter::SegmentTint(argb) => call_static(
                ctx,
                jni_str!("segmentedSetTint"),
                jni_sig!("(Landroid/view/View;ZI)V"),
                &[
                    JValue::Object(view),
                    JValue::Bool(argb.is_some()),
                    JValue::Int(argb.unwrap_or(0)),
                ],
            ),
            Setter::ContentDescription(_) => apply_shared(ctx, view, setter),
            ref other => {
                log::warn!(
                    "frust-native-widgets: control '{KIND}' planned a setter its Android arm \
                     does not implement ({other:?}) — ignored"
                );
                Ok(())
            }
        }
    }

    /// `FrustNativeControlFactory.segmentedAdd(group, label)`.
    fn add_segment(
        ctx: &mut NativeCtx<'_, '_>,
        view: &JObject<'_>,
        label: &str,
    ) -> Result<(), NativeWidgetError> {
        let class = ctx.class(FACTORY_CLASS)?;
        ctx.run_jni("FrustNativeControlFactory.segmentedAdd", |env| {
            let text = env.new_string(label)?;
            env.call_static_method(
                &class,
                jni_str!("segmentedAdd"),
                jni_sig!("(Landroid/view/View;Ljava/lang/String;)V"),
                &[JValue::Object(view), JValue::Object(&text)],
            )?
            .v()
        })
    }

    /// `ViewGroup.getChildCount()` — the live segment count the selection is
    /// range-checked against ([`platform_index`]).
    fn child_count(
        ctx: &mut NativeCtx<'_, '_>,
        view: &JObject<'_>,
    ) -> Result<usize, NativeWidgetError> {
        let count = ctx.run_jni("ViewGroup.getChildCount", |env| {
            env.call_method(view, jni_str!("getChildCount"), jni_sig!("()I"), &[])?
                .i()
        })?;
        Ok(usize::try_from(count).unwrap_or(0))
    }
}

#[cfg(target_os = "ios")]
pub(crate) mod platform {
    //! The iOS half: build a `UISegmentedControl` and apply the shared plan.
    //!
    //! A fresh `UISegmentedControl::new` has no segments, no selection
    //! (`UISegmentedControlNoSegment`), is enabled and non-momentary, with the
    //! system selected-segment tint — exactly
    //! [`SegmentedProps::platform_default`], so no construction-time
    //! normalization precedes the diffed create plan. Selections arrive
    //! through [`FrustNativeControlTarget::attach_segmented`]'s `ValueChanged`
    //! action, which reads `selectedSegmentIndex` off the sender.

    use objc2::MainThreadMarker;
    use objc2::rc::Retained;
    use objc2_foundation::NSString;
    use objc2_ui_kit::UISegmentedControl;

    use super::{KIND, Segmented, SegmentedProps, decode_event, platform_index};
    use crate::NativeWidgetError;
    use crate::apple::{FrustNativeControlTarget, NativeCtx, NativeView};
    use crate::controls::platform;
    use crate::controls::{Plan, Setter};
    use crate::events::EventPayload;
    use crate::runtime::{NativeEvent, NativeWidget, Params};

    /// A live segmented control's retained state — `SwitchState`'s shape.
    pub(crate) struct SegmentedState {
        view: Retained<UISegmentedControl>,
        /// The target `create` attached as the `ValueChanged` action.
        /// `UIControl` holds targets weakly, so this is its only retain
        /// (`crate::apple::events`' *Target retention*).
        target: Retained<FrustNativeControlTarget>,
        /// The index the platform last reported, `None` while untouched —
        /// [`SegmentedProps::plan`]'s write-back drift signal.
        observed: Option<usize>,
    }

    impl NativeWidget for Segmented {
        type Props = SegmentedProps;
        type State = SegmentedState;

        fn decode_props(params: &Params<'_>) -> Result<Self::Props, NativeWidgetError> {
            SegmentedProps::decode(params)
        }

        fn create(
            ctx: &mut NativeCtx<'_, '_>,
            props: &Self::Props,
        ) -> Result<(NativeView, Self::State), NativeWidgetError> {
            let mtm = ctx.mtm();
            let view = UISegmentedControl::new(mtm);
            let plan =
                SegmentedProps::plan(&SegmentedProps::platform_default(props.slot), props, None);
            apply_all(mtm, &view, &plan);
            // Attached after the initial plan, the other controls' create
            // order: an initial selection can never reach the runtime as an
            // event, even in principle.
            let target = FrustNativeControlTarget::attach_segmented(mtm, props.slot, &view);
            let handle = NativeView::new(Retained::clone(&view).into_super().into_super(), mtm);
            Ok((
                handle,
                SegmentedState {
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
            let plan = SegmentedProps::plan(old, new, state.observed);
            apply_all(ctx.mtm(), &state.view, &plan);
            // The platform matches the app again; nothing on this arm can
            // fail, so the drift signal is cleared unconditionally (`Switch`'s
            // iOS arm, verbatim).
            state.observed = None;
            Ok(())
        }

        fn on_event(state: &mut Self::State, event: NativeEvent) -> Option<EventPayload> {
            decode_event(&mut state.observed, event)
        }

        fn dispose(
            _ctx: &mut NativeCtx<'_, '_>,
            state: Self::State,
        ) -> Result<(), NativeWidgetError> {
            // Detach so a stray in-flight action can't reach a torn-down
            // slot; dropping `state` afterwards releases the target's retain.
            state.target.detach_segmented(&state.view);
            Ok(())
        }
    }

    /// Execute a whole [`Plan`], front to back.
    fn apply_all(mtm: MainThreadMarker, view: &UISegmentedControl, plan: &Plan<'_>) {
        for setter in plan {
            apply(mtm, view, setter);
        }
    }

    /// Execute one planned property write against `view`.
    fn apply(mtm: MainThreadMarker, view: &UISegmentedControl, setter: &Setter<'_>) {
        match *setter {
            Setter::Segments(labels) => {
                view.removeAllSegments();
                for (index, label) in labels.iter().enumerate() {
                    view.insertSegmentWithTitle_atIndex_animated(
                        Some(&NSString::from_str(label)),
                        index,
                        false,
                    );
                }
            }
            // No action is sent for a programmatic selection (module doc's
            // echo guard), so the write-back's snap-back is a plain write.
            Setter::SelectedSegment(selected) => {
                view.setSelectedSegmentIndex(platform_index(selected, view.numberOfSegments()));
            }
            Setter::Momentary(momentary) => view.setMomentary(momentary),
            Setter::Enabled(enabled) => view.setEnabled(enabled),
            Setter::SegmentTint(argb) => {
                view.setSelectedSegmentTintColor(platform::optional_ui_color(argb).as_deref());
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
    //! The macOS half: build an `NSSegmentedControl` in select-one tracking
    //! and apply the shared plan.
    //!
    //! # Construction-time normalization
    //!
    //! `create` pins `trackingMode` to `SelectOne` (the non-momentary
    //! [`SegmentedProps::platform_default`]) and the segment distribution to
    //! `FillEqually` — equal-width segments filling the slot, the
    //! `UISegmentedControl` default, so the two Apple arms lay out alike —
    //! before the diffed create plan runs.
    //!
    //! # One action, one kind
    //!
    //! The control's single target/action pair is wired with
    //! [`crate::events::EVENT_KIND_SELECTION`]
    //! ([`FrustNativeControlTarget::attach`]); `frustAction:` reads the
    //! sender's `selectedSegment` and packs it with
    //! [`crate::events::pack_index`], and `on_event` decodes it through the
    //! SAME [`decode_event`] the iOS arm calls.

    use objc2::rc::Retained;
    use objc2_app_kit::{NSSegmentDistribution, NSSegmentSwitchTracking, NSSegmentedControl};
    use objc2_foundation::NSString;

    use super::{KIND, Segmented, SegmentedProps, decode_event, platform_index};
    use crate::NativeWidgetError;
    use crate::appkit::{FrustNativeControlTarget, NativeCtx, NativeView};
    use crate::controls::platform;
    use crate::controls::{Plan, Setter};
    use crate::events::{EVENT_KIND_SELECTION, EventPayload};
    use crate::runtime::{NativeEvent, NativeWidget, Params};

    /// A live segmented control's retained state — the iOS arm's shape.
    pub(crate) struct SegmentedState {
        /// The control, kept typed for its own segment/selection setters.
        view: Retained<NSSegmentedControl>,
        /// The target `create` attached. `NSControl.target` is weak, so this
        /// field is its only retain (`crate::appkit::events`' *Target
        /// retention*).
        target: Retained<FrustNativeControlTarget>,
        /// The index the platform last reported, `None` while untouched —
        /// [`SegmentedProps::plan`]'s write-back drift signal.
        observed: Option<usize>,
    }

    impl NativeWidget for Segmented {
        type Props = SegmentedProps;
        type State = SegmentedState;

        fn decode_props(params: &Params<'_>) -> Result<Self::Props, NativeWidgetError> {
            SegmentedProps::decode(params)
        }

        fn create(
            ctx: &mut NativeCtx<'_, '_>,
            props: &Self::Props,
        ) -> Result<(NativeView, Self::State), NativeWidgetError> {
            let mtm = ctx.mtm();
            let view = NSSegmentedControl::new(mtm);
            // Module doc's *Construction-time normalization*.
            view.setTrackingMode(NSSegmentSwitchTracking::SelectOne);
            view.setSegmentDistribution(NSSegmentDistribution::FillEqually);
            let plan =
                SegmentedProps::plan(&SegmentedProps::platform_default(props.slot), props, None);
            apply_all(&view, &plan);
            // Attached after the initial plan, the other controls' order.
            let target =
                FrustNativeControlTarget::attach(mtm, &view, props.slot, EVENT_KIND_SELECTION);
            let handle = NativeView::new(Retained::clone(&view).into_super().into_super(), mtm);
            Ok((
                handle,
                SegmentedState {
                    view,
                    target,
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
            apply_all(&state.view, &SegmentedProps::plan(old, new, state.observed));
            // Cleared unconditionally, as on iOS: nothing here can fail.
            state.observed = None;
            Ok(())
        }

        fn on_event(state: &mut Self::State, event: NativeEvent) -> Option<EventPayload> {
            decode_event(&mut state.observed, event)
        }

        fn dispose(
            _ctx: &mut NativeCtx<'_, '_>,
            state: Self::State,
        ) -> Result<(), NativeWidgetError> {
            // Detach so a stray in-flight action can't reach a torn-down
            // slot; dropping `state` afterwards releases the target's retain.
            state.target.detach(&state.view);
            Ok(())
        }
    }

    /// Execute a whole [`Plan`], front to back.
    fn apply_all(view: &NSSegmentedControl, plan: &Plan<'_>) {
        for setter in plan {
            apply(view, setter);
        }
    }

    /// Execute one planned property write against `view`.
    fn apply(view: &NSSegmentedControl, setter: &Setter<'_>) {
        match *setter {
            Setter::Segments(labels) => {
                // `labels` is capped at `MAX_SEGMENTS` on decode, so every
                // count/index below fits an `NSInteger`.
                view.setSegmentCount(labels.len() as isize);
                for (index, label) in labels.iter().enumerate() {
                    view.setLabel_forSegment(&NSString::from_str(label), index as isize);
                }
            }
            // `setSelectedSegment:` sends no action (module doc's echo guard).
            Setter::SelectedSegment(selected) => {
                let count = usize::try_from(view.segmentCount()).unwrap_or(0);
                view.setSelectedSegment(platform_index(selected, count));
            }
            Setter::Momentary(momentary) => view.setTrackingMode(if momentary {
                NSSegmentSwitchTracking::Momentary
            } else {
                NSSegmentSwitchTracking::SelectOne
            }),
            Setter::Enabled(enabled) => platform::set_enabled(view, enabled),
            Setter::SegmentTint(argb) => {
                view.setSelectedSegmentBezelColor(argb.map(platform::ns_color).as_deref());
            }
            Setter::ContentDescription(label) => platform::set_accessibility_label(view, label),
            ref other => platform::warn_unexpected_setter(KIND, other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controls::Tier;
    use crate::events::pack_index;
    use crate::runtime::with_identity;

    fn decode(body: &str) -> SegmentedProps {
        let raw = with_identity(KIND, 5, body);
        SegmentedProps::decode(&Params::new(&raw)).expect("decodes")
    }

    fn labels(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_string()).collect()
    }

    const DAY_WEEK_MONTH: &str =
        "\"segmentCount\":3,\"segment0\":\"Day\",\"segment1\":\"Week\",\"segment2\":\"Month\"";

    #[test]
    fn absent_fields_decode_to_the_platform_defaults_and_plan_nothing() {
        let props = decode("");
        assert_eq!(props, SegmentedProps::platform_default(5));
        assert!(
            SegmentedProps::plan(&SegmentedProps::platform_default(5), &props, None).is_empty()
        );
    }

    #[test]
    fn labels_decode_from_the_flat_count_and_indexed_keys() {
        let props = decode(&format!("{DAY_WEEK_MONTH},\"selected\":1"));
        assert_eq!(props.labels, labels(&["Day", "Week", "Month"]));
        assert_eq!(props.selected, Some(1));
    }

    #[test]
    fn a_missing_label_decodes_empty_and_the_count_is_capped() {
        let props = decode("\"segmentCount\":2,\"segment1\":\"B\"");
        assert_eq!(props.labels, labels(&["", "B"]));
        let huge = decode("\"segmentCount\":100000");
        assert_eq!(huge.labels.len(), MAX_SEGMENTS);
        let negative = decode("\"segmentCount\":-3");
        assert!(negative.labels.is_empty());
    }

    #[test]
    fn an_out_of_range_selection_decodes_to_no_selection() {
        let past_end = decode(&format!("{DAY_WEEK_MONTH},\"selected\":3"));
        assert_eq!(past_end.selected, None);
        let negative = decode(&format!("{DAY_WEEK_MONTH},\"selected\":-1"));
        assert_eq!(negative.selected, None);
        let no_segments = decode("\"selected\":0");
        assert_eq!(no_segments.selected, None);
    }

    #[test]
    fn the_create_plan_is_momentary_then_segments_then_selection_in_order() {
        let props = decode(&format!(
            "{DAY_WEEK_MONTH},\"selected\":2,\"momentary\":true,\"enabled\":false,\"tint\":9,\
             \"contentDescription\":\"range\""
        ));
        assert_eq!(
            SegmentedProps::plan(&SegmentedProps::platform_default(5), &props, None),
            vec![
                Setter::Momentary(true),
                Setter::Segments(&props.labels),
                Setter::SelectedSegment(Some(2)),
                Setter::Enabled(false),
                Setter::SegmentTint(Some(9)),
                Setter::ContentDescription(Some("range")),
            ]
        );
    }

    #[test]
    fn a_momentary_create_with_a_selection_still_plans_it_for_the_platform_to_mute() {
        let props = decode(&format!(
            "{DAY_WEEK_MONTH},\"selected\":2,\"momentary\":true"
        ));
        let plan = SegmentedProps::plan(&SegmentedProps::platform_default(5), &props, None);
        let momentary = plan
            .iter()
            .position(|s| matches!(s, Setter::Momentary(true)));
        let selected = plan
            .iter()
            .position(|s| matches!(s, Setter::SelectedSegment(Some(2))));
        // Momentary lands first, so Android's select sees the flag and draws no check.
        assert!(momentary.is_some() && momentary < selected);
    }

    #[test]
    fn a_selection_change_alone_plans_exactly_one_setter() {
        let old = decode(&format!("{DAY_WEEK_MONTH},\"selected\":0"));
        let new = decode(&format!("{DAY_WEEK_MONTH},\"selected\":1"));
        assert_eq!(
            SegmentedProps::plan(&old, &new, None),
            vec![Setter::SelectedSegment(Some(1))]
        );
        assert!(
            SegmentedProps::plan(&new, &new, None).is_empty(),
            "unchanged props plan nothing — the zero-FFI property"
        );
    }

    #[test]
    fn a_label_change_replaces_wholesale_and_reasserts_the_selection() {
        let old = decode(&format!("{DAY_WEEK_MONTH},\"selected\":1"));
        let new = decode(
            "\"segmentCount\":3,\"segment0\":\"Day\",\"segment1\":\"Wk\",\"segment2\":\"Month\",\
             \"selected\":1",
        );
        assert_eq!(
            SegmentedProps::plan(&old, &new, None),
            vec![
                Setter::Segments(&new.labels),
                Setter::SelectedSegment(Some(1)),
            ],
            "replacing the segments resets the platform's selection, so it is re-planned"
        );
    }

    #[test]
    fn a_platform_drift_is_written_back_even_when_the_props_did_not_change_it() {
        // The user tapped segment 2; the app kept `selected: 0` and changed
        // something else — the control must snap back to 0.
        let old = decode(&format!("{DAY_WEEK_MONTH},\"selected\":0"));
        let new = decode(&format!(
            "{DAY_WEEK_MONTH},\"selected\":0,\"enabled\":false"
        ));
        assert_eq!(
            SegmentedProps::plan(&old, &new, Some(2)),
            vec![Setter::SelectedSegment(Some(0)), Setter::Enabled(false)]
        );
        assert_eq!(
            SegmentedProps::plan(&old, &new, Some(0)),
            vec![Setter::Enabled(false)],
            "an observed value matching the app's is no drift"
        );
    }

    #[test]
    fn a_momentary_control_ignores_the_drift_signal() {
        let old = decode(&format!("{DAY_WEEK_MONTH},\"momentary\":true"));
        let new = decode(&format!(
            "{DAY_WEEK_MONTH},\"momentary\":true,\"enabled\":false"
        ));
        assert_eq!(
            SegmentedProps::plan(&old, &new, Some(2)),
            vec![Setter::Enabled(false)]
        );
    }

    #[test]
    fn clearing_the_tint_plans_the_nullable_setter() {
        let tinted = decode("\"tint\":7");
        let plain = decode("");
        assert_eq!(
            SegmentedProps::plan(&tinted, &plain, None),
            vec![Setter::SegmentTint(None)]
        );
    }

    #[test]
    fn every_planned_setter_reports_its_documented_tier() {
        let props = decode(&format!(
            "{DAY_WEEK_MONTH},\"selected\":1,\"momentary\":true,\"enabled\":false,\"tint\":1"
        ));
        for setter in SegmentedProps::plan(&SegmentedProps::platform_default(5), &props, None) {
            let expected = if matches!(setter, Setter::Segments(_)) {
                Tier::Relayout
            } else {
                Tier::Cheap
            };
            assert_eq!(setter.tier(), expected, "{setter:?}");
        }
    }

    #[test]
    fn the_platform_index_is_never_out_of_range() {
        assert_eq!(platform_index(Some(0), 3), 0);
        assert_eq!(platform_index(Some(2), 3), 2);
        assert_eq!(platform_index(Some(3), 3), NO_SEGMENT);
        assert_eq!(platform_index(None, 3), NO_SEGMENT);
        assert_eq!(platform_index(Some(0), 0), NO_SEGMENT);
    }

    // --- events ---------------------------------------------------------

    fn selection(index: isize) -> NativeEvent {
        NativeEvent {
            kind: EVENT_KIND_SELECTION,
            detail: pack_index(index),
        }
    }

    #[test]
    fn a_user_selection_decodes_and_updates_the_drift_signal() {
        let mut observed = None;
        assert_eq!(
            decode_event(&mut observed, selection(2)),
            Some(EventPayload::Selected(2))
        );
        assert_eq!(observed, Some(2));
    }

    #[test]
    fn a_no_segment_report_decodes_to_nothing_and_leaves_observed_untouched() {
        let mut observed = Some(1);
        assert_eq!(decode_event(&mut observed, selection(NO_SEGMENT)), None);
        assert_eq!(observed, Some(1));
    }

    #[test]
    fn a_misrouted_kind_decodes_to_nothing() {
        let mut observed = None;
        let toggle = NativeEvent {
            kind: crate::events::EVENT_KIND_TOGGLED,
            detail: 1,
        };
        assert_eq!(decode_event(&mut observed, toggle), None);
        assert_eq!(observed, None);
    }

    // --- the Android arm's wire contract -------------------------------------

    #[test]
    fn the_android_key_set_is_the_shared_flat_params() {
        // The Android arm decodes through the one `SegmentedProps::decode`; the
        // Kotlin helpers receive the plan's scalars, never raw keys. Pin the
        // key spellings the api layer injects so a rename cannot desync them.
        assert_eq!(KIND, "segmented");
        assert_eq!(SEGMENT_COUNT, "segmentCount");
        assert_eq!(SELECTED, "selected");
        assert_eq!(MOMENTARY, "momentary");
        assert_eq!(segment_key(7), "segment7");
        let props = decode(&format!(
            "{DAY_WEEK_MONTH},\"selected\":0,\"enabled\":false,\"tint\":-65536"
        ));
        assert_eq!(props.tint, Some(-65536));
        assert!(!props.enabled);
    }

    #[test]
    fn a_recorded_index_is_written_back_once_then_the_signal_clears() {
        // Echo-guard sequence on the Android arm: the user taps segment 2
        // (`observed`), the app rejects it and keeps 0. The next differing
        // params re-plan the check; `update` then clears `observed`, so a
        // following identical update plans nothing — a programmatic check
        // never re-triggers itself.
        let old = decode(&format!("{DAY_WEEK_MONTH},\"selected\":0"));
        let mut observed = None;
        decode_event(&mut observed, selection(2));
        let new = decode(&format!(
            "{DAY_WEEK_MONTH},\"selected\":0,\"enabled\":false"
        ));
        assert_eq!(
            SegmentedProps::plan(&old, &new, observed),
            vec![Setter::SelectedSegment(Some(0)), Setter::Enabled(false)]
        );
        observed = None;
        assert!(SegmentedProps::plan(&new, &new, observed).is_empty());
    }

    #[test]
    fn a_stale_unchecked_report_is_not_a_selection() {
        // MaterialButtonToggleGroup also notifies for the button that just
        // lost its check; the Kotlin listener drops those, and a negative
        // index that slipped through still decodes to nothing.
        let mut observed = Some(1);
        assert_eq!(decode_event(&mut observed, selection(-1)), None);
        assert_eq!(observed, Some(1));
    }
}
