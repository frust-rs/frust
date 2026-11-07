//! Android backend: `ActivityManager.getMemoryInfo` and `isLowRamDevice` over plain JNI through
//! `frust-plugin`.
//!
//! # Why no Gradle module
//!
//! `ActivityManager`, `ActivityManager.MemoryInfo` and `Context.getSystemService` are framework
//! classes reachable through the bootstrap class loader, so there is no app-defined helper class,
//! no `platform/android` module and no R8 keep rule. Neither call needs a manifest permission.
//!
//! # Platform handles + threading
//!
//! The query runs inside [`frust_plugin::android::with_jni_env`], which attaches the current
//! thread for the duration of the call and hands over a live `Env` and the application `Context`.
//! The attachment is scoped per query, which is cheap enough for a per-query call, and the query
//! may be made from any thread: neither `getMemoryInfo` nor `isLowRamDevice` documents a
//! main-thread requirement. Before the shell installs the platform handles the call reports
//! `NotInitialized`, which surfaces as `None`.
//!
//! # Why this exists
//!
//! Since API 34 `ComponentCallbacks2` delivers only `TRIM_MEMORY_UI_HIDDEN` and
//! `TRIM_MEMORY_BACKGROUND`; the running-state levels are not sent to a foreground app. This
//! module answers that gap with a figure the app can poll.
//!
//! # What each figure means
//!
//! - `total_bytes` is `MemoryInfo.totalMem`, basically the RAM size of the device, not including
//!   below-kernel fixed allocations.
//! - `system_available_bytes` is `MemoryInfo.availMem`; a system-wide estimate that should not be
//!   considered absolute, not this process's budget.
//! - `system_low` is `MemoryInfo.lowMemory`, whether the system is below its low-memory
//!   threshold. `MemoryInfo.threshold` is logged at debug but not surfaced.
//! - `low_ram_device` is `ActivityManager.isLowRamDevice()`.
//!
//! Every JNI failure is logged at debug and reported as `None`; a pending Java exception is
//! cleared before returning. The Android floor is API 29, so no API-level branching is needed.

use jni::objects::{JObject, JValue};
use jni::{Env, jni_sig, jni_str};

use crate::DeviceMemory;

/// `Context.ACTIVITY_SERVICE`.
const ACTIVITY_SERVICE: &str = "activity";

/// Reads the device's memory now; `None` on any JNI failure.
pub(crate) fn query() -> Option<DeviceMemory> {
    match frust_plugin::android::with_jni_env(read) {
        Ok(memory) => memory,
        Err(err) => {
            log::debug!("frust-device-info android: platform handle unavailable: {err}");
            None
        }
    }
}

/// Runs the JNI sequence, clearing any pending Java exception afterwards.
fn read(env: &mut Env, context: &JObject) -> Option<DeviceMemory> {
    let result = read_inner(env, context);
    if env.exception_check() {
        log::debug!("frust-device-info android: Java exception during query, cleared");
        env.exception_clear();
        return None;
    }
    result
}

fn read_inner(env: &mut Env, context: &JObject) -> Option<DeviceMemory> {
    let service_name = step(env.new_string(ACTIVITY_SERVICE), "new_string")?;
    let am = step(
        env.call_method(
            context,
            jni_str!("getSystemService"),
            jni_sig!("(Ljava/lang/String;)Ljava/lang/Object;"),
            &[JValue::Object(&service_name)],
        )
        .and_then(|v| v.l()),
        "getSystemService",
    )?;
    if am.is_null() {
        log::debug!("frust-device-info android: getSystemService(\"activity\") returned null");
        return None;
    }
    let mi = step(
        env.new_object(
            jni_str!("android/app/ActivityManager$MemoryInfo"),
            jni_sig!("()V"),
            &[],
        ),
        "new MemoryInfo",
    )?;
    step(
        env.call_method(
            &am,
            jni_str!("getMemoryInfo"),
            jni_sig!("(Landroid/app/ActivityManager$MemoryInfo;)V"),
            &[JValue::Object(&mi)],
        ),
        "getMemoryInfo",
    )?;
    let total = step(
        env.get_field(&mi, jni_str!("totalMem"), jni_sig!("J"))
            .and_then(|v| v.j()),
        "totalMem",
    )?;
    let avail = step(
        env.get_field(&mi, jni_str!("availMem"), jni_sig!("J"))
            .and_then(|v| v.j()),
        "availMem",
    )?;
    let low = step(
        env.get_field(&mi, jni_str!("lowMemory"), jni_sig!("Z"))
            .and_then(|v| v.z()),
        "lowMemory",
    )?;
    let threshold = step(
        env.get_field(&mi, jni_str!("threshold"), jni_sig!("J"))
            .and_then(|v| v.j()),
        "threshold",
    )?;
    let low_ram = step(
        env.call_method(&am, jni_str!("isLowRamDevice"), jni_sig!("()Z"), &[])
            .and_then(|v| v.z()),
        "isLowRamDevice",
    )?;
    log::debug!(
        "frust-device-info android: total={total} avail={avail} low={low} threshold={threshold} low_ram={low_ram}"
    );
    Some(DeviceMemory {
        total_bytes: total.max(0) as u64,
        low_ram_device: low_ram,
        system_available_bytes: Some(avail.max(0) as u64),
        process_headroom_bytes: None,
        system_low: Some(low),
    })
}

/// Logs a failed JNI step at debug and converts the result to an `Option`.
fn step<T>(result: Result<T, jni::errors::Error>, what: &str) -> Option<T> {
    match result {
        Ok(v) => Some(v),
        Err(err) => {
            log::debug!("frust-device-info android: {what} failed: {err}");
            None
        }
    }
}
