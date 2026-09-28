//! `Spinner` — a real indeterminate activity indicator, built and driven
//! from Rust: `android.widget.ProgressBar` constructed indeterminate (the
//! same JNI construction path as `progress.rs`, a different stock style),
//! `UIActivityIndicatorView` on iOS, and an `NSProgressIndicator` in its
//! `Spinning` style on macOS — the sanctioned cross-platform route for "give
//! me an indeterminate spinner" (`progress.rs`'s own Apple module doc points
//! here; that control's `Setter::Indeterminate` stays a documented no-op on
//! iOS rather than growing a second view internally).
//!
//! Five properties: animating, size class, tint, accessibility label,
//! enabled — every one of them [`Tier::Cheap`](super::Tier::Cheap) except
//! [`Setter::SizeClass`] ([`Tier::Relayout`]: a style/`controlSize` swap
//! re-measures the view). Display-only: it emits no events, so its
//! [`on_event`](crate::runtime::NativeWidget::on_event) stays the trait's
//! default no-op.
//!
//! # `size_class`: baked at construction on Android, live everywhere else
//!
//! Android has no live call at all: an indeterminate `ProgressBar`'s spinner
//! drawable is resolved from the stock style
//! (`android.R.attr.progressBarStyleSmall`/`progressBarStyle`/
//! `progressBarStyleLarge`) it was **constructed** with — there is no
//! `setSpinnerSize` to swap it later without rebuilding the view. So
//! [`platform::create`](self) on that arm reads `props.size_class` directly
//! to pick the constructor style (mirroring `progress.rs`'s own *why this
//! control does not use the one-argument constructor* — construction
//! tailored to what the first `create` actually needs, not a fixed
//! default), and [`SpinnerProps::plan`] still compares `old.size_class` to
//! `new.size_class` like any other field: a size actually requested at
//! create time therefore plans nothing (already achieved), while a genuine
//! *later* change reaches [`Setter::SizeClass`] and is warned-and-ignored on
//! this one arm (`android::platform::warn_size_class_unsupported`,
//! `crate::controls::mod`'s Android `platform`) — this crate's
//! documented-per-platform-gap policy
//! (`docs/PLUGINS_CODE_STANDARDS.md`'s "a platform capability gap is
//! recorded per-platform, never corrected onto the platform that doesn't
//! have it"), not a bug.
//!
//! `UIActivityIndicatorView.style` and `NSProgressIndicator.controlSize` are
//! both genuinely live properties, so the two Apple arms need no such
//! trick: `create` seeds the view at [`SizeClass::Medium`] (the shared
//! [`SpinnerProps::platform_default`]) and lets the ordinary diffed plan
//! apply [`Setter::SizeClass`] like any other property, on create and on
//! every later change alike.
//!
//! iOS has no small style of its own — `UIActivityIndicatorView.Style`
//! offers only `.medium`/`.large` (the `.white`/`.whiteLarge`/`.gray`
//! constants are deprecated pre-iOS-13 aliases, not a third size) — so
//! [`SizeClass::Small`] degrades to the same style as [`SizeClass::Medium`]
//! on that one arm ([`SizeClass::activity_indicator_style`]); macOS maps all
//! three sizes exactly (`NSControlSize::Small`/`Regular`/`Large`), and
//! Android does too, at construction.
//!
//! # `animating` maps to visibility on Android
//!
//! Android's indeterminate `ProgressBar` has no `start`/`stop` call at all —
//! its spinner drawable's own animation callback already runs only while the
//! view is visible and attached to a window. So [`Setter::Animating`] maps
//! to `View.setVisibility(VISIBLE)`/`View.setVisibility(INVISIBLE)` on that
//! arm — `INVISIBLE`, not `GONE`, so the slot keeps its layout space rather
//! than collapsing it — which lands the view in exactly the "stopped and
//! hidden" state `UIActivityIndicatorView.hidesWhenStopped`/
//! `NSProgressIndicator.setDisplayedWhenStopped(false)` already give the
//! other two arms for free around their own `startAnimating`/
//! `stopAnimating` and `startAnimation:`/`stopAnimation:` calls. Because
//! Android's initial visibility has to be right from the very first frame
//! (there is no later setter call that corrects it — `View.setVisibility`
//! genuinely is that setter, it simply also has to run once up front),
//! [`platform::create`](self) on that arm sets it directly from
//! `props.animating` rather than diffing a fixed default through, the same
//! shape [`SizeClass`] above uses.
//!
//! [`SpinnerProps::platform_default`]'s shared `animating: false` matches a
//! freshly constructed `UIActivityIndicatorView`
//! (`hidesWhenStopped(true)`, not yet started) and a freshly constructed
//! `NSProgressIndicator` (`displayedWhenStopped(false)`, not yet started)
//! with no further normalization on either Apple arm — only Android's
//! auto-starting spinner needs one (module doc above).
//!
//! # `tint`: `accent_ink`, where the platform exposes one
//!
//! `crate::api::theme` folds the theme's `accent_ink` role into this
//! control's `tint` field (the same accent-ink role `Switch`/`Slider`'s
//! thumb tint already reads — see that module's mapping table).
//! `UIActivityIndicatorView.color` and Android's
//! `ProgressBar.setIndeterminateTintList` both honour it; `NSProgressIndicator`
//! exposes no tint property at all (the same gap `progress.rs`'s macOS arm
//! already documents for its own `ProgressTint`) — logged at debug and
//! no-op'd, not faked, through the shared `platform::set_tint` helper.
//!
//! # `enabled` has no UIKit/AppKit equivalent
//!
//! `View.setEnabled` is a plain `android.view.View` method, so it applies to
//! *any* Android control including a display-only one — Android genuinely
//! honours it (dims certain drawable states, and TalkBack announces a
//! disabled control differently). `UIActivityIndicatorView` and
//! `NSProgressIndicator` are bare `UIView`/`NSView` subclasses, not
//! `UIControl`/`NSControl` ones, so neither has an `enabled` property at
//! all — both Apple arms silently ignore [`Setter::Enabled`], the exact
//! treatment `switch.rs`'s iOS arm already gives its own unusable
//! `Setter::Typeface`.

