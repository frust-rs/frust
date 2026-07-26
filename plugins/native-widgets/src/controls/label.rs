//! `Label` — a real `android.widget.TextView`, built and driven from Rust.
//!
//! Five properties: text, enabled, text colour, text size, accessibility
//! label. Display-only — it emits no events on any platform, so its
//! [`NativeWidget::on_event`](crate::runtime::NativeWidget::on_event) stays
//! the trait's default no-op and it carries no slot-id-bearing listener.
//!
//! Two of its five setters are [`Tier::Relayout`](super::Tier::Relayout)
//! (`setText`, `setTextSize` — ~29 µs, ~35× a colour set), which makes this
//! the control most likely to be misused on a per-frame path: a streaming
//! readout should stream a **colour** and leave the text alone, or accept a
//! re-layout per update (see [`super`]'s per-frame guidance).

use super::{
    CONTENT_DESCRIPTION, ENABLED, Plan, Setter, TEXT, TEXT_COLOR, TEXT_SIZE_SP, color, owned_text,
    plan_color, slot_of, text_or_empty,
};
use crate::NativeWidgetError;
use crate::registry::SlotId;
use crate::runtime::Params;

/// The registered kind string the api layer injects as `__frustControl`.
pub(crate) const KIND: &str = "label";

/// The marker type registered under [`KIND`]; its
/// [`NativeWidget`](crate::runtime::NativeWidget) impl is the Android half
/// below.
pub(crate) struct Label;

/// Everything a `Label` slot can be told, as one Rust-diffed value.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LabelProps {
    /// The differ's slot id — **not a property**, see
    /// [`ButtonProps::slot`](super::button::ButtonProps::slot).
    pub(crate) slot: SlotId,
    /// The text.
    pub(crate) text: String,
    /// `View.setEnabled` — a `TextView` renders its disabled state through
    /// the colour state list, so this is visible even without interaction.
    pub(crate) enabled: bool,
    /// Packed ARGB text colour, or `None` to leave the theme's own.
    pub(crate) text_color: Option<i32>,
    /// Text size in scale-independent pixels, or `None` for the platform's.
    pub(crate) text_size_sp: Option<f32>,
    /// The TalkBack label; `None` lets the platform read the text itself.
    pub(crate) content_description: Option<String>,
}

impl LabelProps {
    /// The state a freshly constructed `new TextView(context)` is already in.
    pub(crate) fn platform_default(slot: SlotId) -> Self {
        Self {
            slot,
            text: String::new(),
            enabled: true,
            text_color: None,
            text_size_sp: None,
            content_description: None,
        }
    }

    /// Decode a `Label` slot's params (same degrade-don't-fail rule as
    /// [`ButtonProps::decode`](super::button::ButtonProps::decode)).
    ///
    /// # Errors
    /// [`NativeWidgetError::Params`] when the reserved identity keys are
    /// missing.
    pub(crate) fn decode(params: &Params<'_>) -> Result<Self, NativeWidgetError> {
        Ok(Self {
            slot: slot_of(params)?,
            text: text_or_empty(params, TEXT),
            enabled: params.flag(ENABLED).unwrap_or(true),
            text_color: color(params, TEXT_COLOR),
            text_size_sp: params.float(TEXT_SIZE_SP).map(|size| size as f32),
            content_description: owned_text(params, CONTENT_DESCRIPTION),
        })
    }

