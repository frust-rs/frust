//! The performance-hint (ADPF) session, checked on the host.
//!
//! `src/adpf.rs` keeps its policy platform-free: the session lifecycle
//! (`HintSession`), the target estimate (`TargetPeriod`) and the
//! `FRUST_ADPF` opt-in parsing compile on every target, and only the NDK binding
//! is Android-only. The file is included here as a module (precedent:
//! `crates/frust-engine/tests/atlas_churn.rs`), so the lifecycle runs against
//! a recording fake backend instead of the platform.
//!
//! The wiring lives in Android-only files no host build compiles, so the rest
//! is pinned on the source (precedent: `tests/thread_policy.rs`): the hint is
//! armed after the render thread's scheduling call, the session follows the
//! surface and the activity lifecycle, the UI thread publishes the gate's
//! regime before its skip return, and the NDK symbols are only ever resolved
//! with `dlsym`. Comment lines are dropped before every scan.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

#[allow(dead_code)]
#[path = "../src/adpf.rs"]
mod adpf;

use adpf::{
    DEFAULT_TARGET_NANOS, FrameHint, HintBackend, HintSession, PERIOD_WINDOW, TargetPeriod,
    WorkSample, adpf_opt_in, target_moved,
};

const UI_TID: i32 = 4100;
const RENDER_TID: i32 = 4200;
/// 120Hz and 60Hz display periods.
const P120: u64 = 8_333_333;
const P60: u64 = 16_666_667;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Call {
    Create { thread_ids: Vec<i32>, target: i64 },
    UpdateTarget(i64),
    Report(WorkSample),
    Close,
}

/// A backend that records every call and can be told to fail.
#[derive(Default, Clone)]
struct Fake {
    calls: Rc<RefCell<Vec<Call>>>,
    refuse_create: Rc<RefCell<bool>>,
    fail_report: Rc<RefCell<bool>>,
}

impl Fake {
    fn calls(&self) -> Vec<Call> {
        self.calls.borrow().clone()
    }

    fn reports(&self) -> Vec<WorkSample> {
        self.calls()
            .into_iter()
            .filter_map(|call| match call {
                Call::Report(sample) => Some(sample),
                _ => None,
            })
            .collect()
    }

    fn count(&self, wanted: fn(&Call) -> bool) -> usize {
        self.calls
            .borrow()
            .iter()
            .filter(|call| wanted(call))
            .count()
    }
}

impl HintBackend for Fake {
    type Session = ();

    fn describe(&self) -> &'static str {
        "fake"
    }

    fn create(&self, thread_ids: &[i32], target_nanos: i64) -> Option<()> {
        self.calls.borrow_mut().push(Call::Create {
            thread_ids: thread_ids.to_vec(),
            target: target_nanos,
        });
        (!*self.refuse_create.borrow()).then_some(())
    }

    fn update_target(&self, _session: &mut (), target_nanos: i64) -> bool {
        self.calls
            .borrow_mut()
            .push(Call::UpdateTarget(target_nanos));
        true
    }

    fn report(&self, _session: &mut (), sample: WorkSample) -> bool {
        self.calls.borrow_mut().push(Call::Report(sample));
        !*self.fail_report.borrow()
    }

    fn close(&self, _session: ()) {
        self.calls.borrow_mut().push(Call::Close);
    }
}

fn session() -> (HintSession<Fake>, Fake) {
    let fake = Fake::default();
    (HintSession::new(fake.clone(), UI_TID, RENDER_TID), fake)
}

fn ms(value: u64) -> Duration {
    Duration::from_millis(value)
}

/// Drive `frames` continuous frames one `period` apart from `start`, each
/// with 3 ms of UI work and 2 ms of render work; returns the next timestamp.
fn run(hint: &mut HintSession<Fake>, start: u64, period: u64, frames: usize) -> u64 {
    let mut now = start;
    for _ in 0..frames {
        hint.frame(true, now, ms(3), ms(2));
        now += period;
    }
    now
}

// ---------------------------------------------------------------------
// Session lifecycle
// ---------------------------------------------------------------------

#[test]
fn no_session_opens_while_the_loop_is_not_continuous() {
    let (mut hint, fake) = session();
    for frame in 0..10u64 {
        assert_eq!(
            hint.frame(false, 1_000_000_000 + frame * P60, ms(3), ms(2)),
            FrameHint::Idle
        );
    }
    assert!(!hint.is_open());
    assert!(fake.calls().is_empty(), "no platform call off the regime");
}

