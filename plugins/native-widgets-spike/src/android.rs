//! THROWAWAY — spike 1's Android arm: Rust-built native controls over JNI.
//!
//! Kotlin ↔ Rust contract (mirrors camera's plugin-private export pattern —
//! the exports live in THIS dependency crate, proven to survive into the app's
//! `.so` by the camera cycle):
//!
//! - `FrustNativeSpikeFactory.createView` → `nativeCreateControl(params,
//!   activity, context) -> View` — Rust builds the control (a real
//!   `android.widget.Button`, or the 50-child stress hierarchy) and hands the
//!   local ref back to the factory.
//! - `FrustNativeSpikeFactory.updateParams` → `nativeUpdateParams(view,
//!   params)` — the spike's `"click":N` bump triggers `performClick()` on the
//!   Rust-retained button (round-trip proof without spike 3's forwarding).
//! - `FrustNativeSpikeFactory.disposeView` → `nativeDisposeControl(view)` —
//!   paired-delete of every retained global ref (spike 1's leak bar).
//! - `FrustNativeListener.onClick` → `nativeOnEvent(controlId, kind)` — the
//!   generic listener glue: ONE class, one native method, dispatching by id.
//!
//! Global-ref discipline (KNOWN FACT 8: ART aborts at 51,200 refs): every
//! per-control ref lives in [`REGISTRY`] and is dropped on dispose;
//! [`live_ref_count`] must read 0 after a full dispose cycle. The method-ID /
//! class / interned-string caches are deliberate process-lifetime refs
//! (camera's `HOST_CLASS` shape), excluded from the per-control count.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, MutexGuard, OnceLock};
use std::time::Instant;

use jni::errors::LogErrorAndDefault;
use jni::objects::{JMethodID, JObject, JString, JValue};
use jni::refs::Global;
use jni::signature::{Primitive, ReturnType};
use jni::sys::{jint, jlong, jobject};
use jni::{Env, EnvUnowned, jni_sig, jni_str};

/// Everything the spike retains per live control, keyed off dispose-time
/// object identity ([`Env::is_same_object`]).
#[derive(Default)]
struct Registry {
    /// Retained single controls (the button), keyed by spike control id.
    controls: HashMap<i64, Global<JObject<'static>>>,
    /// The stress hierarchy's 50 child `TextView`s (property-set targets).
    stress: Vec<Global<JObject<'static>>>,
    /// The stress hierarchy's container (`LinearLayout`) + its control id.
    stress_container: Option<(i64, Global<JObject<'static>>)>,
    /// Last observed `"click":N` bump per control (performClick edge trigger).
    click_bumps: HashMap<i64, i64>,
}

static REGISTRY: LazyLock<Mutex<Registry>> = LazyLock::new(|| Mutex::new(Registry::default()));

/// Process-lifetime JNI caches (spike 1's "method-ID caching via `OnceLock`
/// from day 1" requirement): the `TextView` class, the two hot setter method
/// IDs, and two interned Java strings the stress loop alternates between so a
/// `setText` batch measures the FFI + TextView cost, not per-set
/// `NewStringUTF` allocation.
struct Cached {
    set_text: JMethodID,
    set_text_color: JMethodID,
    is_enabled: JMethodID,
    str_a: Global<JString<'static>>,
    str_b: Global<JString<'static>>,
}
// SAFETY-adjacent note: `JMethodID` is a process-wide-valid handle in JNI —
// jni 0.22 marks it Send+Sync for exactly this caching pattern.
static CACHED: OnceLock<Cached> = OnceLock::new();

/// Rolling stress stats, logged every [`STATS_EVERY`] ticks as a
/// `spike-perf stress ...` logcat line.
#[derive(Default)]
struct StressStats {
    ticks: u64,
    sum_us: u64,
    min_us: u64,
    max_us: u64,
}

static STATS: LazyLock<Mutex<StressStats>> = LazyLock::new(|| Mutex::new(StressStats::default()));

/// How many stress ticks between `spike-perf stress` log lines (~1/s at the
/// 30Hz cosmetic-loop cadence driving the ticks).
const STATS_EVERY: u64 = 30;

/// The two alternating text-color ARGB values (amber / cyan — visibly
/// changing, so a batch can't be silently elided anywhere down the stack).
const COLOR_A: i32 = 0xFFFF_B300_u32 as i32;
/// See [`COLOR_A`].
const COLOR_B: i32 = 0xFF00_E5FF_u32 as i32;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Per-control live global refs (cache refs excluded — see module doc).
pub(crate) fn live_ref_count() -> usize {
    let reg = lock(&REGISTRY);
    reg.controls.len() + reg.stress.len() + usize::from(reg.stress_container.is_some())
}

// --- tiny hand parsers (spike-grade; params are this crate's own strings) ---

/// Extract `"key":<integer>` from a flat JSON object string.
fn json_i64(s: &str, key: &str) -> Option<i64> {
    let pat = format!("\"{key}\":");
    let start = s.find(&pat)? + pat.len();
    let rest = s[start..].trim_start();
    let end = rest
        .char_indices()
        .find(|(_, c)| !(c.is_ascii_digit() || *c == '-'))
        .map_or(rest.len(), |(i, _)| i);
    rest[..end].parse().ok()
}

/// Extract `"key":"<value>"` from a flat JSON object string (no escapes —
/// spike-grade).
fn json_str<'a>(s: &'a str, key: &str) -> Option<&'a str> {
    let pat = format!("\"{key}\":\"");
    let start = s.find(&pat)? + pat.len();
    let rest = &s[start..];
    let end = rest.find('"')?;
    Some(&rest[..end])
}

