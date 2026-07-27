//! `Button` — a real `android.widget.Button`, built and driven from Rust.
//!
//! Six properties (PLAN 1.2's "the 5–8 things apps actually set"): caption,
//! enabled, text colour, background colour, text size, accessibility label.
//! The caption and the two layout-affecting properties are
//! [`Tier::Relayout`](super::Tier::Relayout); the rest are
//! [`Tier::Cheap`](super::Tier::Cheap) — see [`super`]'s tier table.
//!
//! Clicks are platform-owned: `create` attaches the shared
//! `dev.frust.FrustNativeListener` as an `OnClickListener` on Android, and a
//! `FrustNativeControlTarget` as the `TouchUpInside` action on iOS
//! (`crate::apple::events`, task p2-03); both arms'
//! [`on_event`](crate::runtime::NativeWidget::on_event) decode the firing via
//! the SAME [`crate::events::decode_click`] into the runtime's event
//! dispatch. Nothing about that goes through `RenderRoot::event` (the crate
//! doc's event-bypass rule) — a click is never caused by `update`'s own
//! setters, so unlike `Switch`/`Slider` this control needs no echo guard, on
//! either platform.

use super::{
    BACKGROUND_COLOR, CONTENT_DESCRIPTION, CORNER_RADIUS_DP, ENABLED, Plan, Setter, TEXT,
    TEXT_COLOR, TEXT_SIZE_SP, TYPEFACE, color, owned_text, plan_color, slot_of, text_or_empty,
};
use crate::NativeWidgetError;
use crate::controls::typeface::{self, Typeface};
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
    /// for it; `create` needs it to construct the slot's listener and it
    /// cannot change for a live instance.
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
    /// Corner radius, dp — theme ladder L2 (p1-07:
    /// `crate::api::theme::ResolvedTheme::corner_radius_dp`, folded in by
    /// `api::builders`). `Some` alongside a `Some` [`Self::background_color`]
    /// plans [`Setter::ThemedBackground`]; a background colour with no radius
    /// keeps the pre-p1-07 flat [`Setter::BackgroundColor`] path (see
    /// [`Self::plan`]).
    pub(crate) corner_radius_dp: Option<f32>,
    /// Theme ladder L3 (p1-08): the resolved
    /// [`crate::api::theme::ResolvedTheme::button_typeface`], or
    /// [`Typeface::System`] when absent (an older api layer, or a control
    /// built with no theme threaded).
    pub(crate) typeface: Typeface,
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
            corner_radius_dp: None,
            typeface: Typeface::System,
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
            corner_radius_dp: params.float(CORNER_RADIUS_DP).map(|radius| radius as f32),
            typeface: typeface::decode(params, TYPEFACE),
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
        plan_themed_background(&mut plan, old, new);
        if old.text_size_sp != new.text_size_sp
            && let Some(size) = new.text_size_sp
        {
            plan.push(Setter::TextSizeSp(size));
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

/// Plan [`Self::background_color`]/[`Self::corner_radius_dp`] together — the
/// two need ONE combined [`Setter::ThemedBackground`] whenever a corner
/// radius is present (theme ladder L2, p1-07), because Android has no "round
/// this `ColorDrawable`'s corners" call; a background colour with no radius
/// keeps the pre-p1-07 flat [`Setter::BackgroundColor`] (a possible future
/// explicit-colour-only override, and the exact plan every p1-04 test still
/// pins, since a payload with no `cornerRadiusDp` key decodes `None` on both
/// sides of any diff).
///
/// [`Self`]: ButtonProps
fn plan_themed_background<'a>(plan: &mut Plan<'a>, old: &ButtonProps, new: &'a ButtonProps) {
    if old.background_color == new.background_color && old.corner_radius_dp == new.corner_radius_dp
    {
        return;
    }
    match (new.background_color, new.corner_radius_dp) {
        (Some(fill), Some(radius_dp)) => plan.push(Setter::ThemedBackground { fill, radius_dp }),
        (Some(fill), None) => plan.push(Setter::BackgroundColor(fill)),
        (None, _) => {}
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
    use crate::events::{EventPayload, decode_click};
    use crate::runtime::{NativeEvent, NativeWidget, Params};

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
            let plan = ButtonProps::plan(&ButtonProps::platform_default(props.slot), props);
            ctx.with_frame(FRAME_CAPACITY, |ctx| apply_all(ctx, &view, &plan))?;
            // Attached after the initial plan, matching `Switch`/`Slider`'s
            // create order (`switch.rs`'s module doc) — a click setter never
            // fires anyway, but the same order keeps the three controls
            // predictable.
            let listener = ctx.new_listener(props.slot)?;
            ctx.set_on_click_listener(&view, &listener)?;
            let handle = ctx.retain(&view)?;
            let retained = ctx.retain(&view)?;
            let listener_ref = ctx.retain(&listener)?;
            Ok((
                NativeView::with_extra(handle, vec![listener_ref]),
                ButtonState { view: retained },
            ))
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

        fn on_event(_state: &mut Self::State, event: NativeEvent) -> Option<EventPayload> {
            decode_click(event)
        }

        fn dispose(
            ctx: &mut NativeCtx<'_, '_>,
            state: Self::State,
        ) -> Result<(), NativeWidgetError> {
            // Detach so a stray in-flight click can't fire after this slot's
            // instance is gone (tolerated either way —
            // `crate::runtime`'s late/duplicate disposal — but nothing keeps
            // the listener attached once this returns). Dropping `state`
            // releases its own global reference, and the runtime drops the
            // view's (and the listener's, retained in `extra`) immediately
            // afterwards — the paired delete.
            ctx.set_on_click_listener(&state.view, &JObject::null())
        }
    }
}

