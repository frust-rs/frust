//! The Android [`Backend`] — `android.os.Vibrator`/`VibrationEffect` over
//! plain JNI, no Kotlin/Gradle module.
//!
//! # Why no Gradle module
//!
//! Like `frust-clipboard`'s `ClipboardManager` backend: `Vibrator` and
//! `VibrationEffect` are plain framework classes reachable through the
//! bootstrap class loader — no app-defined helper class, and so no
//! `plugins/haptics/platform/android` module and no R8 keep rule. The one
//! thing this plugin genuinely needs from the app side — the `VIBRATE`
//! manifest permission — arrives via the registry's own
//! `Contribution::ManifestPermission` entry (`crates/frust-drive/src/plugin/registry.rs`'s
//! `HAPTICS` spec), not from anything in this module.
//!
//! # `Vibrator`, not `VibratorManager`
//!
//! `Context.VIBRATOR_SERVICE` (`android.os.Vibrator`) is deprecated since API
//! 31 in favor of `Context.VIBRATOR_MANAGER_SERVICE` (`VibratorManager`,
//! which addresses a *specific* actuator on a multi-vibrator device). It is
//! not *removed* — the platform continues to service it — and this crate's
//! vocabulary ([`HapticEffect`]) has no per-actuator addressing to offer in
//! the first place, so there is nothing `VibratorManager` would let this
//! backend do that `Vibrator` cannot. Using the single, API-26-compatible
//! class keeps this backend's JNI surface to one service lookup rather than
//! two (`VibratorManager` requiring its own separate `getSystemService` +
//! `getDefaultVibrator()` indirection) for identical behavior.
//!
//! # Platform handles + threading
//!
//! Every operation routes through
//! [`frust_plugin::android::with_jni_env`], which scoped-attaches the
//! current thread and hands us a live `Env` + the application `Context` —
//! the same substrate `frust-clipboard`/`frust-shared-preferences`/
//! `frust-secure-storage` use. Before the host shell installs the
//! `(JavaVM, Context)` handles, that call reports
//! [`frust_plugin::PlatformHandleError::NotInitialized`], mapped to
//! [`HapticsError::PlatformNotInitialized`] — an old scaffold predating
//! `nativeInitPlatform` degrades to a typed error, never a panic.
//!
//! `Vibrator.vibrate(VibrationEffect)` carries **no documented main-thread
//! requirement** and returns immediately (the vibration itself plays
//! asynchronously on the vibrator HAL, not synchronously inside the call) —
//! so this backend calls it directly from the caller's thread, inside a
//! fresh scoped JNI attachment per operation, matching the crate-wide
//! fire-and-forget contract with no extra dispatch machinery needed (unlike
//! [`crate::apple`], which does need one — UIKit's feedback generators are
//! main-thread-only objects).
//!
//! # No vibrator hardware is a silent no-op, not an error
//!
//! `Vibrator.hasVibrator()` is checked before building a `VibrationEffect`;
//! a device with no vibration motor (a small class of tablets/TVs, plus every
//! emulator without a configured vibrator) reports `Ok(())` having done
//! nothing further — matching the crate doc's *silent no-op where
//! unsupported* contract, not a [`HapticsError`].
//!
//! # API-level branching
//!
//! `VibrationEffect.createPredefined(int)` (the `EFFECT_TICK`/`EFFECT_CLICK`/
//! `EFFECT_HEAVY_CLICK` constants) is API 29+; this workspace's Android floor
//! is API 26 (`frust-embedding`'s `minSdk`). Below 29, [`HapticEffect::SelectionClick`]/
//! [`HapticEffect::ImpactLight`]/[`HapticEffect::ImpactHeavy`] fall back to
//! `VibrationEffect.createOneShot(long, int)` (API 26) with a duration/
//! amplitude pair standing in for the missing predefined effect.
//! [`HapticEffect::ImpactMedium`] has no predefined-effect equivalent on any
//! API level (Android ships only a "click" and a "heavy click", no "medium
//! click") and a composed waveform ([`HapticEffect::Success`]/
//! [`HapticEffect::Warning`]/[`HapticEffect::Error`]) is `createWaveform`
//! (also API 26), so those four effects use the same JNI call on every
//! supported API level — no branch needed. `Build.VERSION.SDK_INT` is read
//! reflectively (matching `frust-secure-storage::android::device_api_level`'s
//! precedent) rather than hardcoded, so this backend needs no compile-time
//! SDK constant.
//!
//! [`HapticEffect::SelectionClick`] additionally distinguishes API 29 from
//! API 30+: `Vibrator.areEffectsSupported(int...)` (the per-effect hardware
//! support query) is itself API 30, so only at 30+ can this backend ask
//! whether `EFFECT_TICK` is actually supported before committing to it,
//! falling back to `EFFECT_CLICK` when the query reports anything other than
//! "definitely supported" (`_NO` or `_UNKNOWN` both fall back conservatively —
//! see [`effect_supported`]). On API 29 exactly, this backend has no way to
//! ask and always tries `EFFECT_TICK` directly, matching the platform's own
//! documented behavior for an effect a given vibrator HAL doesn't support
//! (either a built-in fallback pattern, per the Android CDD's vibrator HAL
//! requirements, or — worst case — a no-op single tick attempt, never a
//! crash).

