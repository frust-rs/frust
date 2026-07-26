//! `Button` — a real `android.widget.Button`, built and driven from Rust.
//!
//! Six properties (PLAN 1.2's "the 5–8 things apps actually set"): caption,
//! enabled, text colour, background colour, text size, accessibility label.
//! The caption and the two layout-affecting properties are
//! [`Tier::Relayout`](super::Tier::Relayout); the rest are
//! [`Tier::Cheap`](super::Tier::Cheap) — see [`super`]'s tier table.
//!
//! Clicks are platform-owned: p1-05 attaches the shared
//! `dev.frust.FrustNativeListener` in
//! [`create`](crate::runtime::NativeWidget::create) and routes `onClick` into
//! the runtime's event dispatch. Nothing about that goes through
//! `RenderRoot::event` (the crate doc's event-bypass rule).

use super::{
    BACKGROUND_COLOR, CONTENT_DESCRIPTION, ENABLED, Plan, Setter, TEXT, TEXT_COLOR, TEXT_SIZE_SP,
    color, owned_text, plan_color, slot_of, text_or_empty,
};
use crate::NativeWidgetError;
use crate::registry::SlotId;
use crate::runtime::Params;

/// The registered kind string the api layer injects as
/// `__frustControl` (`crate::runtime`'s generic-factory contract).
pub(crate) const KIND: &str = "button";

/// The marker type registered under [`KIND`]; its
/// [`NativeWidget`](crate::runtime::NativeWidget) impl is the Android half
/// below.
pub(crate) struct Button;

/// Everything a `Button` slot can be told, as one Rust-diffed value.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ButtonProps {
    /// The differ's slot id. **Not a property** — no setter is ever planned
    /// for it; `create` needs it to construct the slot's listener (p1-05) and
    /// it cannot change for a live instance.
    pub(crate) slot: SlotId,
    /// The caption. Empty is legal (an icon-only button).
    pub(crate) text: String,
    /// `View.setEnabled`; absent params mean `true`, the platform default.
    pub(crate) enabled: bool,
    /// Packed ARGB text colour, or `None` to leave the theme's own.
    pub(crate) text_color: Option<i32>,
    /// Packed ARGB background fill, or `None` to keep the themed background
    /// (and its ripple — see [`Setter::BackgroundColor`]).
    pub(crate) background_color: Option<i32>,
    /// Text size in scale-independent pixels, or `None` for the platform's.
    pub(crate) text_size_sp: Option<f32>,
    /// The TalkBack label; `None` lets the platform fall back to the caption.
    pub(crate) content_description: Option<String>,
}

impl ButtonProps {
    /// The state a freshly constructed `new Button(context)` is already in —
    /// the baseline `create` diffs against, so a control that asked for the
    /// platform defaults performs **zero** setter calls
    /// ([`super`]'s two-halves note).
    pub(crate) fn platform_default(slot: SlotId) -> Self {
        Self {
            slot,
            text: String::new(),
            enabled: true,
            text_color: None,
            background_color: None,
            text_size_sp: None,
            content_description: None,
        }
    }