// --- control construction ---------------------------------------------------

/// Build the spike's Button: `new Button(context)` + text + the Rust-attached
/// `FrustNativeListener` — every step a direct JNI call from the platform main
/// thread (the factory's `createView` contract).
fn build_button<'l>(
    env: &mut Env<'l>,
    context: &JObject<'l>,
    id: i64,
) -> Result<JObject<'l>, jni::errors::Error> {
    let btn = env.new_object(
        jni_str!("android/widget/Button"),
        jni_sig!("(Landroid/content/Context;)V"),
        &[JValue::Object(context)],
    )?;
    let label = env.new_string("Native (Rust-built)")?;
    env.call_method(
        &btn,
        jni_str!("setText"),
        jni_sig!("(Ljava/lang/CharSequence;)V"),
        &[JValue::Object(&label)],
    )?;
    env.call_method(
        &btn,
        jni_str!("setAllCaps"),
        jni_sig!("(Z)V"),
        &[JValue::Bool(false)],
    )?;
    // The generic listener: `new FrustNativeListener(id)` — resolvable via
    // FindClass because this native method's declaring class (the factory)
    // lives in the app classloader.
    let listener = env.new_object(
        jni_str!("dev/frust/FrustNativeListener"),
        jni_sig!("(J)V"),
        &[JValue::Long(id)],
    )?;
    env.call_method(
        &btn,
        jni_str!("setOnClickListener"),
        jni_sig!("(Landroid/view/View$OnClickListener;)V"),
        &[JValue::Object(&listener)],
    )?;
    lock(&REGISTRY)
        .controls
        .insert(id, env.new_global_ref(&btn)?);
    Ok(btn)
}

/// Seed [`CACHED`] on first stress build (needs a live `Env`).
fn ensure_cached(env: &mut Env<'_>) -> Result<(), jni::errors::Error> {
    if CACHED.get().is_some() {
        return Ok(());
    }
    let text_view = env.find_class(jni_str!("android/widget/TextView"))?;
    let set_text = env.get_method_id(
        &text_view,
        jni_str!("setText"),
        jni_sig!("(Ljava/lang/CharSequence;)V"),
    )?;
    let set_text_color = env.get_method_id(&text_view, jni_str!("setTextColor"), jni_sig!("(I)V"))?;
    let is_enabled = env.get_method_id(&text_view, jni_str!("isEnabled"), jni_sig!("()Z"))?;
    let a = env.new_string("frust native \u{25b2}")?;
    let b = env.new_string("frust native \u{25bc}")?;
    let cached = Cached {
        set_text,
        set_text_color,
        is_enabled,
        str_a: env.new_global_ref(&a)?,
        str_b: env.new_global_ref(&b)?,
    };
    let _ = CACHED.set(cached);
    Ok(())
}

/// Build the stress hierarchy: one `LinearLayout` containing
/// [`STRESS_CHILDREN`] `TextView`s, built inside a pushed local frame
/// (spike 1's `PushLocalFrame` requirement — the loop would otherwise pin 50+
/// local refs for the whole call).
const STRESS_CHILDREN: usize = 50;