use super::{CONTENT_DESCRIPTION, ENABLED, Plan, Setter, TINT, color, owned_text, slot_of};
use crate::NativeWidgetError;
use crate::registry::SlotId;
use crate::runtime::Params;

/// The registered kind string the api layer injects as `__frustControl`.
pub(crate) const KIND: &str = "spinner";

/// `"animating"` — whether the spinner is currently spinning.
pub(crate) const ANIMATING: &str = "animating";
/// `"sizeClass"` — [`SizeClass`]'s wire spelling.
pub(crate) const SIZE_CLASS: &str = "sizeClass";

/// The marker type registered under [`KIND`]; its
/// [`NativeWidget`](crate::runtime::NativeWidget) impl is the Android half
/// below.
pub(crate) struct Spinner;

/// How large the spinner renders — see the module doc's *`size_class`*
/// section for which arm can change this live.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum SizeClass {
    /// The smallest stock size. Degrades to [`Self::Medium`] on iOS (module
    /// doc) — UIKit has no third style.
    Small,
    /// The platform's own default circular spinner size.
    #[default]
    Medium,
    /// The largest stock size.
    Large,
}

impl SizeClass {
    /// The wire spelling the api layer writes into [`SIZE_CLASS`].
    fn from_wire(raw: &str) -> Option<Self> {
        match raw {
            "small" => Some(Self::Small),
            "medium" => Some(Self::Medium),
            "large" => Some(Self::Large),
            _ => None,
        }
    }

    /// The `android.R.attr` name naming the stock `ProgressBar` style this
    /// size constructs (module doc's *`size_class`* section — construction
    /// only, no live setter).
    #[cfg(target_os = "android")]
    pub(crate) fn android_style_attr(self) -> &'static jni::strings::JNIStr {
        use jni::jni_str;
        match self {
            Self::Small => jni_str!("progressBarStyleSmall"),
            Self::Medium => jni_str!("progressBarStyle"),
            Self::Large => jni_str!("progressBarStyleLarge"),
        }
    }

    /// The `UIActivityIndicatorView.Style` this size selects — degrading
    /// [`Self::Small`] to [`Self::Medium`]'s style (module doc: UIKit has no
    /// third size).
    #[cfg(target_os = "ios")]
    pub(crate) fn activity_indicator_style(self) -> objc2_ui_kit::UIActivityIndicatorViewStyle {
        use objc2_ui_kit::UIActivityIndicatorViewStyle;
        match self {
            Self::Small | Self::Medium => UIActivityIndicatorViewStyle::Medium,
            Self::Large => UIActivityIndicatorViewStyle::Large,
        }
    }

    /// The `NSControlSize` this size selects — an exact, non-degrading
    /// mapping (unlike the iOS arm above).
    #[cfg(target_os = "macos")]
    pub(crate) fn control_size(self) -> objc2_app_kit::NSControlSize {
        use objc2_app_kit::NSControlSize;
        match self {
            Self::Small => NSControlSize::Small,
            Self::Medium => NSControlSize::Regular,
            Self::Large => NSControlSize::Large,
        }
    }
}