    /// Decode a `Button` slot's params.
    ///
    /// Every field but the identity is optional and falls back to the
    /// platform default, so a payload from an older or newer api layer
    /// degrades instead of killing the slot
    /// (`NativeWidget::decode_props`'s contract).
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
            background_color: color(params, BACKGROUND_COLOR),
            text_size_sp: params.float(TEXT_SIZE_SP).map(|size| size as f32),
            content_description: owned_text(params, CONTENT_DESCRIPTION),
        })
    }

    /// The setter-call plan for `old` → `new`: one entry per field that
    /// actually changed, in declaration order (the tests pin the order).
    pub(crate) fn plan<'a>(old: &Self, new: &'a Self) -> Plan<'a> {
        let mut plan = Plan::new();
        if old.text != new.text {
            plan.push(Setter::Text(&new.text));
        }
        if old.enabled != new.enabled {
            plan.push(Setter::Enabled(new.enabled));
        }
        plan_color(&mut plan, old.text_color, new.text_color, Setter::TextColor);
        plan_color(
            &mut plan,
            old.background_color,
            new.background_color,
            Setter::BackgroundColor,
        );
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
    //! The Android half: build the `Button` and hand its planned setters to
    //! [`crate::controls::platform`].

    use jni::objects::JObject;
    use jni::refs::Global;

    use super::{Button, ButtonProps};
    use crate::NativeWidgetError;
    use crate::android::{NativeCtx, NativeView};
    use crate::controls::platform::{FRAME_CAPACITY, apply_all};
    use crate::runtime::{NativeWidget, Params};

    /// `android.widget.Button` — the framework class, not a support/material
    /// one: this plugin never assumes an app dependency it did not ship.
    const CLASS: &str = "android.widget.Button";

    /// A live button's retained state.
    pub(crate) struct ButtonState {
        /// The button itself. This is a **second** global reference to the
        /// same object the runtime's [`NativeView`] holds: `update` is handed
        /// only the state, never the view, so the state has to own its own
        /// way back to the control. Both refs are released on dispose (this
        /// one when the state drops at the end of
        /// [`NativeWidget::dispose`]), so the leak bar is `2 × live controls`
        /// and still returns to zero.
        view: Global<JObject<'static>>,
    }

    impl NativeWidget for Button {
        type Props = ButtonProps;
        type State = ButtonState;

        fn decode_props(params: &Params<'_>) -> Result<Self::Props, NativeWidgetError> {
            ButtonProps::decode(params)
        }

        fn create(
            ctx: &mut NativeCtx<'_, '_>,
            props: &Self::Props,
        ) -> Result<(NativeView, Self::State), NativeWidgetError> {
            let view = ctx.new_view(CLASS)?;
            // p1-05 attaches `ctx.new_listener(props.slot)` here and retains
            // it in the handle's `extra` list, so the listener is
            // pair-deleted with the view.
            let plan = ButtonProps::plan(&ButtonProps::platform_default(props.slot), props);
            ctx.with_frame(FRAME_CAPACITY, |ctx| apply_all(ctx, &view, &plan))?;
            let handle = ctx.retain(&view)?;
            let retained = ctx.retain(&view)?;
            Ok((NativeView::new(handle), ButtonState { view: retained }))
        }

        fn update(
            ctx: &mut NativeCtx<'_, '_>,
            state: &mut Self::State,
            old: &Self::Props,
            new: &Self::Props,
        ) -> Result<(), NativeWidgetError> {
            let plan = ButtonProps::plan(old, new);
            ctx.with_frame(FRAME_CAPACITY, |ctx| apply_all(ctx, &state.view, &plan))
        }

        fn dispose(
            _ctx: &mut NativeCtx<'_, '_>,
            _state: Self::State,
        ) -> Result<(), NativeWidgetError> {
            // Nothing to detach in p1-04 (p1-05's listener detaches here);
            // dropping the state releases its global reference, and the
            // runtime drops the view's immediately afterwards — the paired
            // delete.
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::with_identity;

    fn params(body: &str) -> String {
        with_identity(KIND, 7, body)
    }

    fn decode(body: &str) -> ButtonProps {
        ButtonProps::decode(&Params::new(&params(body))).expect("decodes")
    }

    #[test]
    fn absent_fields_decode_to_the_platform_defaults() {
        let props = decode("");
        assert_eq!(props, ButtonProps::platform_default(7));
        // ...and therefore create plans nothing at all.
        assert!(ButtonProps::plan(&ButtonProps::platform_default(7), &props).is_empty());
    }

    #[test]
    fn the_create_plan_sets_exactly_the_non_default_fields() {
        let props = decode(
            "\"text\":\"Save\",\"enabled\":false,\"textColor\":-1,\"backgroundColor\":255,\
             \"textSizeSp\":18.5,\"contentDescription\":\"Save the note\"",
        );
        assert_eq!(
            ButtonProps::plan(&ButtonProps::platform_default(7), &props),
            vec![
                Setter::Text("Save"),
                Setter::Enabled(false),
                Setter::TextColor(-1),
                Setter::BackgroundColor(255),
                Setter::TextSizeSp(18.5),
                Setter::ContentDescription(Some("Save the note")),
            ]
        );
    }

    #[test]
    fn an_unchanged_field_never_reaches_the_platform() {
        let old = decode("\"text\":\"Save\",\"textColor\":16711680");
        let new = decode("\"text\":\"Saved\",\"textColor\":16711680");
        // Only the caption changed: the (35× more expensive) text setter runs
        // alone, and the colour is not re-applied.
        assert_eq!(ButtonProps::plan(&old, &new), vec![Setter::Text("Saved")]);
        assert_eq!(ButtonProps::plan(&new, &new), vec![]);
    }

    #[test]
    fn clearing_a_color_plans_nothing_but_clearing_a_label_does() {
        let with_color = decode("\"textColor\":123,\"contentDescription\":\"a\"");
        let without = decode("");
        // No platform call can restore a text colour...
        assert_eq!(
            ButtonProps::plan(&with_color, &without),
            vec![Setter::ContentDescription(None)]
        );
        // ...but re-applying one is a normal cheap setter.
        assert_eq!(
            ButtonProps::plan(&without, &with_color),
            vec![
                Setter::TextColor(123),
                Setter::ContentDescription(Some("a")),
            ]
        );
    }

    #[test]
    fn params_without_identity_are_a_params_error() {
        assert!(matches!(
            ButtonProps::decode(&Params::new("{\"text\":\"x\"}")),
            Err(NativeWidgetError::Params(_))
        ));
    }
}