use jni::objects::{JIntArray, JLongArray, JObject, JValue};
use jni::{Env, jni_sig, jni_str};

use crate::{Backend, HapticEffect, HapticsError};

/// `Context.VIBRATOR_SERVICE` — the string key `getSystemService` expects
/// (see the module doc's *`Vibrator`, not `VibratorManager`*).
const VIBRATOR_SERVICE: &str = "vibrator";

/// `android.os.VibrationEffect.EFFECT_TICK` (= 2) — a very short,
/// low-magnitude tick. API 29 (`createPredefined`).
const EFFECT_TICK: i32 = 2;
/// `android.os.VibrationEffect.EFFECT_CLICK` (= 0) — a short, sharp click.
/// API 29.
const EFFECT_CLICK: i32 = 0;
/// `android.os.VibrationEffect.EFFECT_HEAVY_CLICK` (= 5) — a stronger click.
/// API 29.
const EFFECT_HEAVY_CLICK: i32 = 5;

/// `VibrationEffect.DEFAULT_AMPLITUDE` (= -1) is deliberately **not** used
/// below — every one-shot fallback names an explicit amplitude so the three
/// impact intensities stay distinguishable from each other on hardware
/// without a native "medium" effect (see the module doc's *API-level
/// branching*).
const PREDEFINED_EFFECTS_API: i32 = 29;
/// The API level `Vibrator#areEffectsSupported(int...)` was added at.
const EFFECTS_SUPPORTED_QUERY_API: i32 = 30;
/// `android.os.Vibrator.VIBRATION_EFFECT_SUPPORT_YES` (= 1) — the only value
/// [`effect_supported`] treats as "definitely supported"; `_NO` (2) and
/// `_UNKNOWN` (0) both fall back to [`EFFECT_CLICK`] conservatively.
const VIBRATION_EFFECT_SUPPORT_YES: i32 = 1;

// One-shot fallback durations/amplitudes (API 26-28, and `ImpactMedium` on
// every API level — see the module doc). Amplitudes are `1..=255`; chosen so
// the three impact intensities are clearly distinguishable from each other
// on hardware with amplitude control, and degrade to a single fixed strength
// (ignoring the amplitude hint) on hardware without it — never an error
// either way.
const SELECTION_CLICK_ONE_SHOT_MS: i64 = 10;
const SELECTION_CLICK_ONE_SHOT_AMPLITUDE: i32 = 60;
const IMPACT_LIGHT_ONE_SHOT_MS: i64 = 15;
const IMPACT_LIGHT_ONE_SHOT_AMPLITUDE: i32 = 90;
const IMPACT_MEDIUM_ONE_SHOT_MS: i64 = 25;
const IMPACT_MEDIUM_ONE_SHOT_AMPLITUDE: i32 = 150;
const IMPACT_HEAVY_ONE_SHOT_MS: i64 = 35;
const IMPACT_HEAVY_ONE_SHOT_AMPLITUDE: i32 = 220;

