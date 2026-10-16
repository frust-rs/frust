//! The Android performance-hint (ADPF) session: tells the platform how long
//! each frame's work took against the display period, so the CPU governor can
//! clock the UI and render threads for the frame loop instead of for their duty
//! cycle.
//!
//! A render thread that finishes each frame early and then blocks in the
//! swapchain acquire looks lightly loaded to a utilisation-driven governor,
//! which therefore never clocks it up. A hint session over the threads doing
//! the frame's work, fed one actual duration per frame against a target
//! duration, is the platform's own answer to that
//! (`APerformanceHint_*`, NDK `android/performance_hint.h`).
//!
//! # Shape
//!
//! - **One session per process**, over `{UI thread, frust-render thread}`,
//!   owned by the render thread's [`RenderHint`] and parked in a process-wide
//!   slot so the UI thread can close it on pause. The session API is not
//!   thread-safe; the slot's mutex serialises every call.
//! - **Opened lazily**, on the first rendered frame of a *continuous* loop —
//!   `frust_shell_common::FrameGate::is_continuous`, which the UI thread
//!   publishes every tick ([`publish_frame_loop`]) — while a surface is
//!   installed and the activity is resumed. **Closed** on pause
//!   ([`suspend`]), on surface destroy, and when the render thread exits. A
//!   loop that goes idle keeps the session but stops reporting; the platform
//!   lets an unreported session go stale on its own.
//! - **Target** = the display period, the smallest plausible delta between
//!   the Choreographer timestamps of recent continuous frames
//!   ([`TargetPeriod`]); it follows a refresh-rate switch.
//! - **Actual** = the frame's measured work: the UI thread's
//!   rebuild + layout + paint spans plus the render thread's encode span —
//!   the existing perf spans, measured while a session is wanted even with
//!   tracing off. One report per frame, from the render thread, which already
//!   receives the UI half with the scene. API 35+ reports through
//!   `AWorkDuration` (with the frame's Choreographer timestamp as the work
//!   period start); 33–34 through the basic call.
//! - **`FRUST_ADPF`** (compile-time `--define` or runtime env, any non-`"0"`
//!   value) opts in. Off by default: no session is created unless it is set.
//!
//! # Binding
//!
//! The entry points are resolved at runtime with `dlsym` from `libandroid.so`
//! and never linked: they do not exist below API 33, and frust's minSdk is 26.
//! Any missing symbol leaves the session off, never a failed load. The pure
//! session logic ([`HintSession`], [`TargetPeriod`], the kill switch) is
//! platform-free and host-tested through `tests/adpf.rs`; the `unsafe` FFI
//! lives in the Android-only `platform` submodule, each block `SAFETY`-noted.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// The ADPF opt-in environment/compile-time variable: the hint session runs
/// only when it is set to a non-`"0"` value.
pub(crate) const ADPF_VAR: &str = "FRUST_ADPF";

/// Whether the hint session may run: `true` only when [`ADPF_VAR`] is engaged
/// by the compile-time define or the runtime environment. Off by default.
/// Read once, when the render thread starts.
pub(crate) fn adpf_enabled() -> bool {
    adpf_opt_in(
        option_env!("FRUST_ADPF"),
        std::env::var(ADPF_VAR).ok().as_deref(),
    )
}

/// The pure decision [`adpf_enabled`] returns: a non-empty, non-`"0"` value
/// from either source opts in — the same parsing as
/// `frust_shell_common::render_split::thread_tuning_enabled`.
pub(crate) fn adpf_opt_in(compile_time: Option<&str>, runtime: Option<&str>) -> bool {
    fn is_set_non_zero(value: Option<&str>) -> bool {
        matches!(value, Some(v) if !v.is_empty() && v != "0")
    }
    is_set_non_zero(compile_time) || is_set_non_zero(runtime)
}

// ---------------------------------------------------------------------
// UI thread -> render thread: the frame-loop regime
// ---------------------------------------------------------------------

/// The UI thread's latest frame-gate regime (`FrameGate::is_continuous`).
static FRAME_LOOP_CONTINUOUS: AtomicBool = AtomicBool::new(false);