#[test]
fn the_first_continuous_frame_opens_a_session_over_both_threads_and_reports() {
    let (mut hint, fake) = session();
    assert_eq!(
        hint.frame(true, 1_000_000_000, ms(3), ms(2)),
        FrameHint::Opened
    );
    assert!(hint.is_open());
    assert_eq!(
        fake.calls(),
        vec![
            Call::Create {
                thread_ids: vec![UI_TID, RENDER_TID],
                target: DEFAULT_TARGET_NANOS as i64,
            },
            Call::Report(WorkSample {
                start_nanos: 1_000_000_000,
                work_nanos: 5_000_000,
            }),
        ],
        "opened at the default target with no period observed yet; the work is \
         UI + render and the work period starts at the frame's own timestamp"
    );
    assert_eq!(
        hint.frame(true, 1_000_000_000 + P60, ms(4), ms(1)),
        FrameHint::Reported
    );
    assert_eq!(fake.count(|c| matches!(c, Call::Create { .. })), 1);
}

#[test]
fn an_unmeasured_frame_never_reports() {
    // Zero UI work means the UI thread's span clocks were off for that frame.
    let (mut hint, fake) = session();
    assert_eq!(
        hint.frame(true, 1_000_000_000, Duration::ZERO, ms(2)),
        FrameHint::Idle
    );
    assert_eq!(hint.frame(true, 0, ms(3), ms(2)), FrameHint::Idle);
    assert!(fake.calls().is_empty());
}

#[test]
fn an_idle_loop_keeps_the_session_but_stops_reporting() {
    let (mut hint, fake) = session();
    let now = run(&mut hint, 1_000_000_000, P60, 4);
    let reports = fake.reports().len();
    for frame in 0..5 {
        assert_eq!(
            hint.frame(false, now + frame * P60, ms(3), ms(2)),
            FrameHint::Idle
        );
    }
    assert!(
        hint.is_open(),
        "the session stays open across an idle stretch"
    );
    assert_eq!(fake.reports().len(), reports, "no report while idle");
    assert_eq!(fake.count(|c| *c == Call::Close), 0);
}

#[test]
fn pause_closes_the_session_and_resume_lets_it_reopen() {
    let (mut hint, fake) = session();
    let now = run(&mut hint, 1_000_000_000, P60, 3);
    hint.suspend();
    assert!(!hint.is_open());
    assert_eq!(fake.count(|c| *c == Call::Close), 1);
    // A frame already in flight when the activity paused must not reopen it.
    assert_eq!(hint.frame(true, now, ms(3), ms(2)), FrameHint::Idle);
    assert!(!hint.is_open());

    hint.resume();
    assert_eq!(hint.frame(true, now + P60, ms(3), ms(2)), FrameHint::Opened);
    assert_eq!(fake.count(|c| matches!(c, Call::Create { .. })), 2);
}

#[test]
fn surface_destroy_closes_and_the_next_continuous_frame_reopens() {
    let (mut hint, fake) = session();
    let now = run(&mut hint, 1_000_000_000, P60, 3);
    hint.close();
    assert!(!hint.is_open());
    hint.surface_installed();
    assert_eq!(hint.frame(true, now, ms(3), ms(2)), FrameHint::Opened);
    assert_eq!(fake.count(|c| *c == Call::Close), 1);
}

#[test]
fn a_refused_session_is_asked_again_only_after_the_next_surface() {
    let (mut hint, fake) = session();
    *fake.refuse_create.borrow_mut() = true;
    assert_eq!(
        hint.frame(true, 1_000_000_000, ms(3), ms(2)),
        FrameHint::Refused
    );
    run(&mut hint, 1_000_000_000 + P60, P60, 5);
    assert_eq!(
        fake.count(|c| matches!(c, Call::Create { .. })),
        1,
        "a refusing platform is asked once per surface, not once per frame"
    );
    *fake.refuse_create.borrow_mut() = false;
    hint.surface_installed();
    assert_eq!(
        hint.frame(true, 2_000_000_000, ms(3), ms(2)),
        FrameHint::Opened
    );
}

#[test]
fn a_failed_report_closes_the_session_until_the_next_surface() {
    let (mut hint, fake) = session();
    run(&mut hint, 1_000_000_000, P60, 2);
    *fake.fail_report.borrow_mut() = true;
    assert_eq!(
        hint.frame(true, 1_000_000_000 + 2 * P60, ms(3), ms(2)),
        FrameHint::Dropped
    );
    assert!(!hint.is_open());
    assert_eq!(fake.count(|c| *c == Call::Close), 1);
    *fake.fail_report.borrow_mut() = false;
    assert_eq!(
        hint.frame(true, 1_000_000_000 + 3 * P60, ms(3), ms(2)),
        FrameHint::Idle
    );
    hint.surface_installed();
    assert_eq!(
        hint.frame(true, 1_000_000_000 + 4 * P60, ms(3), ms(2)),
        FrameHint::Opened
    );
}

