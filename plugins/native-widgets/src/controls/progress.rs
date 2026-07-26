//! `ProgressBar` — a real `android.widget.ProgressBar`, built and driven from
//! Rust.
//!
//! Six properties: value, min, max, indeterminate, progress tint,
//! accessibility label — all [`Tier::Cheap`](super::Tier::Cheap), which makes
//! this the one control in the set whose *whole* property surface is safe to
//! drive at frame rate (PLAN 4's streaming-surface use case).
//!
//! Display-only: it emits no events, so its
//! [`on_event`](crate::runtime::NativeWidget::on_event) stays the trait's
//! default no-op.
//!
//! The app-space `[min, max]` range is mapped onto the platform's
//! `[0, max - min]` exactly as `slider.rs` describes (same `ProgressBar`
//! setters, same API-26 `setMin` problem, same mandatory max-before-progress
//! ordering).
//!
//! # Why this control does not use the one-argument constructor
//!
//! `new ProgressBar(context)` resolves the framework's default
//! `progressBarStyle`, which is the **circular spinner** — it ships an
//! indeterminate drawable and no progress drawable, so `setIndeterminate(false)`
//! on one draws nothing at all. The control therefore constructs with
//! `android.R.attr.progressBarStyleHorizontal` (the three-argument
//! `ProgressBar(Context, AttributeSet, int)`), the one stock style that
//! carries **both** drawables — so `indeterminate` is a live property here
//! rather than a construction-time fork.
//!
//! That style's own defaults (`indeterminate = false`, `[0, 100]`, at 0) are
//! what [`ProgressProps::platform_default`] describes, and therefore what the
//! create plan diffs against.

use super::{
    CONTENT_DESCRIPTION, INDETERMINATE, MAX, MIN, PROGRESS_TINT, Plan, Setter, VALUE, color,
    owned_text, slot_of,
};
use crate::NativeWidgetError;
use crate::registry::SlotId;
use crate::runtime::Params;

/// The registered kind string the api layer injects as `__frustControl`.
pub(crate) const KIND: &str = "progress";

/// The marker type registered under [`KIND`]; its
/// [`NativeWidget`](crate::runtime::NativeWidget) impl is the Android half
/// below.
pub(crate) struct Progress;

/// Everything a `ProgressBar` slot can be told, as one Rust-diffed value.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ProgressProps {
    /// The differ's slot id — **not a property**. Carried for symmetry with
    /// the interactive controls (and because a future accessibility or
    /// diagnostics path is slot-keyed); no listener is ever attached.
    pub(crate) slot: SlotId,
    /// The position, in the app's own `[min, max]`.
    pub(crate) value: i32,
    /// The app-space range floor. Mapped away — see the module doc.
    pub(crate) min: i32,
    /// The app-space range ceiling.
    pub(crate) max: i32,
    /// Spinner mode: `true` ignores the value entirely.
    pub(crate) indeterminate: bool,
    /// Packed ARGB bar tint, or `None` to restore the platform's.
    pub(crate) progress_tint: Option<i32>,
    /// The TalkBack label.
    pub(crate) content_description: Option<String>,
}

impl ProgressProps {
    /// The state a horizontal-styled `ProgressBar` is constructed in:
    /// determinate, `[0, 100]`, at 0 (module doc's *why not the one-argument
    /// constructor*).
    pub(crate) fn platform_default(slot: SlotId) -> Self {
        Self {
            slot,
            value: 0,
            min: 0,
            max: super::slider::PLATFORM_DEFAULT_MAX,
            indeterminate: false,
            progress_tint: None,
            content_description: None,
        }
    }

    /// Decode a `ProgressBar` slot's params.
    ///
    /// # Errors
    /// [`NativeWidgetError::Params`] when the reserved identity keys are
    /// missing.
    pub(crate) fn decode(params: &Params<'_>) -> Result<Self, NativeWidgetError> {
        Ok(Self {
            slot: slot_of(params)?,
            value: int_or(params, VALUE, 0),
            min: int_or(params, MIN, 0),
            max: int_or(params, MAX, super::slider::PLATFORM_DEFAULT_MAX),
            indeterminate: params.flag(INDETERMINATE).unwrap_or(false),
            progress_tint: color(params, PROGRESS_TINT),
            content_description: owned_text(params, CONTENT_DESCRIPTION),
        })
    }