/// Everything a `Spinner` slot can be told, as one Rust-diffed value.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SpinnerProps {
    /// The differ's slot id — **not a property**. Carried for symmetry with
    /// the other display-only controls; no listener is ever attached.
    pub(crate) slot: SlotId,
    /// Whether the spinner is currently animating — module doc's `animating`
    /// section.
    pub(crate) animating: bool,
    /// The spinner's size — module doc's `size_class` section.
    pub(crate) size_class: SizeClass,
    /// Packed ARGB tint, or `None` to draw the platform's own colour.
    pub(crate) tint: Option<i32>,
    /// `View.setEnabled` — Android-only (module doc's *`enabled`* section).
    pub(crate) enabled: bool,
    /// The TalkBack/VoiceOver label.
    pub(crate) content_description: Option<String>,
}

impl SpinnerProps {
    /// The state every arm's `create` diffs its first plan against: not yet
    /// animating, [`SizeClass::Medium`], no tint, enabled, no label — see
    /// the module doc's `animating`/`size_class` sections for how each arm
    /// reaches this state at construction.
    pub(crate) fn platform_default(slot: SlotId) -> Self {
        Self {
            slot,
            animating: false,
            size_class: SizeClass::Medium,
            tint: None,
            enabled: true,
            content_description: None,
        }
    }

    /// Decode a `Spinner` slot's params.
    ///
    /// # Errors
    /// [`NativeWidgetError::Params`] when the reserved identity keys are
    /// missing.
    pub(crate) fn decode(params: &Params<'_>) -> Result<Self, NativeWidgetError> {
        Ok(Self {
            slot: slot_of(params)?,
            animating: params.flag(ANIMATING).unwrap_or(false),
            size_class: params
                .string(SIZE_CLASS)
                .and_then(|raw| SizeClass::from_wire(&raw))
                .unwrap_or_default(),
            tint: color(params, TINT),
            enabled: params.flag(ENABLED).unwrap_or(true),
            content_description: owned_text(params, CONTENT_DESCRIPTION),
        })
    }

    /// The setter-call plan for `old` → `new`, in declaration order.
    pub(crate) fn plan<'a>(old: &Self, new: &'a Self) -> Plan<'a> {
        let mut plan = Plan::new();
        if old.size_class != new.size_class {
            plan.push(Setter::SizeClass(new.size_class));
        }
        if old.animating != new.animating {
            plan.push(Setter::Animating(new.animating));
        }
        if old.tint != new.tint {
            plan.push(Setter::SpinnerTint(new.tint));
        }
        if old.content_description != new.content_description {
            plan.push(Setter::ContentDescription(
                new.content_description.as_deref(),
            ));
        }
        if old.enabled != new.enabled {
            plan.push(Setter::Enabled(new.enabled));
        }
        plan
    }
}

#[cfg(target_os = "android")]
pub(crate) mod platform {
    //! The Android half: build the `ProgressBar` at the requested spinner
    //! size and hand its planned setters to [`crate::controls::platform`].

    use jni::objects::{JObject, JValue};
    use jni::refs::Global;
    use jni::{jni_sig, jni_str};

    use super::{Spinner, SpinnerProps};
    use crate::NativeWidgetError;
    use crate::android::{NativeCtx, NativeView};
    use crate::controls::platform::{FRAME_CAPACITY, apply_all};
    use crate::runtime::{NativeWidget, Params};

    /// `android.widget.ProgressBar` — the framework class, same as
    /// `progress.rs`'s.
    const CLASS: &str = "android.widget.ProgressBar";

    /// `android.R$attr` — see `progress.rs`'s own `R_ATTR_CLASS` for why the
    /// style attribute is read from the framework's own `R.attr` rather than
    /// hardcoded.
    const R_ATTR_CLASS: &str = "android.R$attr";