// Composed waveform patterns for the three notification-style effects
// (`VibrationEffect.createWaveform(long[], int[], int)`, API 26 — see the
// module doc's *API-level branching*). Each `timings`/`amplitudes` pair is
// the same length; `repeat = -1` (no repeat) is passed at the call site.
// Kept short per this crate's fire-and-forget contract.
const SUCCESS_TIMINGS_MS: [i64; 3] = [40, 40, 60];
const SUCCESS_AMPLITUDES: [i32; 3] = [160, 0, 220];
const WARNING_TIMINGS_MS: [i64; 3] = [90, 50, 50];
const WARNING_AMPLITUDES: [i32; 3] = [200, 0, 140];
const ERROR_TIMINGS_MS: [i64; 5] = [35, 35, 35, 35, 35];
const ERROR_AMPLITUDES: [i32; 5] = [220, 0, 220, 0, 220];

/// The Android backend. Stateless — every operation re-fetches the
/// `Vibrator` inside a fresh scoped JNI attachment.
pub(crate) struct AndroidHaptics;

/// Run `f` with a live `Env` + application `Context`, flattening the two
/// error layers: a missing platform handle → [`HapticsError::PlatformNotInitialized`];
/// a JVM attach failure → [`HapticsError::Platform`]. Mirrors
/// `frust-clipboard::android::with_context`.
fn with_context<T>(
    f: impl FnOnce(&mut Env, &JObject) -> Result<T, HapticsError>,
) -> Result<T, HapticsError> {
    match frust_plugin::android::with_jni_env(f) {
        Ok(inner) => inner,
        Err(frust_plugin::PlatformHandleError::NotInitialized) => {
            Err(HapticsError::PlatformNotInitialized)
        }
        Err(other) => Err(HapticsError::Platform(format!(
            "Android platform handle error: {other}"
        ))),
    }
}

/// Run a sequence of JNI calls, converting any pending Java exception into a
/// [`HapticsError::Platform`]. Mirrors `frust-clipboard::android::run_jni`.
fn run_jni<'local, T>(
    env: &mut Env<'local>,
    f: impl FnOnce(&mut Env<'local>) -> Result<T, jni::errors::Error>,
) -> Result<T, HapticsError> {
    let result = f(env);
    if env.exception_check() {
        let message = match env.exception_occurred() {
            Some(throwable) => {
                let name = env
                    .get_object_class(&throwable)
                    .ok()
                    .and_then(|class| class.get_name(env).ok())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "<unknown exception class>".to_string());
                let msg = throwable
                    .get_message(env)
                    .ok()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "<no message>".to_string());
                format!("{name}: {msg}")
            }
            None => "JNI reported an exception with no throwable".to_string(),
        };
        env.exception_clear();
        // Defensive: don't leave a second exception pending.
        if env.exception_check() {
            env.exception_clear();
        }
        return Err(HapticsError::Platform(format!(
            "Android Vibrator JNI exception: {message}"
        )));
    }
    result.map_err(|e| HapticsError::Platform(format!("Android Vibrator JNI error: {e}")))
}

/// `context.getSystemService(Context.VIBRATOR_SERVICE)`, cast to `Vibrator`.
fn get_vibrator<'local>(
    env: &mut Env<'local>,
    context: &JObject,
) -> Result<JObject<'local>, jni::errors::Error> {
    let service_name = env.new_string(VIBRATOR_SERVICE)?;
    env.call_method(
        context,
        jni_str!("getSystemService"),
        jni_sig!("(Ljava/lang/String;)Ljava/lang/Object;"),
        &[JValue::Object(&service_name)],
    )?
    .l()
}

/// `Build.VERSION.SDK_INT` — the device's API level. Read reflectively via a
/// static field so this backend needs no compile-time SDK constant (mirrors
/// `frust-secure-storage::android::device_api_level`).
fn device_api_level(env: &mut Env) -> Result<i32, jni::errors::Error> {
    env.get_static_field(
        jni_str!("android/os/Build$VERSION"),
        jni_str!("SDK_INT"),
        jni_sig!("I"),
    )?
    .i()
}

/// `Vibrator.hasVibrator()`.
fn has_vibrator(env: &mut Env, vibrator: &JObject) -> Result<bool, jni::errors::Error> {
    env.call_method(vibrator, jni_str!("hasVibrator"), jni_sig!("()Z"), &[])?
        .z()
}

