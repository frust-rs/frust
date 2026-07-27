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
    BACKGROUND_COLOR, CONTENT_DESCRIPTION, ENABLED, Plan, Setter, TEXT, TEXT_COLOR, TEXT_SIZE_SP,
    TYPEFACE, color, owned_text, plan_color, slot_of, text_or_empty,
};
use crate::NativeWidgetError;
use crate::controls::typeface::{self, Typeface};
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
    /// Packed ARGB background fill (theme ladder L2 followup, f1-01:
    /// `crate::api::theme::ResolvedTheme::surface_bg`), matching the page
    /// surface this `Label` sits on — EXPLICIT so it re-paints live on a
    /// brightness flip instead of pinning to whatever the platform's own
    /// default background resolved to under L1's creation-time
    /// night-qualified `Context` (`crate::android::theme`'s "Baked at
    /// construction, not live" note — the exact defect this field closes,
    /// `VERIFY-P1.md` bar 3). `None` leaves the platform's own default when
    /// no theme is threaded.
    pub(crate) background_color: Option<i32>,
    /// Text size in scale-independent pixels, or `None` for the platform's.
    pub(crate) text_size_sp: Option<f32>,
    /// The TalkBack label; `None` lets the platform read the text itself.
    pub(crate) content_description: Option<String>,
    /// Theme ladder L3 (p1-08): the resolved
    /// [`crate::api::theme::ResolvedTheme::body_typeface`], or
    /// [`Typeface::System`] when absent.
    pub(crate) typeface: Typeface,
}