fn build_stress<'l>(
    env: &mut Env<'l>,
    context: &JObject<'l>,
    id: i64,
) -> Result<JObject<'l>, jni::errors::Error> {
    ensure_cached(env)?;
    let container = env.new_object(
        jni_str!("android/widget/LinearLayout"),
        jni_sig!("(Landroid/content/Context;)V"),
        &[JValue::Object(context)],
    )?;
    // setOrientation(VERTICAL=1)
    env.call_method(
        &container,
        jni_str!("setOrientation"),
        jni_sig!("(I)V"),
        &[JValue::Int(1)],
    )?;

    let mut child_refs = Vec::with_capacity(STRESS_CHILDREN);
    for i in 0..STRESS_CHILDREN {
        // Each child in its own local frame so at most a handful of locals
        // are live at once; the global ref is what survives.
        let global = env.with_local_frame(8, |env| -> Result<_, jni::errors::Error> {
            let tv = env.new_object(
                jni_str!("android/widget/TextView"),
                jni_sig!("(Landroid/content/Context;)V"),
                &[JValue::Object(context)],
            )?;
            let text = env.new_string(format!("stress {i:02}"))?;
            env.call_method(
                &tv,
                jni_str!("setText"),
                jni_sig!("(Ljava/lang/CharSequence;)V"),
                &[JValue::Object(&text)],
            )?;
            env.call_method(
                &tv,
                jni_str!("setTextSize"),
                jni_sig!("(F)V"),
                &[JValue::Float(9.0)],
            )?;
            env.call_method(
                &container,
                jni_str!("addView"),
                jni_sig!("(Landroid/view/View;)V"),
                &[JValue::Object(&tv)],
            )?;
            env.new_global_ref(&tv)
        })?;
        child_refs.push(global);
    }

    let mut reg = lock(&REGISTRY);
    reg.stress = child_refs;
    reg.stress_container = Some((id, env.new_global_ref(&container)?));
    log::info!(
        "spike-refs after stress create: live={}",
        reg.controls.len() + reg.stress.len() + 1
    );
    Ok(container)
}

// --- the perf floor ---------------------------------------------------------

/// 500 (or `sets`) direct property sets against the retained stress views —
/// timed µs per batch, method-IDs cached, alternating values so nothing can
/// no-op. Called from the app's rebuild on the platform main thread (the
/// exact Phase 1 `api`-layer path).
pub(crate) fn stress_tick(sets: usize) -> Option<u64> {
    let reg = lock(&REGISTRY);
    if reg.stress.is_empty() {
        return None;
    }
    let cached = CACHED.get()?;
    let flip = lock(&STATS).ticks % 2 == 0;
    let start = Instant::now();
    // Timed in three segments so the FFI floor and the Android-side property
    // cost separate: (a) half the batch as `setTextColor` (invalidate-only —
    // the cheap-setter case), (b) half as `setText` (requestLayout per call —
    // the heavyweight case), (c) `isEnabled` (a no-side-effect getter — the
    // bare JNI crossing + dispatch floor).
    let mut color_us = 0u64;
    let mut text_us = 0u64;
    let mut bare_us = 0u64;
    let ran = frust_plugin::android::with_jni_env(|env, _context| {
        let n = reg.stress.len();
        let half = sets / 2;
        let t0 = Instant::now();
        for i in 0..half {
            let view = &reg.stress[i % n];
            let color = if flip { COLOR_A } else { COLOR_B };
            // SAFETY: cached method id belongs to android.widget.TextView,
            // the exact class every stress view was constructed from; the
            // signature is (I)V and we pass one jint.
            unsafe {
                env.call_method_unchecked(
                    view,
                    cached.set_text_color,
                    ReturnType::Primitive(Primitive::Void),
                    &[JValue::Int(color).as_jni()],
                )
            }?;
        }
        let t1 = Instant::now();
        for i in 0..half {
            let view = &reg.stress[i % n];
            let s = if flip { &cached.str_a } else { &cached.str_b };
            // SAFETY: as above; signature (Ljava/lang/CharSequence;)V, one
            // object arg.
            unsafe {
                env.call_method_unchecked(
                    view,
                    cached.set_text,
                    ReturnType::Primitive(Primitive::Void),
                    &[JValue::Object(s).as_jni()],
                )
            }?;
        }
        let t2 = Instant::now();
        for i in 0..half {
            let view = &reg.stress[i % n];
            // SAFETY: android.view.View.isEnabled()Z — no args, boolean
            // return, no side effects.
            unsafe {
                env.call_method_unchecked(
                    view,
                    cached.is_enabled,
                    ReturnType::Primitive(Primitive::Boolean),
                    &[],
                )
            }?;
        }
        color_us = t1.duration_since(t0).as_micros() as u64;
        text_us = t2.duration_since(t1).as_micros() as u64;
        bare_us = t2.elapsed().as_micros() as u64;
        Ok::<(), jni::errors::Error>(())
    });
    drop(reg);
    match ran {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            log::warn!("spike stress batch failed: {e}");
            return None;
        }
        Err(e) => {
            log::warn!("spike stress: platform handle: {e}");
            return None;
        }
    }
    let us = start.elapsed().as_micros() as u64;
    let mut stats = lock(&STATS);
    stats.ticks += 1;
    stats.sum_us += us;
    stats.min_us = if stats.min_us == 0 { us } else { stats.min_us.min(us) };
    stats.max_us = stats.max_us.max(us);
    if stats.ticks % STATS_EVERY == 0 {
        log::info!(
            "spike-perf stress sets={} last_us={} color_us={} text_us={} bare_us={} avg_us={} min_us={} max_us={} ticks={}",
            sets,
            us,
            color_us,
            text_us,
            bare_us,
            stats.sum_us / stats.ticks,
            stats.min_us,
            stats.max_us,
            stats.ticks,
        );
    }
    Some(us)
}