#[test]
fn dropping_the_session_closes_it() {
    let (mut hint, fake) = session();
    run(&mut hint, 1_000_000_000, P60, 2);
    drop(hint);
    assert_eq!(fake.calls().last(), Some(&Call::Close));
}

// ---------------------------------------------------------------------
// Target
// ---------------------------------------------------------------------

#[test]
fn the_target_follows_the_display_period() {
    let (mut hint, fake) = session();
    // Opens at the default, then settles on the observed 120Hz period.
    run(&mut hint, 1_000_000_000, P120, 4);
    assert_eq!(
        fake.calls()
            .iter()
            .filter_map(|c| match c {
                Call::UpdateTarget(t) => Some(*t),
                _ => None,
            })
            .collect::<Vec<_>>(),
        vec![P120 as i64],
        "one retarget to the observed period, none for an unchanged one"
    );
}

#[test]
fn the_period_ignores_a_dropped_frame_and_adopts_a_slower_rate_once_the_window_turns() {
    let mut period = TargetPeriod::new();
    assert_eq!(period.period_nanos(), None);
    assert_eq!(period.target_nanos(), DEFAULT_TARGET_NANOS);

    let mut now = 1_000_000_000;
    for _ in 0..4 {
        period.observe(now);
        now += P120;
    }
    // A frame the render thread never saw: a two-period gap.
    now += P120;
    period.observe(now);
    assert_eq!(period.period_nanos(), Some(P120));

    // The display drops to 60Hz: the old period holds until the window of
    // recent deltas has turned over.
    for frame in 0..PERIOD_WINDOW {
        now += P60;
        period.observe(now);
        if frame + 1 < PERIOD_WINDOW - 1 {
            assert_eq!(period.period_nanos(), Some(P120), "frame {frame}");
        }
    }
    assert_eq!(period.period_nanos(), Some(P60));

    // A long idle gap is not a period.
    period.observe(now + 2_000_000_000);
    assert_eq!(period.period_nanos(), Some(P60));
}

#[test]
fn choreographer_jitter_does_not_retarget() {
    assert!(!target_moved(P60, P60 + 200_000));
    assert!(!target_moved(P60, P60 - 200_000));
    assert!(target_moved(P60, P120));
    assert!(target_moved(P120, P60));
}

// ---------------------------------------------------------------------
// Opt-in switch
// ---------------------------------------------------------------------

#[test]
fn frust_adpf_opts_in_from_either_source() {
    assert!(!adpf_opt_in(None, None));
    assert!(!adpf_opt_in(Some("0"), Some("0")));
    assert!(!adpf_opt_in(Some(""), None));
    assert!(adpf_opt_in(Some("1"), None));
    assert!(adpf_opt_in(None, Some("1")));
    assert!(adpf_opt_in(Some("0"), Some("yes")));
}

// ---------------------------------------------------------------------
// Wiring, on the source
// ---------------------------------------------------------------------

const ANDROID_GLUE: &str = include_str!("../src/jni_glue.rs");
const FRAME: &str = include_str!("../src/app/frame.rs");
const ADPF: &str = include_str!("../src/adpf.rs");
const LIB: &str = include_str!("../src/lib.rs");

/// `source` with every comment-only line dropped.
fn code(source: &str) -> String {
    source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The body of the first `fn` whose signature starts with `signature`.
fn fn_body<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("`{signature}` not found"));
    let open = start
        + source[start..]
            .find("{\n")
            .unwrap_or_else(|| panic!("`{signature}` has no body"));
    let mut depth = 0usize;
    for (offset, ch) in source[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[open + 1..open + offset];
                }
            }
            _ => {}
        }
    }
    panic!("`{signature}` body is unbalanced");
}

/// Byte offset of `needle` in `haystack`, failing with `what` when absent.
fn position(haystack: &str, needle: &str, what: &str) -> usize {
    haystack
        .find(needle)
        .unwrap_or_else(|| panic!("{what}: `{needle}` not found"))
}

#[test]
fn the_hint_is_armed_right_after_the_render_thread_is_scheduled() {
    let source = code(ANDROID_GLUE);
    let body = fn_body(&source, "pub(crate) fn render_loop(");
    let statements: Vec<&str> = body
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    assert_eq!(statements[0], "schedule_render_thread();");
    assert_eq!(
        statements[1],
        "let mut frame_hint = crate::adpf::RenderHint::open(ui_tid);"
    );
}

#[test]
fn the_ui_thread_id_is_captured_on_the_ui_thread_and_handed_to_the_render_loop() {
    let source = code(ANDROID_GLUE);
    let body = fn_body(&source, "fn spawn_split_executor(");
    let captured = position(
        body,
        "let ui_tid = crate::adpf::current_tid();",
        "spawn_split_executor",
    );
    let spawned = position(body, ".spawn(move ||", "spawn_split_executor");
    assert!(
        captured < spawned,
        "the id must be read before the render thread exists, on the calling (UI) thread"
    );
}

