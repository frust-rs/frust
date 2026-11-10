//! The Android backend: `frust::BackgroundTask` over `FrustBackgroundTaskHost`, which runs
//! one `shortService` foreground service for every live guard.
//!
//! `begin` is one static call into the host, on a class pinned at [`install`], so the UI
//! thread does no classloader lookup inside `onPause`. The only blocking on this path is
//! the host's one binder call. The registry lock is never held across a JNI call: expiry
//! arrives on the main thread and a guard can drop on any thread.
//!
//! A refusal is an ordinary outcome: `set_refused`, id 0, and the work proceeds. Anything
//! the host raises counts as a refusal, and a pending exception is always cleared.
//!
//! This module holds no `unsafe` block; the one export runs under
//! [`jni::EnvUnowned::with_env`], which catches an unwind.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Instant;

use frust_reactive::{BackgroundTaskBackend, TaskState, install_background_task_backend};
use jni::errors::LogErrorAndDefault;
use jni::objects::{JClass, JLongArray, JObject, JValue};
use jni::refs::Global;
use jni::sys::jlong;
use jni::{Env, EnvUnowned, jni_sig, jni_str};

use crate::registry::Registry;

const HOST_CLASS_BINARY: &str = "dev.frust.backgroundtask.FrustBackgroundTaskHost";

/// Pinned once by [`install`]; `begin` and `end` only read it.
static HOST_CLASS: OnceLock<Global<JClass<'static>>> = OnceLock::new();

/// The installed backend, reachable from the free-function JNI export.
static TASKS: OnceLock<Arc<AndroidBackgroundTasks>> = OnceLock::new();

/// Set by the first refusal, so a device that always refuses logs one warning.
static REFUSAL_LOGGED: AtomicBool = AtomicBool::new(false);

#[derive(Default)]
pub(crate) struct AndroidBackgroundTasks {
    registry: Mutex<Registry>,
}

impl AndroidBackgroundTasks {
    fn registry(&self) -> MutexGuard<'_, Registry> {
        self.registry.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl BackgroundTaskBackend for AndroidBackgroundTasks {
    fn begin(&self, name: &str, state: &Arc<TaskState>) -> u64 {
        let started = Instant::now();
        let id = self.registry().allot(state.clone());
        let granted = host_begin(id, name);
        log::debug!(
            "frust-background-task: begin id={id} granted={granted} in {} µs",
            started.elapsed().as_micros()
        );
        if granted {
            return id;
        }
        state.set_refused();
        self.registry().take(id);
        if !REFUSAL_LOGGED.swap(true, Ordering::Relaxed) {
            log::warn!(
                "frust-background-task: the foreground service was refused; this work runs \
                 unprotected (further refusals are not logged)"
            );
        }
        0
    }

    fn end(&self, id: u64, _state: &Arc<TaskState>) {
        // Bound first, so the registry guard is released before the JNI call.
        let taken = self.registry().take(id);
        if taken.is_some() {
            host_end(id);
        }
    }
}

/// Pin the host class, then install the backend (install-if-absent). Without the class
/// nothing could ever start the service, so the guard stays the core no-op.
pub(crate) fn install() -> bool {
    if let Err(reason) = pin_host_class() {
        log::warn!("frust-background-task: backend not installed: {reason}");
        return false;
    }
    let tasks = TASKS.get_or_init(Arc::default).clone();
    install_background_task_backend(tasks)
}

fn pin_host_class() -> Result<(), String> {
    if HOST_CLASS.get().is_some() {
        return Ok(());
    }
    let class = frust_plugin::android::with_jni_env(|env, context| {
        let loaded = load_host_class(env, context);
        if env.exception_check() {
            env.exception_clear();
            return Err(
                "the plugin's Gradle module is not in this app; add the plugin from the frust \
                 TUI (Add Plugin) or wire the module by hand (see the README)"
                    .to_string(),
            );
        }
        loaded.map_err(|e| format!("loading {HOST_CLASS_BINARY}: {e}"))
    })
    .map_err(|e| format!("Android platform handle error: {e}"))??;
    HOST_CLASS.get_or_init(|| class);
    Ok(())
}

/// `context.getClassLoader().loadClass(..)`, promoted to a global reference. Plain
/// `FindClass` would resolve against the system loader off the main thread.
fn load_host_class(
    env: &mut Env<'_>,
    context: &JObject,
) -> Result<Global<JClass<'static>>, jni::errors::Error> {
    let loader = env
        .call_method(
            context,
            jni_str!("getClassLoader"),
            jni_sig!("()Ljava/lang/ClassLoader;"),
            &[],
        )?
        .l()?;
    let name = env.new_string(HOST_CLASS_BINARY)?;
    let class = env
        .call_method(
            &loader,
            jni_str!("loadClass"),
            jni_sig!("(Ljava/lang/String;)Ljava/lang/Class;"),
            &[JValue::Object(&name)],
        )?
        .l()?;
    let class = env.cast_local::<JClass>(class)?;
    env.new_global_ref(class)
}