#[cfg(target_os = "ios")]
pub(crate) mod platform {
    //! The Apple half: build the `UIButton` and apply the same planned setters
    //! the Android half hands JNI.
    //!
    //! # `UIButtonType::System`, not a bare `UIButton::new`
    //!
    //! `ButtonProps::platform_default` describes *"the state a freshly
    //! constructed button is already in"*, and the create plan diffs against
    //! it — so which constructor runs decides which setters a default control
    //! pays for. `UIButton::new` yields `UIButtonType::Custom`: no title
    //! colour, no highlight behaviour, an invisible control until an app sets
    //! something. `buttonWithType(System)` yields UIKit's own button — the
    //! tinted title and press feedback a user recognizes — which is the honest
    //! analogue of Android's `new Button(context)` picking up the themed
    //! `Widget.Material.Button`.
    //!
    //! Together with the fill and title colour the theme ladder folds into
    //! `Props` (p1-07), that closes the Phase 0 spike's *"black box with
    //! text"* cosmetic gap: the spike hand-built a `Custom` button and got
    //! exactly that.
    //!
    //! # Clicks are platform-owned, exactly like Android's
    //!
    //! `create` attaches a [`FrustNativeControlTarget`] as the button's
    //! `TouchUpInside` action (task p2-03), and `on_event` decodes its firing
    //! via [`crate::events::decode_click`] into the runtime's event dispatch —
    //! the same decoder Android's `on_event` calls, so kind/detail parity is
    //! automatic (`crate::apple::events`'s module doc). Nothing about that
    //! goes through `RenderRoot::event` (the crate doc's event-bypass rule) —
    //! a click is never caused by `update`'s own setters, so unlike
    //! `Switch`/`Slider` this control needs no echo guard, on either
    //! platform.

    use objc2::MainThreadMarker;
    use objc2::rc::Retained;
    use objc2_foundation::NSString;
    use objc2_ui_kit::{UIButton, UIButtonType, UIControlState};

    use super::{Button, ButtonProps, KIND};
    use crate::NativeWidgetError;
    use crate::apple::{FrustNativeControlTarget, NativeCtx, NativeView};
    use crate::controls::platform;
    use crate::controls::{Plan, Setter};
    use crate::events::{EventPayload, decode_click};
    use crate::runtime::{NativeEvent, NativeWidget, Params};