/// Whether a render thread holds a live hint (the knob is on and the
/// platform exports the API). Gates the UI thread's span clocks.
static HINT_ARMED: AtomicBool = AtomicBool::new(false);

/// Publish this tick's frame-gate regime. Called by the UI thread on every
/// gated tick, skipped ones included, so an idle loop reads `false`.
pub(crate) fn publish_frame_loop(continuous: bool) {
    FRAME_LOOP_CONTINUOUS.store(continuous, Ordering::Relaxed);
}

/// Whether this frame's work should be measured for the hint session: a hint
/// is armed and the loop is continuous. The UI thread reads it to time its
/// spans with tracing off; the render thread reads it once per frame.
pub(crate) fn measures_frame_work() -> bool {
    HINT_ARMED.load(Ordering::Relaxed) && FRAME_LOOP_CONTINUOUS.load(Ordering::Relaxed)
}

// ---------------------------------------------------------------------
// Target period
// ---------------------------------------------------------------------

/// Shortest frame-to-frame delta accepted as a display period (250Hz).
pub(crate) const MIN_PERIOD_NANOS: u64 = 4_000_000;

/// Longest frame-to-frame delta accepted as a display period (20Hz); a longer
/// gap is a stall or an idle stretch, not a refresh rate.
pub(crate) const MAX_PERIOD_NANOS: u64 = 50_000_000;

/// The target used before any period has been observed: 60Hz.
pub(crate) const DEFAULT_TARGET_NANOS: u64 = 16_666_667;

/// How many recent deltas the period is the minimum of. A frame the UI thread
/// produced but the render thread never saw (the hand-off keeps only the
/// newest) shows up as a two-period delta; the window minimum ignores it, and
/// a drop to a lower refresh rate is adopted once the window has turned over.
pub(crate) const PERIOD_WINDOW: usize = 16;

/// The display period estimated from the Choreographer timestamps of the
/// frames the render thread receives: the minimum plausible delta over the
/// last [`PERIOD_WINDOW`] frames.
#[derive(Debug, Clone)]
pub(crate) struct TargetPeriod {
    last_frame_time: Option<u64>,
    window: [u64; PERIOD_WINDOW],
    filled: usize,
    next: usize,
}

impl Default for TargetPeriod {
    fn default() -> Self {
        Self::new()
    }
}

impl TargetPeriod {
    /// An estimator with nothing observed yet.
    pub(crate) fn new() -> Self {
        Self {
            last_frame_time: None,
            window: [0; PERIOD_WINDOW],
            filled: 0,
            next: 0,
        }
    }

    /// Feed one received frame's timestamp (Choreographer `frameTimeNanos`).
    /// A delta outside [`MIN_PERIOD_NANOS`]`..=`[`MAX_PERIOD_NANOS`] is not a
    /// period and is dropped, as is a timestamp that does not advance.
    pub(crate) fn observe(&mut self, frame_time_nanos: u64) {
        let previous = self.last_frame_time.replace(frame_time_nanos);
        let Some(previous) = previous else {
            return;
        };
        let delta = frame_time_nanos.saturating_sub(previous);
        if !(MIN_PERIOD_NANOS..=MAX_PERIOD_NANOS).contains(&delta) {
            return;
        }
        self.window[self.next] = delta;
        self.next = (self.next + 1) % PERIOD_WINDOW;
        self.filled = (self.filled + 1).min(PERIOD_WINDOW);
    }

    /// The estimated period, or `None` before the first plausible delta.
    pub(crate) fn period_nanos(&self) -> Option<u64> {
        self.window[..self.filled].iter().copied().min()
    }

    /// The target duration to hand the session: the estimate, or
    /// [`DEFAULT_TARGET_NANOS`] before there is one.
    pub(crate) fn target_nanos(&self) -> u64 {
        self.period_nanos().unwrap_or(DEFAULT_TARGET_NANOS)
    }
}

/// Whether `current` is far enough from `sent` (more than a sixteenth) to be
/// worth an `updateTargetWorkDuration` call; Choreographer jitter is not.
pub(crate) fn target_moved(sent: u64, current: u64) -> bool {
    sent.abs_diff(current) > sent / 16
}