impl LabelProps {
    /// The state a freshly constructed `new TextView(context)` is already in.
    pub(crate) fn platform_default(slot: SlotId) -> Self {
        Self {
            slot,
            text: String::new(),
            enabled: true,
            text_color: None,
            background_color: None,
            text_size_sp: None,
            content_description: None,
            typeface: Typeface::System,
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
            background_color: color(params, BACKGROUND_COLOR),
            text_size_sp: params.float(TEXT_SIZE_SP).map(|size| size as f32),
            content_description: owned_text(params, CONTENT_DESCRIPTION),
            typeface: typeface::decode(params, TYPEFACE),
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
        plan_color(
            &mut plan,
            old.background_color,
            new.background_color,
            Setter::BackgroundColor,
        );
        plan_color(&mut plan, old.text_color, new.text_color, Setter::TextColor);
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

#[cfg(target_os = "ios")]
pub(crate) mod platform {
    //! The Apple half: build the `UILabel` and apply the same planned setters
    //! the Android half hands JNI.
    //!
    //! # One construction-time normalization: `numberOfLines = 0`
    //!
    //! A fresh `UILabel` shows **one** line and truncates; a fresh Android
    //! `TextView` wraps to as many lines as its box allows. Neither is
    //! expressible as a [`Setter`](crate::controls::Setter) — no `Props` field
    //! names a line count — so leaving UIKit's own default would silently give
    //! the same `Label` different content on the two platforms. `create`
    //! therefore sets `numberOfLines = 0` (UIKit's "as many as needed") once,
    //! before the plan runs, bringing the fresh view into the behaviour
    //! `LabelProps` already describes rather than adding a property to it.
    //!
    //! This is the *shape* every control on this arm uses where UIKit's own
    //! constructor lands somewhere other than `Props::platform_default`
    //! describes: normalize in `create`, then diff against the shared default
    //! exactly as Android does (see `slider.rs`/`image.rs`, which normalize a
    //! real `Props` field rather than an unmodelled one).

    use objc2::MainThreadMarker;
    use objc2::rc::Retained;
    use objc2_foundation::NSString;
    use objc2_ui_kit::UILabel;

    use super::{KIND, Label, LabelProps};
    use crate::NativeWidgetError;
    use crate::apple::{NativeCtx, NativeView};
    use crate::controls::platform;
    use crate::controls::{Plan, Setter};
    use crate::runtime::{NativeWidget, Params};

    /// UIKit's "wrap to as many lines as the box allows" — the behaviour a
    /// fresh Android `TextView` already has (module doc).
    const UNLIMITED_LINES: isize = 0;

    /// A live label's retained state (see `button.rs`'s note on why one
    /// reference is enough on this arm).
    pub(crate) struct LabelState {
        view: Retained<UILabel>,
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
            let mtm = ctx.mtm();
            let view = UILabel::new(mtm);
            view.setNumberOfLines(UNLIMITED_LINES);
            let plan = LabelProps::plan(&LabelProps::platform_default(props.slot), props);
            apply_all(mtm, &view, &plan);
            let handle = NativeView::new(Retained::clone(&view).into_super(), mtm);
            Ok((handle, LabelState { view }))
        }

        fn update(
            ctx: &mut NativeCtx<'_, '_>,
            state: &mut Self::State,
            old: &Self::Props,
            new: &Self::Props,
        ) -> Result<(), NativeWidgetError> {
            apply_all(ctx.mtm(), &state.view, &LabelProps::plan(old, new));
            Ok(())
        }

        fn dispose(
            _ctx: &mut NativeCtx<'_, '_>,
            _state: Self::State,
        ) -> Result<(), NativeWidgetError> {
            // Display-only: nothing attached, and dropping the state releases
            // its retain.
            Ok(())
        }
    }

    /// Execute a whole [`Plan`], front to back.
    fn apply_all(mtm: MainThreadMarker, view: &UILabel, plan: &Plan<'_>) {
        for setter in plan {
            apply(mtm, view, setter);
        }
    }

    /// Execute one planned property write against `view`.
    fn apply(mtm: MainThreadMarker, view: &UILabel, setter: &Setter<'_>) {
        match *setter {
            Setter::Text(text) => view.setText(Some(&NSString::from_str(text))),
            // `UILabel` carries its own `isEnabled` (it dims the text) — it is
            // NOT the `UIControl` property the interactive controls set, since
            // a label is not a control at all.
            Setter::Enabled(enabled) => view.setEnabled(enabled),
            Setter::BackgroundColor(argb) => platform::set_background_color(view, argb),
            Setter::TextColor(argb) => {
                platform::set_label_text_color(view, &platform::ui_color(argb));
            }
            Setter::TextSizeSp(sp) => platform::set_label_font(view, &platform::system_font(sp)),
            Setter::Typeface(face) => platform::apply_typeface(face),
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

    // --- theme ladder L2 followup (f1-01): the explicit background setter --

    #[test]
    fn a_background_color_alone_plans_exactly_one_setter() {
        let old = decode("\"backgroundColor\":1");
        let new = decode("\"backgroundColor\":2");
        assert_eq!(
            LabelProps::plan(&old, &new),
            vec![Setter::BackgroundColor(2)]
        );
        assert_eq!(
            LabelProps::plan(&new, &new),
            vec![],
            "an unchanged background plans nothing — the zero-FFI property"
        );
    }

    #[test]
    fn clearing_a_background_plans_nothing_no_platform_restore_call_exists() {
        // Same shape as `TextColor`/`plan_color`'s own documented rule: an
        // int-taking colour setter has no "restore the platform default"
        // call, so a `Some` -> `None` transition plans nothing.
        let with_bg = decode("\"backgroundColor\":5");
        let without = decode("");
        assert_eq!(LabelProps::plan(&with_bg, &without), vec![]);
        assert_eq!(
            LabelProps::plan(&without, &with_bg),
            vec![Setter::BackgroundColor(5)]
        );
    }

    // --- theme ladder L3: the typeface setter, and Props gating ------------

    #[test]
    fn typeface_defaults_to_system_when_absent() {
        let props = decode("");
        assert_eq!(props.typeface, Typeface::System);
        assert_eq!(props, LabelProps::platform_default(3));
    }

    #[test]
    fn a_typeface_change_alone_plans_exactly_one_setter() {
        let old = decode("\"typeface\":\"system\"");
        let new = decode("\"typeface\":\"glyphPlex\"");
        assert_eq!(
            LabelProps::plan(&old, &new),
            vec![Setter::Typeface(Typeface::GlyphPlex)]
        );
        assert_eq!(
            LabelProps::plan(&new, &new),
            vec![],
            "an unchanged typeface plans nothing — the zero-FFI property"
        );
    }
}