    /// The platform-space span (`max - min`, never negative).
    pub(crate) fn span(&self) -> i32 {
        self.max.saturating_sub(self.min).max(0)
    }

    /// The platform-space progress (`value - min`, clamped into the range).
    pub(crate) fn progress(&self) -> i32 {
        self.value
            .clamp(self.min, self.max.max(self.min))
            .saturating_sub(self.min)
    }

    /// The setter-call plan for `old` → `new`, max before progress (the
    /// platform clamps — see the module doc).
    ///
    /// The value setters are planned even while `indeterminate` is true: the
    /// platform keeps the determinate state behind the spinner, so a control
    /// that flips back shows the right position immediately instead of a
    /// stale one.
    pub(crate) fn plan<'a>(old: &Self, new: &'a Self) -> Plan<'a> {
        let mut plan = Plan::new();
        if old.span() != new.span() {
            plan.push(Setter::Max(new.span()));
        }
        if old.progress() != new.progress() {
            plan.push(Setter::Progress(new.progress()));
        }
        if old.indeterminate != new.indeterminate {
            plan.push(Setter::Indeterminate(new.indeterminate));
        }
        if old.progress_tint != new.progress_tint {
            plan.push(Setter::ProgressTint(new.progress_tint));
        }
        if old.content_description != new.content_description {
            plan.push(Setter::ContentDescription(
                new.content_description.as_deref(),
            ));
        }
        plan
    }
}

/// An integer field with a documented fallback.
fn int_or(params: &Params<'_>, key: &str, fallback: i32) -> i32 {
    params
        .int(key)
        .and_then(|raw| i32::try_from(raw).ok())
        .unwrap_or(fallback)
}

#[cfg(target_os = "android")]
pub(crate) mod platform {
    //! The Android half: build the `ProgressBar` and hand its planned setters
    //! to [`crate::controls::platform`].

    use jni::objects::{JObject, JValue};
    use jni::refs::Global;
    use jni::{jni_sig, jni_str};

    use super::{Progress, ProgressProps};
    use crate::NativeWidgetError;
    use crate::android::{NativeCtx, NativeView};
    use crate::controls::platform::{FRAME_CAPACITY, apply_all};
    use crate::runtime::{NativeWidget, Params};

    /// `android.widget.ProgressBar` — the framework class.
    const CLASS: &str = "android.widget.ProgressBar";

    /// `android.R$attr` — where the stock style attribute below lives (the
    /// binary name of the nested `R.attr` class, `$`-separated as
    /// `ClassLoader.loadClass` expects).
    const R_ATTR_CLASS: &str = "android.R$attr";