/// A duration in the session API's signed nanoseconds, saturating.
pub(crate) fn duration_nanos(duration: Duration) -> i64 {
    i64::try_from(duration.as_nanos()).unwrap_or(i64::MAX)
}

/// A count of nanoseconds in the session API's signed form, saturating.
pub(crate) fn nanos_i64(nanos: u64) -> i64 {
    i64::try_from(nanos).unwrap_or(i64::MAX)
}

// ---------------------------------------------------------------------
// The session state machine
// ---------------------------------------------------------------------

/// One frame's report: when its work period started and how long the work
/// took, both in `CLOCK_MONOTONIC` nanoseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WorkSample {
    /// The frame's Choreographer timestamp (`System.nanoTime` domain).
    pub(crate) start_nanos: i64,
    /// UI work plus render work; always positive.
    pub(crate) work_nanos: i64,
}

/// The platform calls a [`HintSession`] makes. Implemented over the NDK on
/// Android and by a recording fake in the host tests.
pub(crate) trait HintBackend {
    /// An open platform session.
    type Session;
    /// One-line description of the reporting path, for the open log line.
    fn describe(&self) -> &'static str;
    /// Create a session over `thread_ids` with an initial target, or `None`
    /// if the platform refuses (no hint support, a thread not in this
    /// process).
    fn create(&self, thread_ids: &[i32], target_nanos: i64) -> Option<Self::Session>;
    /// Move the target; `false` when the platform reports an error.
    fn update_target(&self, session: &mut Self::Session, target_nanos: i64) -> bool;
    /// Report one cycle's work; `false` when the platform reports an error.
    fn report(&self, session: &mut Self::Session, sample: WorkSample) -> bool;
    /// Close the session.
    fn close(&self, session: Self::Session);
}

/// What [`HintSession::frame`] did with one frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FrameHint {
    /// Nothing: the loop is not continuous, the session is suspended or
    /// refused, or the frame's work was not measured.
    Idle,
    /// The session was opened on this frame, and the frame reported.
    Opened,
    /// The frame reported into the open session.
    Reported,
    /// The platform refused to create a session; no retry until the next
    /// surface install.
    Refused,
    /// A target update or report failed; the session was closed and stays
    /// closed until the next surface install.
    Dropped,
}

/// The hint-session lifecycle over a [`HintBackend`]: when to open, report,
/// retarget and close. Platform-free; see the module docs for the policy.
pub(crate) struct HintSession<B: HintBackend> {
    backend: B,
    thread_ids: [i32; 2],
    session: Option<B::Session>,
    /// The target the open session was last given.
    sent_target: u64,
    period: TargetPeriod,
    /// Set between [`suspend`](Self::suspend) and [`resume`](Self::resume):
    /// the activity is paused, so no session may open.
    suspended: bool,
    /// Set when the platform refused a session or a call failed; cleared by
    /// the next surface install, so a refusing platform is asked once per
    /// surface, not once per frame.
    refused: bool,
}

impl<B: HintBackend> HintSession<B> {
    /// A closed session over the UI and render threads.
    pub(crate) fn new(backend: B, ui_tid: i32, render_tid: i32) -> Self {
        Self {
            backend,
            thread_ids: [ui_tid, render_tid],
            session: None,
            sent_target: 0,
            period: TargetPeriod::new(),
            suspended: false,
            refused: false,
        }
    }

