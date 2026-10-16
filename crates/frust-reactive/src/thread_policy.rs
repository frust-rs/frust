//! Scheduling classes for the framework's own threads.
//!
//! Two kinds of thread the framework spawns compete with the UI thread for the
//! same cores: the render thread, which owns each frame's encode → acquire →
//! submit tail, and this crate's background runtime (its async workers and the
//! [`spawn_blocking`](crate::spawn_blocking) pool that JSON parsing and image
//! decoding land on). Left alone, both run at whatever priority they inherit
//! from the thread that spawned them. On Android that is the UI thread's boosted
//! top-app nice value, so a background decode competes with the frame on equal
//! terms, while a fixed render-thread nice value written over the inherited one
//! can *lower* the frame's own submit path below its producers.
//!
//! [`set_current_thread_class`] states which side of the frame a thread is on
//! and lets the platform shim translate that:
//!
//! | Platform | [`ThreadClass::Frame`] | [`ThreadClass::Background`] |
//! |---|---|---|
//! | Android, Linux | nice `min(inherited, -4)` (`THREAD_PRIORITY_DISPLAY`); an inherited better value is kept | nice `reference + 10`, never below the inherited value, at most `19` |
//! | iOS, macOS | `QOS_CLASS_USER_INTERACTIVE` | `QOS_CLASS_UTILITY` |
//! | everything else | nothing ([`ThreadClassOutcome::Unsupported`]) | nothing |
//!
//! The `reference` for the background class is the nice value of the thread
//! that built the background runtime — the UI thread (see
//! [`ReactiveRuntime::init`](crate::ReactiveRuntime::init)) — recorded once, so
//! every pool thread lands ten steps below the UI thread whichever thread
//! happened to spawn it (a blocking thread spawned from an already-lowered
//! worker would otherwise drift a further ten steps down).
//!
//! The shim only ever asks for a priority the platform hands out without
//! privilege, and never takes one away from a thread that inherited better:
//! the frame class asks for `-4` only when the inherited value is worse, and
//! keeps the inherited value when the kernel refuses (`EACCES` outside an
//! Android app's rlimit); the background class only ever raises the nice value
//! (lower priority), which a thread may always do to itself.
//!
//! [`NO_THREAD_TUNING_VAR`] (`FRUST_NO_THREAD_TUNING`, compile-time define or
//! runtime env, any non-`"0"` value) turns the policy off: the background
//! runtime is built with tokio's defaults and nothing calls this module. The
//! shells consult the same variable through
//! `frust_shell_common::render_split::thread_tuning_enabled` for the render
//! thread and keep their previous fixed priority under it.
//!
//! The `unsafe` here is three plain libc calls on the calling thread
//! (`gettid`/`getpriority`/`setpriority` on Linux kernels,
//! `pthread_set_qos_class_self_np` on Apple), each in its own `SAFETY`-noted
//! block inside the private `sys` module.

#[cfg(any(target_os = "android", target_os = "linux"))]
use std::sync::atomic::{AtomicI32, Ordering};

/// Which side of the frame a thread works on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadClass {
    /// On the frame's critical path: the render thread's encode → acquire →
    /// submit tail.
    Frame,
    /// Off the frame's critical path: the background runtime's async workers
    /// and its blocking pool.
    Background,
}

/// What [`set_current_thread_class`] did to the calling thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadClassOutcome {
    /// Linux kernels: the nice value moved from `from` to `to`.
    Changed {
        /// The nice value the thread inherited.
        from: i32,
        /// The nice value it runs at now.
        to: i32,
    },
    /// Linux kernels: the inherited nice value already satisfies the class,
    /// so nothing was written.
    Kept {
        /// The inherited (and unchanged) nice value.
        nice: i32,
    },
    /// Apple: the QoS class was applied.
    QosApplied,
    /// The OS refused the change; the thread keeps what it inherited. `error`
    /// is the raw OS error code (`errno` on Linux kernels, the returned code
    /// on Apple).
    Refused {
        /// Raw OS error code.
        error: i32,
    },
    /// No policy on this platform; nothing was attempted.
    Unsupported,
}

/// The thread-tuning kill-switch environment/compile-time variable: when set
/// to any non-`"0"` value, [`thread_tuning_enabled`] is `false` and the
/// background runtime keeps tokio's default pool. Parsed exactly like the
/// shells' `FRUST_NO_RENDER_THREAD` (`frust_shell_common::render_split`), which
/// reads this same variable for the render thread.
pub const NO_THREAD_TUNING_VAR: &str = "FRUST_NO_THREAD_TUNING";