    /// `View.VISIBLE`.
    const VIEW_VISIBLE: i32 = 0;
    /// `View.INVISIBLE` — keeps the slot's own layout space, unlike `GONE`
    /// (module doc's *`animating`* section).
    const VIEW_INVISIBLE: i32 = 4;

    /// `new ProgressBar(context, null, <size_class's stock style>)` — module
    /// doc's *`size_class`* section: the size is a construction-time choice
    /// on this arm, so it is read directly from `props` rather than diffed
    /// through a fixed default the way every other field is.
    fn new_spinner<'local>(
        ctx: &mut NativeCtx<'local, '_>,
        size_class: super::SizeClass,
    ) -> Result<JObject<'local>, NativeWidgetError> {
        let attrs = ctx.class(R_ATTR_CLASS)?;
        let attr_name = size_class.android_style_attr();
        let style = ctx.run_jni(
            &format!("android.R.attr.<spinner style {size_class:?}>"),
            |env| env.get_static_field(&attrs, attr_name, jni_sig!("I"))?.i(),
        )?;
        let class = ctx.class(CLASS)?;
        let context = ctx.context()?;
        let view = ctx.run_jni("new ProgressBar(Context, AttributeSet, int)", |env| {
            env.new_object(
                &class,
                jni_sig!("(Landroid/content/Context;Landroid/util/AttributeSet;I)V"),
                &[
                    JValue::Object(context),
                    JValue::Object(&JObject::null()),
                    JValue::Int(style),
                ],
            )
        })?;
        // Explicit, not relying on the stock style's own indeterminate
        // default — this crate's habit of never trusting an implicit
        // platform default (`progress.rs`'s module doc's *Construction-time
        // normalization*), even though every stock circular style already
        // defaults to indeterminate-only.
        ctx.call_void(
            &view,
            jni_str!("setIndeterminate"),
            jni_sig!("(Z)V"),
            &[JValue::Bool(true)],
        )?;
        Ok(view)
    }

    /// A live spinner's retained state.
    pub(crate) struct SpinnerState {
        /// The bar's own global reference (the second one — see
        /// `button.rs`).
        view: Global<JObject<'static>>,
    }

    impl NativeWidget for Spinner {
        type Props = SpinnerProps;
        type State = SpinnerState;

        fn decode_props(params: &Params<'_>) -> Result<Self::Props, NativeWidgetError> {
            SpinnerProps::decode(params)
        }

        fn create(
            ctx: &mut NativeCtx<'_, '_>,
            props: &Self::Props,
        ) -> Result<(NativeView, Self::State), NativeWidgetError> {
            let view = new_spinner(ctx, props.size_class)?;
            // Module doc's *`animating`* section: the initial visibility has
            // to be right from the first frame, so it is read directly from
            // `props` rather than diffed through a fixed default.
            ctx.call_void(
                &view,
                jni_str!("setVisibility"),
                jni_sig!("(I)V"),
                &[JValue::Int(if props.animating {
                    VIEW_VISIBLE
                } else {
                    VIEW_INVISIBLE
                })],
            )?;
            // Both construction-time-only fields are already achieved above,
            // so the diffed plan below must not re-report them as changes —
            // land the shared default on exactly what was just built
            // (mirrors `progress.rs`'s own *why this control does not use
            // the one-argument constructor* shape).
            let mut default = SpinnerProps::platform_default(props.slot);
            default.size_class = props.size_class;
            default.animating = props.animating;
            let plan = SpinnerProps::plan(&default, props);
            ctx.with_frame(FRAME_CAPACITY, |ctx| apply_all(ctx, &view, &plan))?;
            let handle = ctx.retain(&view)?;
            let retained = ctx.retain(&view)?;
            Ok((NativeView::new(handle), SpinnerState { view: retained }))
        }

        fn update(
            ctx: &mut NativeCtx<'_, '_>,
            state: &mut Self::State,
            old: &Self::Props,
            new: &Self::Props,
        ) -> Result<(), NativeWidgetError> {
            let plan = SpinnerProps::plan(old, new);
            ctx.with_frame(FRAME_CAPACITY, |ctx| apply_all(ctx, &state.view, &plan))
        }

        fn dispose(
            _ctx: &mut NativeCtx<'_, '_>,
            _state: Self::State,
        ) -> Result<(), NativeWidgetError> {
            // Display-only: nothing attached; dropping the state releases
            // its global reference.
            Ok(())
        }
    }
}