/// `Vibrator.areEffectsSupported(int...)`, queried for one effect id — `true`
/// only when the platform reports [`VIBRATION_EFFECT_SUPPORT_YES`]
/// definitively (API 30+ only; see the module doc's *API-level branching*).
/// Any failure along the way (an older platform this crate doesn't expect to
/// call this from, a null/malformed result) is treated as "not confirmed
/// supported" rather than propagated — this function only ever narrows a
/// choice between two predefined effects that both work, never surfaces to a
/// caller as a [`HapticsError`].
fn effect_supported(
    env: &mut Env,
    vibrator: &JObject,
    effect_id: i32,
) -> Result<bool, jni::errors::Error> {
    let ids = int_array(env, &[effect_id])?;
    let result = env
        .call_method(
            vibrator,
            jni_str!("areEffectsSupported"),
            jni_sig!("([I)[I"),
            &[JValue::Object(&ids)],
        )?
        .l()?;
    if result.is_null() {
        return Ok(false);
    }
    let result = env.cast_local::<JIntArray>(result)?;
    let mut buf = [0i32; 1];
    result.get_region(env, 0, &mut buf)?;
    Ok(buf[0] == VIBRATION_EFFECT_SUPPORT_YES)
}

/// Build a `long[]` from `values` (`VibrationEffect.createWaveform`'s
/// `timings` parameter).
fn long_array<'local>(
    env: &mut Env<'local>,
    values: &[i64],
) -> Result<JObject<'local>, jni::errors::Error> {
    let array = JLongArray::new(env, values.len())?;
    array.set_region(env, 0, values)?;
    Ok(array.into())
}

/// Build an `int[]` from `values` (`areEffectsSupported`'s `effectIds`
/// varargs / `createWaveform`'s `amplitudes` parameter).
fn int_array<'local>(
    env: &mut Env<'local>,
    values: &[i32],
) -> Result<JObject<'local>, jni::errors::Error> {
    let array = JIntArray::new(env, values.len())?;
    array.set_region(env, 0, values)?;
    Ok(array.into())
}

/// `VibrationEffect.createPredefined(int)` — API 29.
fn predefined<'local>(
    env: &mut Env<'local>,
    effect_id: i32,
) -> Result<JObject<'local>, jni::errors::Error> {
    env.call_static_method(
        jni_str!("android/os/VibrationEffect"),
        jni_str!("createPredefined"),
        jni_sig!("(I)Landroid/os/VibrationEffect;"),
        &[JValue::Int(effect_id)],
    )?
    .l()
}

/// `VibrationEffect.createOneShot(long, int)` — API 26, this backend's
/// fallback for a predefined effect unavailable below API 29.
fn one_shot<'local>(
    env: &mut Env<'local>,
    duration_ms: i64,
    amplitude: i32,
) -> Result<JObject<'local>, jni::errors::Error> {
    env.call_static_method(
        jni_str!("android/os/VibrationEffect"),
        jni_str!("createOneShot"),
        jni_sig!("(JI)Landroid/os/VibrationEffect;"),
        &[JValue::Long(duration_ms), JValue::Int(amplitude)],
    )?
    .l()
}

/// `VibrationEffect.createWaveform(long[], int[], int)` — API 26, `repeat =
/// -1` (no repeat) always.
fn waveform<'local>(
    env: &mut Env<'local>,
    timings_ms: &[i64],
    amplitudes: &[i32],
) -> Result<JObject<'local>, jni::errors::Error> {
    let timings = long_array(env, timings_ms)?;
    let amps = int_array(env, amplitudes)?;
    env.call_static_method(
        jni_str!("android/os/VibrationEffect"),
        jni_str!("createWaveform"),
        jni_sig!("([J[II)Landroid/os/VibrationEffect;"),
        &[
            JValue::Object(&timings),
            JValue::Object(&amps),
            JValue::Int(-1),
        ],
    )?
    .l()
}