    /// Whether a platform session is open.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn is_open(&self) -> bool {
        self.session.is_some()
    }

    /// One rendered frame. `continuous` is the frame-gate regime the frame was
    /// produced under; `frame_time_nanos` its Choreographer timestamp;
    /// `ui_work`/`render_work` its measured spans. A frame whose UI work reads
    /// zero was not measured (the UI thread's span clocks were off) and never
    /// reports.
    pub(crate) fn frame(
        &mut self,
        continuous: bool,
        frame_time_nanos: u64,
        ui_work: Duration,
        render_work: Duration,
    ) -> FrameHint {
        if !continuous {
            return FrameHint::Idle;
        }
        self.period.observe(frame_time_nanos);
        if self.suspended || self.refused || ui_work.is_zero() || frame_time_nanos == 0 {
            return FrameHint::Idle;
        }
        let sample = WorkSample {
            start_nanos: nanos_i64(frame_time_nanos),
            work_nanos: duration_nanos(ui_work.saturating_add(render_work)),
        };
        let target = self.period.target_nanos();

        let opened = if self.session.is_none() {
            match self.backend.create(&self.thread_ids, nanos_i64(target)) {
                Some(session) => {
                    self.session = Some(session);
                    self.sent_target = target;
                    log::info!(
                        "frust-shell-android: ADPF hint session opened (ui tid {}, render tid {}, \
                         target {} us, {})",
                        self.thread_ids[0],
                        self.thread_ids[1],
                        target / 1_000,
                        self.backend.describe()
                    );
                    true
                }
                None => {
                    self.refused = true;
                    log::warn!(
                        "frust-shell-android: ADPF hint session refused by the platform; \
                         continuing without it until the next surface"
                    );
                    return FrameHint::Refused;
                }
            }
        } else {
            false
        };

        let Some(session) = self.session.as_mut() else {
            return FrameHint::Idle;
        };
        if !opened && target_moved(self.sent_target, target) {
            if !self.backend.update_target(session, nanos_i64(target)) {
                self.drop_session("target update");
                return FrameHint::Dropped;
            }
            log::debug!(
                "frust-shell-android: ADPF target {} us -> {} us",
                self.sent_target / 1_000,
                target / 1_000
            );
            self.sent_target = target;
        }
        if !self.backend.report(session, sample) {
            self.drop_session("work report");
            return FrameHint::Dropped;
        }
        if opened {
            FrameHint::Opened
        } else {
            FrameHint::Reported
        }
    }

    /// A surface was (re)installed: a platform that refused before may be
    /// asked again.
    pub(crate) fn surface_installed(&mut self) {
        self.refused = false;
    }

    /// Close the session, if open (surface destroyed, render thread exiting).
    pub(crate) fn close(&mut self) {
        if let Some(session) = self.session.take() {
            self.backend.close(session);
            log::info!("frust-shell-android: ADPF hint session closed");
        }
    }

    /// The activity paused: close the session and open none until
    /// [`resume`](Self::resume).
    pub(crate) fn suspend(&mut self) {
        self.suspended = true;
        self.close();
    }

    /// The activity resumed: the next continuous frame may open a session.
    pub(crate) fn resume(&mut self) {
        self.suspended = false;
    }

    /// Close after a failed call and refuse until the next surface install.
    fn drop_session(&mut self, call: &str) {
        self.refused = true;
        if let Some(session) = self.session.take() {
            self.backend.close(session);
        }
        log::warn!(
            "frust-shell-android: ADPF {call} failed; hint session closed until the next surface"
        );
    }
}

impl<B: HintBackend> Drop for HintSession<B> {
    fn drop(&mut self) {
        self.close();
    }
}

// ---------------------------------------------------------------------
// Android: the NDK binding and the process-wide slot
// ---------------------------------------------------------------------

#[cfg(target_os = "android")]
pub(crate) use platform::{RenderHint, current_tid, resume, suspend};

#[cfg(target_os = "android")]
mod platform {
    use std::ffi::{CStr, c_int, c_void};
    use std::ptr::NonNull;
    use std::sync::atomic::Ordering;
    use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
    use std::time::Duration;

    use super::{HINT_ARMED, HintBackend, HintSession, WorkSample, adpf_enabled};

    // The `performance_hint.h` prototypes (API 33 unless noted), as function
    // pointer types: they are resolved with `dlsym`, never declared `extern`,
    // so nothing links against a symbol older releases lack.
    type GetManagerFn = unsafe extern "C" fn() -> *mut c_void;
    type CreateSessionFn = unsafe extern "C" fn(*mut c_void, *const i32, usize, i64) -> *mut c_void;
    type UpdateTargetFn = unsafe extern "C" fn(*mut c_void, i64) -> c_int;
    type ReportFn = unsafe extern "C" fn(*mut c_void, i64) -> c_int;
    type CloseSessionFn = unsafe extern "C" fn(*mut c_void);
    // API 35.
    type Report2Fn = unsafe extern "C" fn(*mut c_void, *mut c_void) -> c_int;
    type WorkDurationCreateFn = unsafe extern "C" fn() -> *mut c_void;
    type WorkDurationReleaseFn = unsafe extern "C" fn(*mut c_void);
    type WorkDurationSetFn = unsafe extern "C" fn(*mut c_void, i64);