#[cfg(target_os = "ios")]
pub(crate) mod platform {
    //! The Apple half: build the `UIActivityIndicatorView` and apply the
    //! same planned setters the Android half hands JNI. Unlike Android
    //! (module doc), `size_class`/`animating` need no construction-time
    //! reading of `props` — both are genuinely live properties on this arm,
    //! so `create` seeds the shared default and lets the ordinary diffed
    //! plan do the rest, exactly like every other control.

    use objc2::rc::Retained;
    use objc2::{MainThreadMarker, MainThreadOnly};
    use objc2_ui_kit::UIActivityIndicatorView;

    use super::{KIND, Spinner, SpinnerProps};
    use crate::NativeWidgetError;
    use crate::apple::{NativeCtx, NativeView};
    use crate::controls::platform;
    use crate::controls::{Plan, Setter};
    use crate::runtime::{NativeWidget, Params};

    /// A live spinner's retained state.
    pub(crate) struct SpinnerState {
        view: Retained<UIActivityIndicatorView>,
    }

    impl NativeWidget for Spinner {
        type Props = SpinnerProps;
        type State = SpinnerState;

        fn decode_props(params: &Params<'_>) -> Result<Self::Props, NativeWidgetError> {
            SpinnerProps::decode(params)
        }

        fn create(
            ctx: &mut NativeCtx<'_, '_>,
            props: &Self::Props,
        ) -> Result<(NativeView, Self::State), NativeWidgetError> {
            let mtm = ctx.mtm();
            let default = SpinnerProps::platform_default(props.slot);
            let view = UIActivityIndicatorView::initWithActivityIndicatorStyle(
                UIActivityIndicatorView::alloc(mtm),
                default.size_class.activity_indicator_style(),
            );
            // Module doc's *`animating`* section: a fresh view is already
            // `isAnimating == false`, matching `default.animating`, so
            // `hidesWhenStopped` is the only construction-time normalization
            // this arm needs before the diffed plan runs.
            view.setHidesWhenStopped(true);
            let state = SpinnerState { view };
            let plan = SpinnerProps::plan(&default, props);
            apply_all(mtm, &state.view, &plan);
            let handle = NativeView::new(Retained::clone(&state.view).into_super(), mtm);
            Ok((handle, state))
        }

        fn update(
            ctx: &mut NativeCtx<'_, '_>,
            state: &mut Self::State,
            old: &Self::Props,
            new: &Self::Props,
        ) -> Result<(), NativeWidgetError> {
            let plan = SpinnerProps::plan(old, new);
            apply_all(ctx.mtm(), &state.view, &plan);
            Ok(())
        }

        fn dispose(
            _ctx: &mut NativeCtx<'_, '_>,
            _state: Self::State,
        ) -> Result<(), NativeWidgetError> {
            // Display-only: nothing attached, and dropping the state
            // releases its retain.
            Ok(())
        }
    }

    /// Execute a whole [`Plan`], front to back.
    fn apply_all(mtm: MainThreadMarker, view: &UIActivityIndicatorView, plan: &Plan<'_>) {
        for setter in plan {
            apply(mtm, view, setter);
        }
    }

    /// Execute one planned property write against `view`.
    fn apply(mtm: MainThreadMarker, view: &UIActivityIndicatorView, setter: &Setter<'_>) {
        match *setter {
            Setter::SizeClass(size_class) => {
                view.setActivityIndicatorViewStyle(size_class.activity_indicator_style());
            }
            Setter::Animating(animating) => {
                if animating {
                    view.startAnimating();
                } else {
                    view.stopAnimating();
                }
            }
            Setter::SpinnerTint(argb) => {
                // SAFETY: objc2's stated concern is solely whether nil is
                // allowed, and it is — `UIActivityIndicatorView.color` is
                // documented to restore the system default colour when set
                // to nil, exactly the "restore the platform's own colour"
                // meaning `Setter::SpinnerTint(None)` carries (mirrors
                // `set_image_tint`'s reasoning, `crate::controls`'s Apple
                // `platform` module).
                unsafe { view.setColor(platform::optional_ui_color(argb).as_deref()) };
            }
            // `UIActivityIndicatorView` is a bare `UIView`, not a
            // `UIControl` (module doc's *`enabled`* section) — no `enabled`
            // property to set. This field exists in the shared `Props` for
            // Android's real `View.setEnabled` benefit; iOS silently ignores
            // it, the same treatment `switch.rs`'s iOS arm gives its own
            // unusable `Setter::Typeface`.
            Setter::Enabled(_) => {}
            Setter::ContentDescription(label) => {
                platform::set_accessibility_label(view, label, mtm);
            }
            ref other => platform::warn_unexpected_setter(KIND, other),
        }
    }
}