/// Whether the thread-scheduling policy is on: `true` unless the
/// [`NO_THREAD_TUNING_VAR`] kill switch is engaged by the compile-time define
/// or the runtime environment.
pub fn thread_tuning_enabled() -> bool {
    !kill_switch(
        option_env!("FRUST_NO_THREAD_TUNING"),
        std::env::var(NO_THREAD_TUNING_VAR).ok().as_deref(),
    )
}

/// The pure decision [`thread_tuning_enabled`] negates: a non-`"0"` value from
/// either source engages the kill switch.
fn kill_switch(compile_time: Option<&str>, runtime: Option<&str>) -> bool {
    fn is_set_non_zero(value: Option<&str>) -> bool {
        matches!(value, Some(v) if v != "0")
    }
    is_set_non_zero(compile_time) || is_set_non_zero(runtime)
}

/// `android.os.Process.THREAD_PRIORITY_DISPLAY`, the published nice value of
/// Android's own display-pipeline threads: the floor the frame class asks for
/// when the inherited value is worse.
pub const FRAME_NICE: i32 = -4;

/// How many nice steps the background class sits below the reference (UI)
/// thread. Android's own `THREAD_PRIORITY_BACKGROUND` is `10`; relative to a
/// top-app UI thread at `-10` this lands the pool at the default `0`.
pub const BACKGROUND_NICE_OFFSET: i32 = 10;

/// The weakest nice value Linux accepts.
#[cfg_attr(not(any(target_os = "android", target_os = "linux")), allow(dead_code))]
const MAX_NICE: i32 = 19;

/// The frame class's target nice for a thread that inherited `inherited`:
/// `None` when the inherited value is already at least as good as
/// [`FRAME_NICE`] (keep it), otherwise `Some(FRAME_NICE)`.
#[cfg_attr(not(any(target_os = "android", target_os = "linux")), allow(dead_code))]
pub(crate) fn frame_target(inherited: i32) -> Option<i32> {
    (inherited > FRAME_NICE).then_some(FRAME_NICE)
}

/// The background class's target nice for a thread that inherited `inherited`,
/// relative to the recorded `reference` (the inherited value itself when none
/// was recorded): `reference + BACKGROUND_NICE_OFFSET`, never below
/// `inherited` (the class only ever lowers priority) and never above
/// [`MAX_NICE`]. `None` when that is the inherited value already.
#[cfg_attr(not(any(target_os = "android", target_os = "linux")), allow(dead_code))]
pub(crate) fn background_target(inherited: i32, reference: Option<i32>) -> Option<i32> {
    let base = reference.unwrap_or(inherited);
    let target = base
        .saturating_add(BACKGROUND_NICE_OFFSET)
        .max(inherited)
        .min(MAX_NICE);
    (target != inherited).then_some(target)
}

/// The background runtime's blocking-pool cap on this machine: every core but
/// two, never fewer than two, over `std::thread::available_parallelism`
/// (one core assumed when that is unknown). Public so a shell can log the
/// value it runs with.
pub fn blocking_pool_cap() -> usize {
    blocking_pool_cap_for(std::thread::available_parallelism().map_or(1, |n| n.get()))
}

/// The blocking-pool size cap for a machine with `cores` CPUs: every core but
/// two (the UI and render threads keep theirs), never fewer than two.
pub(crate) fn blocking_pool_cap_for(cores: usize) -> usize {
    cores.saturating_sub(2).max(2)
}

/// Sentinel for "no reference recorded yet".
#[cfg(any(target_os = "android", target_os = "linux"))]
const NO_REFERENCE: i32 = i32::MIN;

/// The nice value of the thread that built the background runtime (the UI
/// thread), read by the background class. [`NO_REFERENCE`] until recorded.
#[cfg(any(target_os = "android", target_os = "linux"))]
static REFERENCE_NICE: AtomicI32 = AtomicI32::new(NO_REFERENCE);

/// Records the calling thread's priority as the reference the background class
/// is measured from. Called once on the UI thread while the background runtime
/// is built; a no-op where the platform has no nice values. The wasm runtime
/// starts no threads, so nothing calls it there.
#[cfg_attr(target_family = "wasm", allow(dead_code))]
pub(crate) fn record_reference_thread() {
    #[cfg(any(target_os = "android", target_os = "linux"))]
    if let Ok(nice) = sys::current_nice() {
        REFERENCE_NICE.store(nice, Ordering::Relaxed);
    }
}

#[cfg(any(target_os = "android", target_os = "linux"))]
fn recorded_reference() -> Option<i32> {
    let nice = REFERENCE_NICE.load(Ordering::Relaxed);
    (nice != NO_REFERENCE).then_some(nice)
}