#[test]
fn the_session_follows_the_surface() {
    let source = code(ANDROID_GLUE);
    let body = fn_body(&source, "pub(crate) fn render_loop(");
    let installed = position(body, "hint.surface_installed();", "render_loop");
    let created = position(body, "RenderCommand::SurfaceCreated", "render_loop");
    let destroyed_arm = position(body, "RenderCommand::SurfaceDestroyed", "render_loop");
    let ack = destroyed_arm + position(&body[destroyed_arm..], "ack.acknowledge();", "render_loop");
    let closed = position(body, "hint.surface_destroyed();", "render_loop");
    assert!(created < installed && installed < destroyed_arm);
    assert!(
        ack < closed,
        "the session closes after the destroy ack, so the UI thread's barrier never waits on it"
    );
}

#[test]
fn each_rendered_frame_reports_after_render_scene_with_its_spans_timed() {
    let source = code(ANDROID_GLUE);
    let body = fn_body(&source, "pub(crate) fn render_loop(");
    let wanted = position(
        body,
        "let hint_frame = frame_hint.is_some() && crate::adpf::measures_frame_work();",
        "render_loop",
    );
    let rendered = position(body, "crate::app::render_scene(", "render_loop");
    let timed = position(body, "perf_on || hint_frame,", "render_loop");
    let reported = position(body, "hint.frame(", "render_loop");
    assert!(wanted < rendered && rendered < timed && timed < reported);
}

#[test]
fn pause_and_resume_drive_the_session() {
    let source = code(ANDROID_GLUE);
    assert!(fn_body(&source, "pub fn native_on_pause(").contains("crate::adpf::suspend();"));
    assert!(fn_body(&source, "pub fn native_on_resume(").contains("crate::adpf::resume();"));
}

#[test]
fn the_ui_thread_publishes_the_regime_before_its_skip_return() {
    let source = code(FRAME);
    let decided = position(
        &source,
        "let decision = self.frame_gate.decide_paced(inputs, pacing);",
        "frame.rs",
    );
    let published = position(
        &source,
        "crate::adpf::publish_frame_loop(self.frame_gate.is_continuous());",
        "frame.rs",
    );
    let skip = position(&source, "if decision.is_skip() {", "frame.rs");
    assert!(
        decided < published && published < skip,
        "a skipped tick must publish too, or an idle loop keeps reporting"
    );
    for span in ["rebuild_start", "layout_start", "paint_start"] {
        assert!(
            source.contains(&format!("let {span} = span_clock.then(Instant::now);")),
            "{span} is timed under the shared span clock"
        );
    }
}

#[test]
fn the_ndk_entry_points_are_resolved_never_linked() {
    let source = code(ADPF);
    assert!(
        !source.contains("extern \"C\" {"),
        "no extern block: every performance-hint symbol comes from dlsym"
    );
    for symbol in [
        "APerformanceHint_getManager",
        "APerformanceHint_createSession",
        "APerformanceHint_updateTargetWorkDuration",
        "APerformanceHint_reportActualWorkDuration",
        "APerformanceHint_closeSession",
        "APerformanceHint_reportActualWorkDuration2",
        "AWorkDuration_create",
        "AWorkDuration_release",
    ] {
        assert!(
            source.contains(&format!("c\"{symbol}\"")),
            "{symbol} is looked up by name"
        );
    }
    assert!(source.contains("libc::dlopen(c\"libandroid.so\""));
    // No other file reaches for the API.
    for (name, other) in [
        ("jni_glue.rs", ANDROID_GLUE),
        ("frame.rs", FRAME),
        ("lib.rs", LIB),
    ] {
        assert!(
            !code(other).contains("APerformanceHint_"),
            "{name} must go through crate::adpf"
        );
    }
}

#[test]
fn every_unsafe_site_in_the_hint_module_carries_a_safety_note() {
    let lines: Vec<&str> = ADPF.lines().collect();
    let mut sites = 0;
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") || !trimmed.contains("unsafe ") {
            continue;
        }
        // Function-pointer types name `unsafe extern "C" fn`; they are types,
        // not unsafe code.
        if trimmed.starts_with("type ") || trimmed.contains(": unsafe extern \"C\" fn") {
            continue;
        }
        sites += 1;
        let noted = lines[..index]
            .iter()
            .rev()
            .take_while(|previous| previous.trim_start().starts_with("//"))
            .any(|previous| previous.contains("SAFETY:"));
        assert!(
            noted,
            "adpf.rs:{}: `{}` needs a `// SAFETY:` comment directly above",
            index + 1,
            trimmed
        );
    }
    assert!(
        sites >= 8,
        "the scan found the module's unsafe sites ({sites})"
    );
}