/// [`HapticEffect::SelectionClick`]: `EFFECT_TICK` on API 29+ (falling back
/// to `EFFECT_CLICK` at API 30+ when the platform reports TICK unsupported —
/// see [`effect_supported`]), else a one-shot fallback.
fn selection_click<'local>(
    env: &mut Env<'local>,
    vibrator: &JObject,
    sdk_int: i32,
) -> Result<JObject<'local>, jni::errors::Error> {
    if sdk_int >= PREDEFINED_EFFECTS_API {
        let effect_id = if sdk_int >= EFFECTS_SUPPORTED_QUERY_API
            && !effect_supported(env, vibrator, EFFECT_TICK)?
        {
            EFFECT_CLICK
        } else {
            EFFECT_TICK
        };
        predefined(env, effect_id)
    } else {
        one_shot(
            env,
            SELECTION_CLICK_ONE_SHOT_MS,
            SELECTION_CLICK_ONE_SHOT_AMPLITUDE,
        )
    }
}

/// [`HapticEffect::ImpactLight`]: `EFFECT_CLICK` on API 29+, else a one-shot
/// fallback.
fn impact_light<'local>(
    env: &mut Env<'local>,
    sdk_int: i32,
) -> Result<JObject<'local>, jni::errors::Error> {
    if sdk_int >= PREDEFINED_EFFECTS_API {
        predefined(env, EFFECT_CLICK)
    } else {
        one_shot(
            env,
            IMPACT_LIGHT_ONE_SHOT_MS,
            IMPACT_LIGHT_ONE_SHOT_AMPLITUDE,
        )
    }
}

/// [`HapticEffect::ImpactMedium`]: no predefined effect exists at any API
/// level (the module doc's *API-level branching*), so this is always a
/// one-shot amplitude.
fn impact_medium<'local>(env: &mut Env<'local>) -> Result<JObject<'local>, jni::errors::Error> {
    one_shot(
        env,
        IMPACT_MEDIUM_ONE_SHOT_MS,
        IMPACT_MEDIUM_ONE_SHOT_AMPLITUDE,
    )
}

/// [`HapticEffect::ImpactHeavy`]: `EFFECT_HEAVY_CLICK` on API 29+, else a
/// one-shot fallback.
fn impact_heavy<'local>(
    env: &mut Env<'local>,
    sdk_int: i32,
) -> Result<JObject<'local>, jni::errors::Error> {
    if sdk_int >= PREDEFINED_EFFECTS_API {
        predefined(env, EFFECT_HEAVY_CLICK)
    } else {
        one_shot(
            env,
            IMPACT_HEAVY_ONE_SHOT_MS,
            IMPACT_HEAVY_ONE_SHOT_AMPLITUDE,
        )
    }
}

/// Build the `VibrationEffect` for `effect` — the one exhaustive match (no
/// wildcard arm) that is this backend's half of the crate-wide compile-level
/// routing guarantee (the crate doc's *Backends* section).
fn build_vibration_effect<'local>(
    env: &mut Env<'local>,
    vibrator: &JObject,
    sdk_int: i32,
    effect: HapticEffect,
) -> Result<JObject<'local>, jni::errors::Error> {
    match effect {
        HapticEffect::SelectionClick => selection_click(env, vibrator, sdk_int),
        HapticEffect::ImpactLight => impact_light(env, sdk_int),
        HapticEffect::ImpactMedium => impact_medium(env),
        HapticEffect::ImpactHeavy => impact_heavy(env, sdk_int),
        HapticEffect::Success => waveform(env, &SUCCESS_TIMINGS_MS, &SUCCESS_AMPLITUDES),
        HapticEffect::Warning => waveform(env, &WARNING_TIMINGS_MS, &WARNING_AMPLITUDES),
        HapticEffect::Error => waveform(env, &ERROR_TIMINGS_MS, &ERROR_AMPLITUDES),
    }
}

impl Backend for AndroidHaptics {
    fn perform(&self, effect: HapticEffect) -> Result<(), HapticsError> {
        with_context(|env, context| {
            run_jni(env, |env| {
                let vibrator = get_vibrator(env, context)?;
                if !has_vibrator(env, &vibrator)? {
                    // No vibration hardware — silent no-op (module doc).
                    return Ok(());
                }
                let sdk_int = device_api_level(env)?;
                let vibration_effect = build_vibration_effect(env, &vibrator, sdk_int, effect)?;
                env.call_method(
                    &vibrator,
                    jni_str!("vibrate"),
                    jni_sig!("(Landroid/os/VibrationEffect;)V"),
                    &[JValue::Object(&vibration_effect)],
                )?;
                Ok(())
            })
        })
    }
}