    /// A live button's retained state.
    ///
    /// Two `Retained` fields, where the Android state holds two: one is the
    /// same "`update` needs its own reference beside the runtime's" reason
    /// (`button.rs`'s Android note) — except here ARC makes that extra retain
    /// free and untracked, with no paired-delete discipline to keep. The
    /// second, `target`, is the one retention this arm genuinely needs
    /// explicitly: `UIControl` holds its target-action pair **weakly**, so
    /// nothing but this field keeps the target alive for the slot's lifetime
    /// (`crate::apple::events`'s module doc's *Target retention*).
    pub(crate) struct ButtonState {
        /// The button, kept typed: `update` needs `UIButton`'s own
        /// state-keyed setters, which a `UIView` handle could not reach.
        view: Retained<UIButton>,
        /// The target-action object `create` attached as `view`'s
        /// `TouchUpInside` action. Retained here only; dropped (and thereby
        /// released) alongside the rest of `State` when
        /// `Instance::dispose` tears this slot down.
        target: Retained<FrustNativeControlTarget>,
        /// Theme ladder L3 (p2-04): the combined typeface/size state
        /// `Setter::TextSizeSp`/`Setter::Typeface` share — see
        /// `crate::controls::platform::FontState`'s doc for why a `Button`
        /// needs this at all.
        font: platform::FontState,
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
            let mtm = ctx.mtm();
            let view = UIButton::buttonWithType(UIButtonType::System, mtm);
            let mut font = platform::FontState::platform_default();
            let plan = ButtonProps::plan(&ButtonProps::platform_default(props.slot), props);
            apply_all(mtm, &view, &mut font, &plan);
            // Attached after the initial plan, matching the Android arm's
            // create order (`switch.rs`'s module doc) — a click setter never
            // fires anyway, but the same order keeps the three controls
            // predictable.
            let target = FrustNativeControlTarget::attach_button(mtm, props.slot, &view);
            let handle = NativeView::new(Retained::clone(&view).into_super().into_super(), mtm);
            Ok((handle, ButtonState { view, target, font }))
        }

        fn update(
            ctx: &mut NativeCtx<'_, '_>,
            state: &mut Self::State,
            old: &Self::Props,
            new: &Self::Props,
        ) -> Result<(), NativeWidgetError> {
            apply_all(
                ctx.mtm(),
                &state.view,
                &mut state.font,
                &ButtonProps::plan(old, new),
            );
            Ok(())
        }

        fn on_event(_state: &mut Self::State, event: NativeEvent) -> Option<EventPayload> {
            decode_click(event)
        }

