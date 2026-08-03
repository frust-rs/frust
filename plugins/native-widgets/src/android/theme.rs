//! L1 of the theme ladder: the night-qualified `Context` control creation
//! builds every view against,
//! plus the one Android-only primitive L2's `Setter::ThemedBackground`
//! needs (`dp_to_px`). See [`crate::api::theme`] for L2's platform-neutral
//! token resolution — the module split is deliberate: that one is
//! host-testable (no `target_os` gate), this one is JNI-only.
//!
//! # L1: why a wrapped `Context`, not just tint lists
//!
//! An explicit tint list (L2) recolors what an app told a control to show,
//! but a framework `Button`/`Switch`/`SeekBar` also carries platform-owned
//! chrome an app never explicitly sets at all — the default ripple/state-layer
//! colour, and the `Switch`/`SeekBar` thumb-and-track drawables' own resting
//! colours before any tint is applied. Those resolve from `?android:attr/…`
//! theme attributes the *Context* a view was constructed with supplies,
//! which in turn come from that Context's resolved `Configuration` —
//! including its night-mode bit. A Glyph-dark app whose device is in light
//! mode (or vice versa) would otherwise show a light-resolved ripple/thumb
//! colour underneath an otherwise dark-themed control.
//!
//! [`night_qualified_context`] fixes exactly that bit before `create`
//! constructs anything: `Context.createConfigurationContext(Configuration)`
//! is **API 17+**
//! (<https://developer.android.com/reference/android/content/Context#createConfigurationContext(android.content.res.Configuration)>,
//! verified 2026-07-26) — this plugin's floor is `minSdk` 26, so it is
//! unconditionally available, no version gate needed.
//!
//! # Baked at construction, not live
//!
//! The wrapped Context only affects a view at the moment `new
//! Button(context)`/etc. runs. A live in-place brightness toggle (the
//! catalog's demo row) does NOT recreate the
//! view, so L1's platform-chrome effect stays fixed at whichever brightness
//! the control was first created under — only L2's own explicit setters
//! (tint lists, text/background colours, corner radius) actually re-paint
//! live on a later `update`, because those ride the ordinary
//! Props-diff-then-setter path every other property already uses. This is a
//! documented approximation, not a bug: a fully brightness-correct chrome
//! swap needs recreating the control, which "live re-theme without remount"
//! deliberately rules out for this module.

use std::sync::OnceLock;

use jni::objects::{JObject, JValue};
use jni::{Env, jni_sig, jni_str};

use crate::NativeWidgetError;
use crate::android::ctx::{NativeCtx, run_jni};
use crate::runtime::Params;

/// Whether a slot's create params requested the dark half of the active
/// theme — read straight off the raw wire ([`crate::controls::DARK`])
/// *before* any per-control typed `Props` decode runs: `create_control`
/// calls this ahead of `NativeRuntime::create`'s own kind dispatch, so there
/// is no registered kind to decode against yet.
///
/// Absent (an older api layer, or params from a control kind this task
/// didn't fold theme tokens into) degrades to `false` (light) rather than an
/// error — night-qualification is a cosmetic improvement, never a reason to
/// fail control creation.
pub(crate) fn brightness_is_dark(params_json: &str) -> bool {
    Params::new(params_json)
        .flag(crate::controls::DARK)
        .unwrap_or(false)
}