    /// `new ProgressBar(context, null, android.R.attr.progressBarStyleHorizontal)`
    /// — the module doc's *why not the one-argument constructor*.
    ///
    /// The style attribute is read from the framework's own `R.attr` rather
    /// than hardcoded (`0x01010078`): it is public API, and a literal
    /// resource id in Rust is exactly the kind of value that silently rots.
    ///
    /// # Errors
    /// [`NativeWidgetError::Platform`] when a class cannot be loaded, the call
    /// path carries no `Context`, or the constructor throws.
    fn new_horizontal_bar<'local>(
        ctx: &mut NativeCtx<'local, '_>,
    ) -> Result<JObject<'local>, NativeWidgetError> {
        let attrs = ctx.class(R_ATTR_CLASS)?;
        let style = ctx.run_jni("android.R.attr.progressBarStyleHorizontal", |env| {
            env.get_static_field(
                &attrs,
                jni_str!("progressBarStyleHorizontal"),
                jni_sig!("I"),
            )?
            .i()
        })?;
        let class = ctx.class(CLASS)?;
        let context = ctx.context()?;
        ctx.run_jni("new ProgressBar(Context, AttributeSet, int)", |env| {
            env.new_object(
                &class,
                jni_sig!("(Landroid/content/Context;Landroid/util/AttributeSet;I)V"),
                &[
                    JValue::Object(context),
                    JValue::Object(&JObject::null()),
                    JValue::Int(style),
                ],
            )
        })
    }

    /// A live progress bar's retained state.
    pub(crate) struct ProgressState {
        /// The bar's own global reference (the second one — see `button.rs`).
        view: Global<JObject<'static>>,
    }

    impl NativeWidget for Progress {
        type Props = ProgressProps;
        type State = ProgressState;

        fn decode_props(params: &Params<'_>) -> Result<Self::Props, NativeWidgetError> {
            ProgressProps::decode(params)
        }

        fn create(
            ctx: &mut NativeCtx<'_, '_>,
            props: &Self::Props,
        ) -> Result<(NativeView, Self::State), NativeWidgetError> {
            let view = new_horizontal_bar(ctx)?;
            let plan = ProgressProps::plan(&ProgressProps::platform_default(props.slot), props);
            ctx.with_frame(FRAME_CAPACITY, |ctx| apply_all(ctx, &view, &plan))?;
            let handle = ctx.retain(&view)?;
            let retained = ctx.retain(&view)?;
            Ok((NativeView::new(handle), ProgressState { view: retained }))
        }

        fn update(
            ctx: &mut NativeCtx<'_, '_>,
            state: &mut Self::State,
            old: &Self::Props,
            new: &Self::Props,
        ) -> Result<(), NativeWidgetError> {
            let plan = ProgressProps::plan(old, new);
            ctx.with_frame(FRAME_CAPACITY, |ctx| apply_all(ctx, &state.view, &plan))
        }

        fn dispose(
            _ctx: &mut NativeCtx<'_, '_>,
            _state: Self::State,
        ) -> Result<(), NativeWidgetError> {
            // Display-only: nothing attached; dropping the state releases its
            // global reference.
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::with_identity;

    fn decode(body: &str) -> ProgressProps {
        let raw = with_identity(KIND, 2, body);
        ProgressProps::decode(&Params::new(&raw)).expect("decodes")
    }

    #[test]
    fn a_bare_payload_matches_the_constructed_bar_and_plans_nothing() {
        let props = decode("");
        assert_eq!(props, ProgressProps::platform_default(2));
        assert!(ProgressProps::plan(&ProgressProps::platform_default(2), &props).is_empty());
    }

    #[test]
    fn asking_for_a_spinner_plans_exactly_one_setter() {
        let props = decode("\"indeterminate\":true");
        assert_eq!(
            ProgressProps::plan(&ProgressProps::platform_default(2), &props),
            vec![Setter::Indeterminate(true)]
        );
    }

    #[test]
    fn the_create_plan_sets_exactly_the_non_default_fields_in_order() {
        let props = decode(
            "\"value\":30,\"min\":10,\"max\":110,\"progressTint\":9,\
             \"contentDescription\":\"upload\"",
        );
        assert_eq!(
            ProgressProps::plan(&ProgressProps::platform_default(2), &props),
            vec![
                Setter::Progress(20),
                Setter::ProgressTint(Some(9)),
                Setter::ContentDescription(Some("upload")),
            ],
            "span is already 100, so only the offset progress is planned"
        );
    }

    #[test]
    fn a_streaming_value_change_is_one_cheap_setter_per_update() {
        let old = decode("\"value\":1,\"max\":1000");
        let new = decode("\"value\":2,\"max\":1000");
        let plan = ProgressProps::plan(&old, &new);
        assert_eq!(plan, vec![Setter::Progress(2)]);
        assert!(
            plan.iter()
                .all(|setter| setter.tier() == super::super::Tier::Cheap),
            "the whole ProgressBar surface is frame-rate safe"
        );
    }

    #[test]
    fn the_value_is_kept_current_behind_a_spinner() {
        let bar = decode("\"value\":5,\"max\":10");
        let spinner = decode("\"value\":6,\"max\":10,\"indeterminate\":true");
        assert_eq!(
            ProgressProps::plan(&bar, &spinner),
            vec![Setter::Progress(6), Setter::Indeterminate(true)]
        );
    }
}