    /// The setter-call plan for `old` → `new`, in declaration order.
    pub(crate) fn plan<'a>(old: &Self, new: &'a Self) -> Plan<'a> {
        let mut plan = Plan::new();
        if old.text != new.text {
            plan.push(Setter::Text(&new.text));
        }
        if old.enabled != new.enabled {
            plan.push(Setter::Enabled(new.enabled));
        }
        plan_color(&mut plan, old.text_color, new.text_color, Setter::TextColor);
        if old.text_size_sp != new.text_size_sp
            && let Some(size) = new.text_size_sp
        {
            plan.push(Setter::TextSizeSp(size));
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
    //! The Android half: build the `TextView` and hand its planned setters to
    //! [`crate::controls::platform`].

    use jni::objects::JObject;
    use jni::refs::Global;

    use super::{Label, LabelProps};
    use crate::NativeWidgetError;
    use crate::android::{NativeCtx, NativeView};
    use crate::controls::platform::{FRAME_CAPACITY, apply_all};
    use crate::runtime::{NativeWidget, Params};

    /// `android.widget.TextView` — the framework class.
    const CLASS: &str = "android.widget.TextView";

    /// A live label's retained state.
    pub(crate) struct LabelState {
        /// The label's own global reference. A **second** ref to the object
        /// the runtime's [`NativeView`] already holds, because `update` is
        /// handed only the state (see `button.rs`'s `ButtonState` for the
        /// full note); both are released on dispose.
        view: Global<JObject<'static>>,
    }

    impl NativeWidget for Label {
        type Props = LabelProps;
        type State = LabelState;

        fn decode_props(params: &Params<'_>) -> Result<Self::Props, NativeWidgetError> {
            LabelProps::decode(params)
        }

        fn create(
            ctx: &mut NativeCtx<'_, '_>,
            props: &Self::Props,
        ) -> Result<(NativeView, Self::State), NativeWidgetError> {
            let view = ctx.new_view(CLASS)?;
            let plan = LabelProps::plan(&LabelProps::platform_default(props.slot), props);
            ctx.with_frame(FRAME_CAPACITY, |ctx| apply_all(ctx, &view, &plan))?;
            let handle = ctx.retain(&view)?;
            let retained = ctx.retain(&view)?;
            Ok((NativeView::new(handle), LabelState { view: retained }))
        }

        fn update(
            ctx: &mut NativeCtx<'_, '_>,
            state: &mut Self::State,
            old: &Self::Props,
            new: &Self::Props,
        ) -> Result<(), NativeWidgetError> {
            let plan = LabelProps::plan(old, new);
            ctx.with_frame(FRAME_CAPACITY, |ctx| apply_all(ctx, &state.view, &plan))
        }

        fn dispose(
            _ctx: &mut NativeCtx<'_, '_>,
            _state: Self::State,
        ) -> Result<(), NativeWidgetError> {
            // Display-only: nothing attached, and dropping the state releases
            // its global reference.
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::with_identity;

    fn decode(body: &str) -> LabelProps {
        let raw = with_identity(KIND, 3, body);
        LabelProps::decode(&Params::new(&raw)).expect("decodes")
    }

    #[test]
    fn absent_fields_decode_to_the_platform_defaults_and_plan_nothing() {
        let props = decode("");
        assert_eq!(props, LabelProps::platform_default(3));
        assert!(LabelProps::plan(&LabelProps::platform_default(3), &props).is_empty());
    }

    #[test]
    fn the_create_plan_sets_exactly_the_non_default_fields() {
        let props = decode(
            "\"text\":\"42 fps\",\"enabled\":false,\"textColor\":16777215,\"textSizeSp\":11.0,\
             \"contentDescription\":\"frame rate\"",
        );
        assert_eq!(
            LabelProps::plan(&LabelProps::platform_default(3), &props),
            vec![
                Setter::Text("42 fps"),
                Setter::Enabled(false),
                Setter::TextColor(16_777_215),
                Setter::TextSizeSp(11.0),
                Setter::ContentDescription(Some("frame rate")),
            ]
        );
    }

    #[test]
    fn a_colour_only_change_never_pays_for_the_relayout_setters() {
        let old = decode("\"text\":\"42 fps\",\"textColor\":1");
        let new = decode("\"text\":\"42 fps\",\"textColor\":2");
        let plan = LabelProps::plan(&old, &new);
        assert_eq!(plan, vec![Setter::TextColor(2)]);
        assert!(
            plan.iter()
                .all(|setter| setter.tier() == super::super::Tier::Cheap)
        );
    }
}