/// `FrustBackgroundTaskHost.begin`. Every failure is `false`: no class, no attach, a JNI
/// error, or an exception the host let through.
fn host_begin(id: u64, name: &str) -> bool {
    let Some(class) = HOST_CLASS.get() else {
        return false;
    };
    frust_plugin::android::with_jni_env(|env, context| {
        let granted = call_host_begin(env, context, class, id, name);
        if env.exception_check() {
            env.exception_clear();
            return false;
        }
        granted.unwrap_or(false)
    })
    .unwrap_or(false)
}

fn call_host_begin(
    env: &mut Env<'_>,
    context: &JObject,
    class: &Global<JClass<'static>>,
    id: u64,
    name: &str,
) -> Result<bool, jni::errors::Error> {
    let name = env.new_string(name)?;
    env.call_static_method(
        class,
        jni_str!("begin"),
        jni_sig!("(Landroid/content/Context;JLjava/lang/String;)Z"),
        &[
            JValue::Object(context),
            JValue::Long(id as jlong),
            JValue::Object(&name),
        ],
    )?
    .z()
}

/// `FrustBackgroundTaskHost.end`, from any thread. A failure leaves the host's watchdog as
/// the backstop that stops the service.
fn host_end(id: u64) {
    let Some(class) = HOST_CLASS.get() else {
        return;
    };
    let ended = frust_plugin::android::with_jni_env(|env, context| {
        let result = env.call_static_method(
            class,
            jni_str!("end"),
            jni_sig!("(Landroid/content/Context;J)V"),
            &[JValue::Object(context), JValue::Long(id as jlong)],
        );
        if env.exception_check() {
            env.exception_clear();
            return false;
        }
        result.is_ok()
    });
    if !matches!(ended, Ok(true)) {
        log::debug!("frust-background-task: end id={id} did not reach the host");
    }
}

/// The host's expiry callback, on the main thread: the watchdog, `Service.onTimeout`, or a
/// late `startForeground` refusal. The host has already forgotten `ids`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_frust_backgroundtask_FrustBackgroundTaskHost_nativeOnTimeout<
    'local,
>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    ids: JLongArray<'local>,
) {
    env.with_env(|env| {
        let mut raw: Vec<jlong> = vec![0; ids.len(env)?];
        ids.get_region(env, 0, &mut raw)?;
        let ids: Vec<u64> = raw.into_iter().map(|id| id as u64).collect();
        if let Some(tasks) = TASKS.get() {
            let expired = tasks.registry().expire_all(&ids);
            log::debug!(
                "frust-background-task: expired {expired} of {} ids",
                ids.len()
            );
        }
        Ok::<(), jni::errors::Error>(())
    })
    .resolve::<LogErrorAndDefault>();
}