// --- Kotlin -> Rust exports -------------------------------------------------

/// `FrustNativeSpikeFactory.createView` → the Rust-built control, or null on
/// failure (the host catches a null/exception and marks the slot dead).
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_frust_FrustNativeSpikeFactory_nativeCreateControl<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    params: JString<'local>,
    _activity: JObject<'local>,
    context: JObject<'local>,
) -> jobject {
    let out: Option<jobject> = env
        .with_env(|env| -> Result<Option<jobject>, jni::errors::Error> {
            let params: String = params.mutf8_chars(env)?.to_str().into_owned();
            let id = json_i64(&params, "id").unwrap_or(0);
            let kind = json_str(&params, "control").unwrap_or("button").to_string();
            log::info!("spike-create control={kind} id={id} params={params}");
            let view = match kind.as_str() {
                "stress" => build_stress(env, &context, id)?,
                _ => build_button(env, &context, id)?,
            };
            Ok(Some(view.into_raw()))
        })
        .resolve::<LogErrorAndDefault>();
    out.unwrap_or(std::ptr::null_mut())
}

/// `FrustNativeSpikeFactory.updateParams` → performClick edge trigger
/// (`"click":N` bump) — the round-trip proof that needs no touch routing.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_frust_FrustNativeSpikeFactory_nativeUpdateParams<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    view: JObject<'local>,
    params: JString<'local>,
) {
    env.with_env(|env| -> Result<(), jni::errors::Error> {
        let params: String = params.mutf8_chars(env)?.to_str().into_owned();
        let id = json_i64(&params, "id").unwrap_or(0);
        let Some(bump) = json_i64(&params, "click") else {
            return Ok(());
        };
        let fire = {
            let mut reg = lock(&REGISTRY);
            let last = reg.click_bumps.insert(id, bump);
            last.is_some_and(|l| l != bump)
        };
        if fire {
            log::info!("spike-update performClick id={id} bump={bump}");
            env.call_method(&view, jni_str!("performClick"), jni_sig!("()Z"), &[])?;
        }
        Ok(())
    })
    .resolve::<LogErrorAndDefault>();
}

/// `FrustNativeSpikeFactory.disposeView` → paired-delete of every ref this
/// control retained; logs the remaining live count (the leak bar's receipt).
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_frust_FrustNativeSpikeFactory_nativeDisposeControl<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    view: JObject<'local>,
) {
    env.with_env(|env| -> Result<(), jni::errors::Error> {
        let mut reg = lock(&REGISTRY);
        let matched_control = {
            let mut found = None;
            for (id, global) in reg.controls.iter() {
                if env.is_same_object(&view, global)? {
                    found = Some(*id);
                    break;
                }
            }
            found
        };
        if let Some(id) = matched_control {
            reg.controls.remove(&id);
            reg.click_bumps.remove(&id);
        } else if let Some((id, container)) = reg.stress_container.take() {
            if env.is_same_object(&view, &container)? {
                reg.stress.clear();
                reg.click_bumps.remove(&id);
            } else {
                reg.stress_container = Some((id, container));
            }
        }
        log::info!(
            "spike-refs after dispose: live={}",
            reg.controls.len() + reg.stress.len() + usize::from(reg.stress_container.is_some())
        );
        Ok(())
    })
    .resolve::<LogErrorAndDefault>();
}

/// `FrustNativeListener.onClick` → the generic event dispatch into Rust: look
/// up the registered handler and invoke it on this (the platform main)
/// thread. The handler is app code — typically a signal write, which is what
/// wakes exactly one frust frame.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_frust_FrustNativeListener_nativeOnEvent<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    control_id: jlong,
    kind: jint,
) {
    env.with_env(|_env| -> Result<(), jni::errors::Error> {
        log::info!("spike-event control={control_id} kind={kind}");
        let handler = lock(crate::handler_slot()).clone();
        if let Some(h) = handler {
            h(control_id, kind);
        }
        Ok(())
    })
    .resolve::<LogErrorAndDefault>();
}