    /// The API 33 session entry points.
    struct HintApi {
        get_manager: GetManagerFn,
        create_session: CreateSessionFn,
        update_target: UpdateTargetFn,
        report: ReportFn,
        close_session: CloseSessionFn,
        work_duration: Option<WorkDurationApi>,
    }

    /// The API 35 `AWorkDuration` reporting path.
    struct WorkDurationApi {
        report2: Report2Fn,
        create: WorkDurationCreateFn,
        release: WorkDurationReleaseFn,
        set_start: WorkDurationSetFn,
        set_total: WorkDurationSetFn,
        set_cpu: WorkDurationSetFn,
        set_gpu: WorkDurationSetFn,
    }

    /// The process's [`HintApi`], or `None` when `libandroid.so` lacks the
    /// API 33 entry points (an older release). Resolved once.
    fn hint_api() -> Option<&'static HintApi> {
        static API: OnceLock<Option<HintApi>> = OnceLock::new();
        API.get_or_init(|| {
            // SAFETY: `libandroid.so` is a public NDK library already loaded
            // in every app process; the handle is never closed, so every
            // resolved address stays valid for the process lifetime. `dlsym`
            // takes a NUL-terminated name (the `c"..."` literals). Each
            // non-null address is transmuted to the exact prototype
            // `android/performance_hint.h` declares for that symbol (NDK r28:
            // `int64_t` = `i64`, `size_t` = `usize`, `int32_t`/`pid_t` = `i32`,
            // `int` = `c_int`, the opaque struct pointers = `*mut c_void`).
            unsafe {
                let library = libc::dlopen(c"libandroid.so".as_ptr(), libc::RTLD_NOW);
                if library.is_null() {
                    return None;
                }
                let symbol = |name: &CStr| {
                    let address = libc::dlsym(library, name.as_ptr());
                    (!address.is_null()).then_some(address)
                };
                let work_duration = match (
                    symbol(c"APerformanceHint_reportActualWorkDuration2"),
                    symbol(c"AWorkDuration_create"),
                    symbol(c"AWorkDuration_release"),
                    symbol(c"AWorkDuration_setWorkPeriodStartTimestampNanos"),
                    symbol(c"AWorkDuration_setActualTotalDurationNanos"),
                    symbol(c"AWorkDuration_setActualCpuDurationNanos"),
                    symbol(c"AWorkDuration_setActualGpuDurationNanos"),
                ) {
                    (
                        Some(report2),
                        Some(create),
                        Some(release),
                        Some(set_start),
                        Some(set_total),
                        Some(set_cpu),
                        Some(set_gpu),
                    ) => Some(WorkDurationApi {
                        report2: std::mem::transmute::<*mut c_void, Report2Fn>(report2),
                        create: std::mem::transmute::<*mut c_void, WorkDurationCreateFn>(create),
                        release: std::mem::transmute::<*mut c_void, WorkDurationReleaseFn>(release),
                        set_start: std::mem::transmute::<*mut c_void, WorkDurationSetFn>(set_start),
                        set_total: std::mem::transmute::<*mut c_void, WorkDurationSetFn>(set_total),
                        set_cpu: std::mem::transmute::<*mut c_void, WorkDurationSetFn>(set_cpu),
                        set_gpu: std::mem::transmute::<*mut c_void, WorkDurationSetFn>(set_gpu),
                    }),
                    _ => None,
                };
                Some(HintApi {
                    get_manager: std::mem::transmute::<*mut c_void, GetManagerFn>(symbol(
                        c"APerformanceHint_getManager",
                    )?),
                    create_session: std::mem::transmute::<*mut c_void, CreateSessionFn>(symbol(
                        c"APerformanceHint_createSession",
                    )?),
                    update_target: std::mem::transmute::<*mut c_void, UpdateTargetFn>(symbol(
                        c"APerformanceHint_updateTargetWorkDuration",
                    )?),
                    report: std::mem::transmute::<*mut c_void, ReportFn>(symbol(
                        c"APerformanceHint_reportActualWorkDuration",
                    )?),
                    close_session: std::mem::transmute::<*mut c_void, CloseSessionFn>(symbol(
                        c"APerformanceHint_closeSession",
                    )?),
                    work_duration,
                })
            }
        })
        .as_ref()
    }

    /// An opaque NDK object pointer (the hint manager, a session, an
    /// `AWorkDuration`).
    struct NdkPtr(NonNull<c_void>);

    // SAFETY: the three objects behind an `NdkPtr` are plain heap objects the
    // NDK does not bind to the creating thread; the manager is a process
    // singleton, and a session or work duration is only ever touched through
    // the `HINT` mutex below, which is the external synchronisation
    // `performance_hint.h` asks callers to provide.
    unsafe impl Send for NdkPtr {}

    /// The NDK [`HintBackend`].
    pub(super) struct NdkBackend {
        api: &'static HintApi,
        manager: NdkPtr,
    }

    /// An open NDK session plus, on API 35+, the `AWorkDuration` it reports
    /// through (created with the session, released with it).
    pub(super) struct NdkSession {
        session: NdkPtr,
        work: Option<NdkPtr>,
    }

    impl HintBackend for NdkBackend {
        type Session = NdkSession;

        fn describe(&self) -> &'static str {
            if self.api.work_duration.is_some() {
                "AWorkDuration reports, API 35+"
            } else {
                "basic reports, API 33+"
            }
        }

        fn create(&self, thread_ids: &[i32], target_nanos: i64) -> Option<NdkSession> {
            // SAFETY: `manager` is the non-null pointer `getManager` returned;
            // `thread_ids` is a live slice whose pointer/length pair is read
            // only for the duration of the call; the target is positive (a
            // `TargetPeriod` value).
            let session = unsafe {
                (self.api.create_session)(
                    self.manager.0.as_ptr(),
                    thread_ids.as_ptr(),
                    thread_ids.len(),
                    target_nanos,
                )
            };
            let session = NdkPtr(NonNull::new(session)?);
            let work = self.api.work_duration.as_ref().and_then(|api| {
                // SAFETY: `AWorkDuration_create` takes no arguments; a null
                // result (documented as never) falls back to the basic path.
                NonNull::new(unsafe { (api.create)() }).map(NdkPtr)
            });
            Some(NdkSession { session, work })
        }

        fn update_target(&self, session: &mut NdkSession, target_nanos: i64) -> bool {
            // SAFETY: `session` is open (closed only by `close`, which consumes
            // it) and accessed under the `HINT` mutex; the target is positive.
            unsafe { (self.api.update_target)(session.session.0.as_ptr(), target_nanos) == 0 }
        }

        fn report(&self, session: &mut NdkSession, sample: WorkSample) -> bool {
            match (self.api.work_duration.as_ref(), session.work.as_ref()) {
                (Some(api), Some(work)) => {
                    let work = work.0.as_ptr();
                    // SAFETY: `work` is the live `AWorkDuration` created with
                    // this session and `session` is open, both accessed under
                    // the `HINT` mutex. The start timestamp and total are
                    // positive (`HintSession::frame` guarantees both), the CPU
                    // duration equals the total, and a zero GPU duration
                    // means "not measured", as the header allows.
                    unsafe {
                        (api.set_start)(work, sample.start_nanos);
                        (api.set_total)(work, sample.work_nanos);
                        (api.set_cpu)(work, sample.work_nanos);
                        (api.set_gpu)(work, 0);
                        (api.report2)(session.session.0.as_ptr(), work) == 0
                    }
                }
                // SAFETY: `session` is open and accessed under the `HINT`
                // mutex; the duration is positive.
                _ => unsafe {
                    (self.api.report)(session.session.0.as_ptr(), sample.work_nanos) == 0
                },
            }
        }

        fn close(&self, session: NdkSession) {
            if let (Some(api), Some(work)) = (self.api.work_duration.as_ref(), session.work) {
                // SAFETY: `work` was created by `AWorkDuration_create` with this
                // session and is released exactly once, here, as the session
                // is consumed.
                unsafe { (api.release)(work.0.as_ptr()) };
            }
            // SAFETY: the session pointer came from `createSession` and is
            // closed exactly once, here, as the session is consumed.
            unsafe { (self.api.close_session)(session.session.0.as_ptr()) };
        }
    }

    /// The process's hint session. One render thread owns it at a time (see
    /// [`RenderHint::open`]); the UI thread reaches it only to suspend and
    /// resume. Every session call runs under this lock.
    static HINT: Mutex<Option<HintSession<NdkBackend>>> = Mutex::new(None);

    fn hint_slot() -> MutexGuard<'static, Option<HintSession<NdkBackend>>> {
        HINT.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The calling thread's kernel thread id.
    pub(crate) fn current_tid() -> i32 {
        // SAFETY: `gettid` is a plain syscall wrapper with no preconditions;
        // it returns the calling thread's own id.
        unsafe { libc::gettid() }
    }

    /// The activity paused (UI thread): close the session; none opens until
    /// [`resume`].
    pub(crate) fn suspend() {
        if let Some(hint) = hint_slot().as_mut() {
            hint.suspend();
        }
    }

    /// The activity resumed (UI thread): a continuous frame may open a
    /// session again.
    pub(crate) fn resume() {
        if let Some(hint) = hint_slot().as_mut() {
            hint.resume();
        }
    }

    /// The render thread's handle on the process hint session. Dropping it
    /// closes the session and disarms the UI thread's span clocks.
    pub(crate) struct RenderHint {
        _owner: (),
    }

    impl RenderHint {
        /// Arm the hint for this (render) thread, with `ui_tid` as the other
        /// session thread. `None` — and a log line saying why — when
        /// `FRUST_ADPF` is not set, the platform lacks the API, or another
        /// render thread already holds the slot. Opens no session yet.
        pub(crate) fn open(ui_tid: i32) -> Option<Self> {
            if !adpf_enabled() {
                log::info!("frust-shell-android: ADPF off (default; set FRUST_ADPF=1 to opt in)");
                return None;
            }
            let Some(api) = hint_api() else {
                log::info!(
                    "frust-shell-android: ADPF unavailable (no APerformanceHint_* in \
                     libandroid.so, API < 33)"
                );
                return None;
            };
            // SAFETY: `getManager` takes no arguments and returns a process
            // singleton or null.
            let Some(manager) = NonNull::new(unsafe { (api.get_manager)() }) else {
                log::info!("frust-shell-android: ADPF unavailable (no hint manager)");
                return None;
            };
            let render_tid = current_tid();
            let mut slot = hint_slot();
            if slot.is_some() {
                log::warn!(
                    "frust-shell-android: ADPF hint already held by another render thread; \
                     this one runs without it"
                );
                return None;
            }
            let backend = NdkBackend {
                api,
                manager: NdkPtr(manager),
            };
            log::info!(
                "frust-shell-android: ADPF armed (ui tid {ui_tid}, render tid {render_tid}, {}); \
                 the session opens on a continuous frame loop",
                backend.describe()
            );
            *slot = Some(HintSession::new(backend, ui_tid, render_tid));
            HINT_ARMED.store(true, Ordering::Relaxed);
            Some(Self { _owner: () })
        }

        /// One rendered frame: report its work when the loop is continuous.
        /// `continuous` must be the [`super::measures_frame_work`] value the
        /// frame's render spans were measured under.
        pub(crate) fn frame(
            &mut self,
            continuous: bool,
            frame_time_nanos: u64,
            ui_work: Duration,
            render_work: Duration,
        ) {
            if let Some(hint) = hint_slot().as_mut() {
                hint.frame(continuous, frame_time_nanos, ui_work, render_work);
            }
        }

        /// A surface was installed on the render thread.
        pub(crate) fn surface_installed(&mut self) {
            if let Some(hint) = hint_slot().as_mut() {
                hint.surface_installed();
            }
        }

        /// The surface was destroyed on the render thread: close the session.
        pub(crate) fn surface_destroyed(&mut self) {
            if let Some(hint) = hint_slot().as_mut() {
                hint.close();
            }
        }
    }

    impl Drop for RenderHint {
        fn drop(&mut self) {
            HINT_ARMED.store(false, Ordering::Relaxed);
            // Dropping the session closes it.
            drop(hint_slot().take());
        }
    }
}