#[cfg(target_os = "macos")]
pub(crate) mod platform {
    //! The macOS half: build an `NSProgressIndicator` in its `Spinning`
    //! style and apply the same planned setters the other two halves do.
    //! Like iOS (module doc), `size_class` needs no construction-time
    //! reading of `props` — `NSProgressIndicator.controlSize` is a live
    //! property declared directly on the class, not inherited from
    //! `NSControl` (this control is an `NSView`, not an `NSControl` at all —
    //! module doc's *`enabled`* section).

    use objc2::rc::Retained;
    use objc2_app_kit::{NSProgressIndicator, NSProgressIndicatorStyle};

    use super::{KIND, Spinner, SpinnerProps};
    use crate::NativeWidgetError;
    use crate::appkit::{NativeCtx, NativeView};
    use crate::controls::platform;
    use crate::controls::{Plan, Setter};
    use crate::runtime::{NativeWidget, Params};

    /// A live spinner's retained state.
    pub(crate) struct SpinnerState {
        /// The indicator, kept typed for its own `controlSize`/animation
        /// setters.
        view: Retained<NSProgressIndicator>,
    }

    impl NativeWidget for Spinner {
        type Props = SpinnerProps;
        type State = SpinnerState;

        fn decode_props(params: &Params<'_>) -> Result<Self::Props, NativeWidgetError> {
            SpinnerProps::decode(params)
        }

        fn create(
            ctx: &mut NativeCtx<'_, '_>,
            props: &Self::Props,
        ) -> Result<(NativeView, Self::State), NativeWidgetError> {
            let mtm = ctx.mtm();
            let view = NSProgressIndicator::new(mtm);
            // Construction-time normalization (`progress.rs`'s own section
            // of the same name): pin the style, indeterminate mode, and the
            // "hide when stopped" behaviour before the diffed create plan
            // runs, so a plan that sets neither `Setter::SizeClass` nor
            // `Setter::Animating` still leaves the view exactly at
            // `platform_default`.
            view.setStyle(NSProgressIndicatorStyle::Spinning);
            view.setIndeterminate(true);
            view.setDisplayedWhenStopped(false);
            let default = SpinnerProps::platform_default(props.slot);
            view.setControlSize(default.size_class.control_size());
            let state = SpinnerState { view };
            let plan = SpinnerProps::plan(&default, props);
            apply_all(&state.view, &plan);
            let handle = NativeView::new(Retained::clone(&state.view).into_super(), mtm);
            Ok((handle, state))
        }

        fn update(
            _ctx: &mut NativeCtx<'_, '_>,
            state: &mut Self::State,
            old: &Self::Props,
            new: &Self::Props,
        ) -> Result<(), NativeWidgetError> {
            apply_all(&state.view, &SpinnerProps::plan(old, new));
            Ok(())
        }

        fn dispose(
            _ctx: &mut NativeCtx<'_, '_>,
            state: Self::State,
        ) -> Result<(), NativeWidgetError> {
            // `NSProgressIndicator` exposes no `isAnimating` readback (unlike
            // `UIActivityIndicatorView`), so this stops it unconditionally
            // rather than checking first — `stopAnimation:` on an
            // already-stopped indicator is a documented no-op, and dropping
            // `state` afterwards releases the view either way.
            //
            // SAFETY: `stopAnimation:` takes an unused `sender` (`id`), for
            // which nil is the documented argument.
            unsafe { state.view.stopAnimation(None) };
            Ok(())
        }
    }

    /// Execute a whole [`Plan`], front to back.
    fn apply_all(view: &NSProgressIndicator, plan: &Plan<'_>) {
        for setter in plan {
            apply(view, setter);
        }
    }