/// Applies `class` to the calling thread (see the module docs for what each
/// platform does). Best-effort: a refusal leaves the thread at the priority it
/// inherited and is reported, never raised as a panic. Shells log the
/// outcome; this crate has no logger.
pub fn set_current_thread_class(class: ThreadClass) -> ThreadClassOutcome {
    #[cfg(any(target_os = "android", target_os = "linux"))]
    {
        let inherited = match sys::current_nice() {
            Ok(nice) => nice,
            Err(error) => return ThreadClassOutcome::Refused { error },
        };
        let target = match class {
            ThreadClass::Frame => frame_target(inherited),
            ThreadClass::Background => background_target(inherited, recorded_reference()),
        };
        match target {
            None => ThreadClassOutcome::Kept { nice: inherited },
            Some(to) => match sys::set_nice(to) {
                Ok(()) => ThreadClassOutcome::Changed {
                    from: inherited,
                    to,
                },
                Err(error) => ThreadClassOutcome::Refused { error },
            },
        }
    }

    #[cfg(target_vendor = "apple")]
    {
        let qos = match class {
            ThreadClass::Frame => libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE,
            ThreadClass::Background => libc::qos_class_t::QOS_CLASS_UTILITY,
        };
        match sys::set_qos(qos) {
            Ok(()) => ThreadClassOutcome::QosApplied,
            Err(error) => ThreadClassOutcome::Refused { error },
        }
    }

    #[cfg(not(any(target_os = "android", target_os = "linux", target_vendor = "apple")))]
    {
        let _ = class;
        ThreadClassOutcome::Unsupported
    }
}

/// The calling thread's nice value, where the platform has one. Exposed for
/// tests and diagnostics that compare a thread with its spawner.
pub fn current_thread_nice() -> Option<i32> {
    #[cfg(any(target_os = "android", target_os = "linux"))]
    {
        sys::current_nice().ok()
    }
    #[cfg(not(any(target_os = "android", target_os = "linux")))]
    {
        None
    }
}

/// The platform calls. Every function acts on the calling thread only.
#[cfg(any(target_os = "android", target_os = "linux"))]
mod sys {
    /// The calling thread's kernel id, the `who` that scopes `PRIO_PROCESS`
    /// to one thread (Linux nice values are per thread).
    fn tid() -> libc::id_t {
        // SAFETY: `gettid` takes no arguments, cannot fail and touches no
        // memory; it returns the calling thread's kernel id.
        let tid = unsafe { libc::gettid() };
        tid as libc::id_t
    }

    /// The calling thread's nice value, or the raw `errno`.
    ///
    /// Read through the raw syscall rather than libc's `getpriority`, whose
    /// `-1` return is ambiguous (a legitimate nice of `-1` versus an error):
    /// the kernel returns `20 - nice`, always in `1..=40`, and a negative
    /// value only on error.
    pub(super) fn current_nice() -> Result<i32, i32> {
        // SAFETY: `SYS_getpriority` with `PRIO_PROCESS` and this thread's own
        // id only reads the thread's scheduling attribute; no pointer crosses
        // the call.
        let raw = unsafe { libc::syscall(libc::SYS_getpriority, libc::PRIO_PROCESS, tid()) };
        if raw < 0 {
            Err(last_errno())
        } else {
            Ok(20 - raw as i32)
        }
    }

    /// Sets the calling thread's nice value to `nice`, or returns the raw
    /// `errno` (`EACCES` when lowering below what the rlimit allows).
    pub(super) fn set_nice(nice: i32) -> Result<(), i32> {
        // SAFETY: `setpriority` with `PRIO_PROCESS` and this thread's own id
        // changes only this thread's nice value; no pointer crosses the call.
        let rc = unsafe { libc::setpriority(libc::PRIO_PROCESS, tid(), nice) };
        if rc == 0 { Ok(()) } else { Err(last_errno()) }
    }

    fn last_errno() -> i32 {
        std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
    }
}