        fn dispose(
            _ctx: &mut NativeCtx<'_, '_>,
            state: Self::State,
        ) -> Result<(), NativeWidgetError> {
            // Detach so a stray in-flight click can't fire after this slot's
            // instance is gone (tolerated either way —
            // `crate::runtime`'s late/duplicate disposal — mirroring the
            // Android arm's explicit `setOnClickListener(null)`). Dropping
            // `state` afterwards releases the target's own retain — ARC's
            // own paired delete.
            state.target.detach_button(&state.view);
            Ok(())
        }
    }

    /// Execute a whole [`Plan`], front to back — the same order contract the
    /// Android arm's `apply_all` keeps.
    fn apply_all(
        mtm: MainThreadMarker,
        view: &UIButton,
        font: &mut platform::FontState,
        plan: &Plan<'_>,
    ) {
        for setter in plan {
            apply(mtm, view, font, setter);
        }
    }

    /// Execute one planned property write against `view`.
    fn apply(
        mtm: MainThreadMarker,
        view: &UIButton,
        font: &mut platform::FontState,
        setter: &Setter<'_>,
    ) {
        match *setter {
            // A `UIButton`'s caption is per-control-state, unlike a
            // `TextView`'s: `Normal` is the base every other state falls back
            // to, so setting it alone is what "the button's title" means.
            Setter::Text(text) => {
                view.setTitle_forState(Some(&NSString::from_str(text)), UIControlState::Normal);
            }
            Setter::Enabled(enabled) => view.setEnabled(enabled),
            Setter::TextColor(argb) => {
                view.setTitleColor_forState(
                    Some(&platform::ui_color(argb)),
                    UIControlState::Normal,
                );
            }
            Setter::BackgroundColor(argb) => platform::set_background_color(view, argb),
            Setter::TextSizeSp(sp) => {
                let resolved = font.apply_size(sp);
                // `titleLabel` is documented non-null for every stock button
                // type, but the binding is `Option`-typed, so a missing one
                // degrades to "no size change" rather than a panic — the
                // combined-font state above still records the size for a
                // later combining apply.
                if let Some(label) = view.titleLabel() {
                    platform::set_label_font(&label, resolved.as_ui_font());
                }
            }
            Setter::ThemedBackground { fill, radius_dp } => {
                platform::set_background_color(view, fill);
                platform::set_corner_radius(view, radius_dp);
            }
            Setter::Typeface(face) => {
                let resolved = font.apply_typeface(face);
                if let Some(label) = view.titleLabel() {
                    platform::set_label_font(&label, resolved.as_ui_font());
                }
            }
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

    // --- theme ladder L2: the combined themed-background setter ------------

    #[test]
    fn a_fill_with_a_radius_plans_the_combined_themed_background() {
        let props = decode("\"backgroundColor\":255,\"cornerRadiusDp\":6.0");
        assert_eq!(props.corner_radius_dp, Some(6.0));
        assert_eq!(
            ButtonProps::plan(&ButtonProps::platform_default(7), &props),
            vec![Setter::ThemedBackground {
                fill: 255,
                radius_dp: 6.0,
            }]
        );
    }

    #[test]
    fn a_fill_with_no_radius_keeps_the_pre_p1_07_flat_setter() {
        // No `cornerRadiusDp` key at all: exactly the p1-04 shape, still the
        // plan a future explicit-colour-only override takes.
        let props = decode("\"backgroundColor\":255");
        assert_eq!(props.corner_radius_dp, None);
        assert_eq!(
            ButtonProps::plan(&ButtonProps::platform_default(7), &props),
            vec![Setter::BackgroundColor(255)]
        );
    }

    #[test]
    fn a_radius_change_alone_replans_the_themed_background() {
        let old = decode("\"backgroundColor\":255,\"cornerRadiusDp\":6.0");
        let new = decode("\"backgroundColor\":255,\"cornerRadiusDp\":10.0");
        assert_eq!(
            ButtonProps::plan(&old, &new),
            vec![Setter::ThemedBackground {
                fill: 255,
                radius_dp: 10.0,
            }]
        );
        assert_eq!(ButtonProps::plan(&new, &new), vec![]);
    }

    #[test]
    fn clearing_a_radius_back_to_a_flat_fill_falls_back_to_background_color() {
        let themed = decode("\"backgroundColor\":255,\"cornerRadiusDp\":6.0");
        let flat = decode("\"backgroundColor\":255");
        assert_eq!(
            ButtonProps::plan(&themed, &flat),
            vec![Setter::BackgroundColor(255)]
        );
    }

    // --- theme ladder L3: the typeface setter, and Props gating ------------

    #[test]
    fn typeface_defaults_to_system_when_absent() {
        let props = decode("");
        assert_eq!(props.typeface, Typeface::System);
        assert_eq!(props, ButtonProps::platform_default(7));
    }

    #[test]
    fn a_typeface_change_alone_plans_exactly_one_setter() {
        let old = decode("\"typeface\":\"glyphPlex\"");
        let new = decode("\"typeface\":\"glyphMono\"");
        assert_eq!(
            ButtonProps::plan(&old, &new),
            vec![Setter::Typeface(Typeface::GlyphMono)]
        );
    }

    #[test]
    fn an_unchanged_typeface_plans_nothing_the_zero_ffi_property() {
        let props = decode("\"typeface\":\"glyphMono\"");
        assert_eq!(
            ButtonProps::plan(&props, &props),
            vec![],
            "identical typeface Props must plan no setter — the whole-struct \
             PartialEq gate this crate's `Props: PartialEq` contract relies on"
        );
    }
}