    /// Execute one planned property write against `view`.
    fn apply(view: &NSProgressIndicator, setter: &Setter<'_>) {
        match *setter {
            Setter::SizeClass(size_class) => view.setControlSize(size_class.control_size()),
            Setter::Animating(animating) => {
                // SAFETY: as `dispose` above — nil `sender`, unused.
                if animating {
                    unsafe { view.startAnimation(None) };
                } else {
                    unsafe { view.stopAnimation(None) };
                }
            }
            Setter::SpinnerTint(argb) => platform::set_tint(view, "SpinnerTint", argb),
            // `NSProgressIndicator` is an `NSView`, not an `NSControl`
            // (module doc's *`enabled`* section) — no `enabled` property to
            // set.
            Setter::Enabled(_) => {}
            Setter::ContentDescription(label) => platform::set_accessibility_label(view, label),
            ref other => platform::warn_unexpected_setter(KIND, other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::with_identity;

    fn decode(body: &str) -> SpinnerProps {
        let raw = with_identity(KIND, 3, body);
        SpinnerProps::decode(&Params::new(&raw)).expect("decodes")
    }

    #[test]
    fn absent_fields_decode_to_the_platform_defaults_and_plan_nothing() {
        let props = decode("");
        assert_eq!(props, SpinnerProps::platform_default(3));
        assert!(SpinnerProps::plan(&SpinnerProps::platform_default(3), &props).is_empty());
    }

    #[test]
    fn the_create_plan_sets_exactly_the_non_default_fields_in_order() {
        let props = decode(
            "\"animating\":true,\"sizeClass\":\"large\",\"tint\":9,\
             \"contentDescription\":\"loading\",\"enabled\":false",
        );
        assert_eq!(
            SpinnerProps::plan(&SpinnerProps::platform_default(3), &props),
            vec![
                Setter::SizeClass(SizeClass::Large),
                Setter::Animating(true),
                Setter::SpinnerTint(Some(9)),
                Setter::ContentDescription(Some("loading")),
                Setter::Enabled(false),
            ]
        );
    }

    #[test]
    fn asking_to_animate_alone_plans_exactly_one_setter() {
        let props = decode("\"animating\":true");
        assert_eq!(
            SpinnerProps::plan(&SpinnerProps::platform_default(3), &props),
            vec![Setter::Animating(true)]
        );
    }

    #[test]
    fn stopping_a_running_spinner_plans_exactly_one_setter() {
        let running = decode("\"animating\":true");
        let stopped = decode("\"animating\":false");
        assert_eq!(
            SpinnerProps::plan(&running, &stopped),
            vec![Setter::Animating(false)]
        );
    }

    #[test]
    fn a_size_class_change_alone_plans_exactly_one_setter() {
        let old = decode("\"sizeClass\":\"small\"");
        let new = decode("\"sizeClass\":\"large\"");
        assert_eq!(
            SpinnerProps::plan(&old, &new),
            vec![Setter::SizeClass(SizeClass::Large)]
        );
    }

    #[test]
    fn an_unknown_size_class_spelling_degrades_to_medium() {
        assert_eq!(
            decode("\"sizeClass\":\"huge\"").size_class,
            SizeClass::Medium
        );
        assert_eq!(
            decode("\"sizeClass\":\"small\"").size_class,
            SizeClass::Small
        );
        assert_eq!(
            decode("\"sizeClass\":\"large\"").size_class,
            SizeClass::Large
        );
    }

    #[test]
    fn a_tint_change_alone_plans_exactly_one_setter() {
        let old = decode("\"tint\":1");
        let new = decode("\"tint\":2");
        assert_eq!(
            SpinnerProps::plan(&old, &new),
            vec![Setter::SpinnerTint(Some(2))]
        );
    }

    #[test]
    fn clearing_a_tint_plans_the_nullable_setter() {
        let tinted = decode("\"tint\":7");
        let plain = decode("");
        assert_eq!(
            SpinnerProps::plan(&tinted, &plain),
            vec![Setter::SpinnerTint(None)],
            "unlike a text colour, a tint list is clearable (setter takes null)"
        );
    }

    #[test]
    fn an_enabled_change_alone_plans_exactly_one_setter() {
        let props = decode("\"enabled\":false");
        assert_eq!(
            SpinnerProps::plan(&SpinnerProps::platform_default(3), &props),
            vec![Setter::Enabled(false)]
        );
    }

    #[test]
    fn every_planned_setter_reports_its_documented_tier() {
        assert_eq!(Setter::Animating(true).tier(), super::super::Tier::Cheap);
        assert_eq!(Setter::SpinnerTint(None).tier(), super::super::Tier::Cheap);
        assert_eq!(
            Setter::SizeClass(SizeClass::Large).tier(),
            super::super::Tier::Relayout
        );
    }
}