/// The platform calls. Every function acts on the calling thread only.
#[cfg(target_vendor = "apple")]
mod sys {
    /// Tags the calling thread with `qos`, or returns the call's error code.
    pub(super) fn set_qos(qos: libc::qos_class_t) -> Result<(), i32> {
        // SAFETY: `pthread_set_qos_class_self_np` operates on the calling
        // thread only and takes no pointer; a relative priority of `0` is the
        // documented default within the class.
        let rc = unsafe { libc::pthread_set_qos_class_self_np(qos, 0) };
        if rc == 0 { Ok(()) } else { Err(rc) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_target_keeps_an_inherited_value_at_or_above_display() {
        assert_eq!(frame_target(-10), None, "a top-app UI thread's -10 is kept");
        assert_eq!(frame_target(FRAME_NICE), None);
    }

    #[test]
    fn frame_target_asks_for_display_when_the_inherited_value_is_worse() {
        assert_eq!(frame_target(0), Some(FRAME_NICE));
        assert_eq!(frame_target(10), Some(FRAME_NICE));
    }

    #[test]
    fn background_target_sits_ten_steps_below_the_reference() {
        // A pool thread spawned from a -10 UI thread lands at 0.
        assert_eq!(background_target(-10, Some(-10)), Some(0));
        // One spawned from an already-lowered worker stays where it is
        // instead of drifting a further ten steps down.
        assert_eq!(background_target(0, Some(-10)), None);
    }

    #[test]
    fn background_target_never_raises_priority() {
        // Inherited weaker than reference + offset: kept, never lowered.
        assert_eq!(background_target(15, Some(-10)), None);
    }

    #[test]
    fn background_target_without_a_reference_is_relative_to_the_inherited_value() {
        assert_eq!(background_target(0, None), Some(10));
        assert_eq!(background_target(-4, None), Some(6));
    }

    #[test]
    fn background_target_clamps_at_the_weakest_nice() {
        assert_eq!(background_target(15, None), Some(MAX_NICE));
        assert_eq!(background_target(MAX_NICE, None), None);
    }

    #[test]
    fn blocking_pool_cap_leaves_two_cores_and_never_drops_below_two() {
        assert_eq!(blocking_pool_cap_for(0), 2);
        assert_eq!(blocking_pool_cap_for(1), 2);
        assert_eq!(blocking_pool_cap_for(4), 2);
        assert_eq!(blocking_pool_cap_for(6), 4);
        assert_eq!(blocking_pool_cap_for(8), 6);
        assert_eq!(blocking_pool_cap_for(12), 10);
    }

    #[test]
    fn kill_switch_parses_like_the_render_thread_switch() {
        assert!(!kill_switch(None, None));
        assert!(kill_switch(Some("1"), None));
        assert!(kill_switch(None, Some("1")));
        assert!(!kill_switch(Some("0"), None));
        assert!(!kill_switch(None, Some("0")));
        assert!(kill_switch(Some("0"), Some("1")));
    }

    /// Runs `f` on a fresh thread spawned from the calling one and returns its
    /// result, so a class change never leaks into the test-runner thread.
    #[cfg(any(target_os = "android", target_os = "linux"))]
    fn on_spawned_thread<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> R {
        std::thread::spawn(f).join().expect("probe thread panicked")
    }

    #[cfg(any(target_os = "android", target_os = "linux"))]
    #[test]
    fn a_background_thread_runs_at_a_higher_nice_than_its_spawner() {
        let spawner = current_thread_nice().expect("nice is readable on Linux");
        let (outcome, after) = on_spawned_thread(|| {
            let outcome = set_current_thread_class(ThreadClass::Background);
            (outcome, current_thread_nice().expect("nice is readable"))
        });
        if spawner >= MAX_NICE {
            assert_eq!(after, MAX_NICE, "nothing weaker than {MAX_NICE} exists");
            return;
        }
        assert!(
            after > spawner,
            "background thread nice {after} must be higher than its spawner's {spawner} \
             (outcome {outcome:?})"
        );
    }

    #[cfg(any(target_os = "android", target_os = "linux"))]
    #[test]
    fn a_frame_thread_never_runs_below_its_inherited_priority() {
        let spawner = current_thread_nice().expect("nice is readable on Linux");
        let (outcome, after) = on_spawned_thread(|| {
            let outcome = set_current_thread_class(ThreadClass::Frame);
            (outcome, current_thread_nice().expect("nice is readable"))
        });
        assert!(
            after <= spawner,
            "frame thread nice {after} must not be weaker than the inherited {spawner} \
             (outcome {outcome:?})"
        );
        match outcome {
            ThreadClassOutcome::Kept { nice } => assert_eq!(nice, spawner),
            ThreadClassOutcome::Changed { from, to } => {
                assert_eq!(from, spawner);
                assert_eq!(to, FRAME_NICE);
            }
            // An unprivileged host refuses the move to -4; the thread keeps
            // what it inherited, which the assertion above already pinned.
            ThreadClassOutcome::Refused { .. } => assert_eq!(after, spawner),
            other => panic!("unexpected outcome on Linux: {other:?}"),
        }
    }
}