/// L1: wrap `base` in a `Configuration.uiMode`-forced `Context` — see the
/// module doc's *why a wrapped Context* section.
///
/// Copies `base`'s own live `Configuration` (locale, screen size, density —
/// every axis but the night bits) rather than building one from scratch, so
/// this never touches layout or text scale, only which day/night resource
/// set the platform's own chrome resolves against.
///
/// # Errors
/// [`NativeWidgetError::Platform`] on any JNI failure. The caller
/// (`android::create_control`) falls back to the unqualified `base` Context
/// on `Err` rather than failing the whole control creation over a cosmetic
/// miss.
pub(crate) fn night_qualified_context<'local>(
    env: &mut Env<'local>,
    base: &JObject<'local>,
    dark: bool,
) -> Result<JObject<'local>, NativeWidgetError> {
    run_jni(env, "createConfigurationContext", |env| {
        let resources = env
            .call_method(
                base,
                jni_str!("getResources"),
                jni_sig!("()Landroid/content/res/Resources;"),
                &[],
            )?
            .l()?;
        let live_config = env
            .call_method(
                &resources,
                jni_str!("getConfiguration"),
                jni_sig!("()Landroid/content/res/Configuration;"),
                &[],
            )?
            .l()?;
        let config_class = env.get_object_class(&live_config)?;
        // A COPY: `createConfigurationContext` must not receive (and must
        // not mutate) the live Resources' own Configuration object.
        let config = env.new_object(
            &config_class,
            jni_sig!("(Landroid/content/res/Configuration;)V"),
            &[JValue::Object(&live_config)],
        )?;

        let current_ui_mode = env
            .get_field(&config, jni_str!("uiMode"), jni_sig!("I"))?
            .i()?;
        let mask = env
            .get_static_field(&config_class, jni_str!("UI_MODE_NIGHT_MASK"), jni_sig!("I"))?
            .i()?;
        let night_yes = env
            .get_static_field(&config_class, jni_str!("UI_MODE_NIGHT_YES"), jni_sig!("I"))?
            .i()?;
        let night_no = env
            .get_static_field(&config_class, jni_str!("UI_MODE_NIGHT_NO"), jni_sig!("I"))?
            .i()?;
        let desired = if dark { night_yes } else { night_no };
        env.set_field(
            &config,
            jni_str!("uiMode"),
            jni_sig!("I"),
            JValue::Int((current_ui_mode & !mask) | desired),
        )?;

        env.call_method(
            base,
            jni_str!("createConfigurationContext"),
            jni_sig!("(Landroid/content/res/Configuration;)Landroid/content/Context;"),
            &[JValue::Object(&config)],
        )?
        .l()
    })
}

// ---------------------------------------------------------------------------
// L2's one Android-only helper: dp -> device px for a raw-pixel drawable API
// ---------------------------------------------------------------------------

/// The cached display density (module doc's *baked at construction* note
/// applies here too, in miniature: a mid-session display move is unhandled,
/// matching this plugin's static-first placement doctrine).
static DENSITY: OnceLock<f32> = OnceLock::new();

/// Convert a dp value into device pixels — `GradientDrawable.setCornerRadius`
/// (backing [`crate::controls::Setter::ThemedBackground`]) takes raw device
/// pixels, unlike `TextView.setTextSize(float)` (already SP, self-scaling),
/// so this is the one place in the plugin that needs the conversion.
///
/// Falls back to a 1:1 (density 1.0) conversion — logged once — only if the
/// density has never been read AND this call has no `Context` to read it
/// from (an `update`/`dispose` path with nothing cached yet). Every
/// control's own `create` always has a `Context`, so a themed control's
/// FIRST corner-radius application seeds the real density before any later
/// `update` for that same slot ever needs the fallback.
pub(crate) fn dp_to_px(ctx: &mut NativeCtx<'_, '_>, dp: f32) -> f32 {
    dp * density(ctx)
}

/// See [`dp_to_px`].
fn density(ctx: &mut NativeCtx<'_, '_>) -> f32 {
    if let Some(density) = DENSITY.get() {
        return *density;
    }
    match read_density(ctx) {
        Ok(density) => *DENSITY.get_or_init(|| density),
        Err(e) => {
            log::warn!(
                "frust-native-widgets: could not read display density ({e}) — corner radii apply \
                 at 1 device px per dp until a themed control is created with a live Context"
            );
            1.0
        }
    }
}

/// `context.getResources().getDisplayMetrics().density`.
fn read_density(ctx: &mut NativeCtx<'_, '_>) -> Result<f32, NativeWidgetError> {
    let context = ctx.context()?;
    ctx.run_jni("Resources.getDisplayMetrics().density", |env| {
        let resources = env
            .call_method(
                context,
                jni_str!("getResources"),
                jni_sig!("()Landroid/content/res/Resources;"),
                &[],
            )?
            .l()?;
        let metrics = env
            .call_method(
                &resources,
                jni_str!("getDisplayMetrics"),
                jni_sig!("()Landroid/util/DisplayMetrics;"),
                &[],
            )?
            .l()?;
        env.get_field(&metrics, jni_str!("density"), jni_sig!("F"))?
            .f()
    })
}
