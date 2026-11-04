//! The animation time scale at the `RenderRoot` level: the scaled
//! [`PaintCtx::frame_time`] beside the raw [`PaintCtx::input_time`].
//!
//! No test here stores a scale other than `1.0` in the process-wide slot, so
//! parallel tests never observe one; scaled runs go through the thread-local
//! [`with_thread_animation_time_scale`].

use super::*;
use crate::anim::{
    MAX_ANIMATION_TIME_SCALE, MIN_ANIMATION_TIME_SCALE, animation_time_scale,
    set_animation_time_scale, with_thread_animation_time_scale,
};

/// The `(frame_time, input_time)` pair the probe saw on its latest paint.
type Seen = Rc<Cell<(FrameTime, FrameTime)>>;

struct ClockProbe(Seen);

impl crate::widget::Widget for ClockProbe {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(10.0, 10.0))
    }
    fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
        self.0.set((ctx.frame_time(), ctx.input_time()));
    }
}

struct ClockProbeView(Seen);

impl View<()> for ClockProbeView {
    type Element = ClockProbe;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> ClockProbe {
        ClockProbe(Rc::clone(&self.0))
    }
    fn rebuild(&self, _prev: &Self, _el: &mut ClockProbe, _ctx: &mut BuildCtx<'_>) -> ChangeFlags {
        ChangeFlags::NONE
    }
}

struct Probe {
    root: RenderRoot<(), ClockProbeView>,
    seen: Seen,
}

impl Probe {
    fn new() -> Self {
        let seen: Seen = Rc::new(Cell::new((FrameTime::ZERO, FrameTime::ZERO)));
        let mut root = RenderRoot::new();
        let view_seen = Rc::clone(&seen);
        root.rebuild(&mut |_| ClockProbeView(Rc::clone(&view_seen)), &mut ());
        root.layout(Size::new(100.0, 100.0));
        Self { root, seen }
    }

    /// Paint at raw `nanos` under `scale`; returns `(frame_time, input_time)`.
    fn paint(&mut self, nanos: u64, scale: f64) -> (FrameTime, FrameTime) {
        with_thread_animation_time_scale(scale, || {
            let mut scene = RecordingScene::default();
            self.root.paint(&mut scene, FrameTime::from_nanos(nanos));
        });
        self.seen.get()
    }
}

const MS: u64 = 1_000_000;

#[test]
fn scale_one_is_an_exact_identity() {
    let mut probe = Probe::new();
    let mut raw = 7 * MS;
    let mut lcg: u64 = 0x2545_F491_4F6C_DD1D;
    for _ in 0..1000 {
        lcg = lcg.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        raw += 1 + (lcg >> 40) % (40 * MS);
        let (frame, input) = probe.paint(raw, 1.0);
        assert_eq!(frame, input);
        assert_eq!(input, FrameTime::from_nanos(raw));
    }
}

#[test]
fn scaled_clock_runs_at_the_set_rate() {
    let mut probe = Probe::new();
    let (frame0, input0) = probe.paint(1_000 * MS, 0.25);
    let mut last = (frame0, input0);
    for step in 1..=25 {
        last = probe.paint(1_000 * MS + step * 16 * MS, 0.25);
    }
    assert_eq!(last.0.saturating_sub(frame0).as_nanos(), (100 * MS) as u128);
    assert_eq!(last.1.saturating_sub(input0).as_nanos(), (400 * MS) as u128);
}

#[test]
fn scale_change_is_continuous() {
    let mut probe = Probe::new();
    let mut raw = 50 * MS;
    let (mut prev_frame, _) = probe.paint(raw, 1.0);
    for (scale, frames) in [(1.0, 10), (0.5, 10), (2.0, 10), (1.0, 10)] {
        for _ in 0..frames {
            raw += 16 * MS + 333;
            let (frame, _) = probe.paint(raw, scale);
            assert!(frame >= prev_frame, "the animation clock moved backwards");
            let step = frame.saturating_sub(prev_frame).as_nanos() as f64;
            let expected = ((16 * MS + 333) as f64 * scale).round();
            assert_eq!(step, expected, "a {scale} step jumped");
            prev_frame = frame;
        }
    }
}

#[test]
fn a_backward_raw_step_never_rewinds_a_scaled_clock() {
    let mut probe = Probe::new();
    probe.paint(100 * MS, 0.5);
    let (forward, _) = probe.paint(200 * MS, 0.5);
    let (after_rewind, input) = probe.paint(150 * MS, 0.5);
    assert_eq!(after_rewind, forward);
    assert_eq!(input, FrameTime::from_nanos(150 * MS));
    let (resumed, _) = probe.paint(170 * MS, 0.5);
    assert_eq!(
        resumed.saturating_sub(forward).as_nanos(),
        (10 * MS) as u128
    );
}

#[test]
fn returning_to_scale_one_continues_one_to_one_after_a_scaled_run() {
    let mut probe = Probe::new();
    let mut raw = 40 * MS;
    probe.paint(raw, 1.0);
    for _ in 0..8 {
        raw += 16 * MS + 7;
        probe.paint(raw, 0.5);
    }
    raw += 16 * MS;
    let (mut prev_frame, mut prev_input) = probe.paint(raw, 1.0);
    assert_ne!(prev_frame, prev_input, "the scaled offset must persist");
    for step in 1..=20u64 {
        raw += 9 * MS + step * 113;
        let (frame, input) = probe.paint(raw, 1.0);
        assert_eq!(
            frame.saturating_sub(prev_frame),
            input.saturating_sub(prev_input),
            "a scale-1.0 frame after a scaled run must advance one to one"
        );
        (prev_frame, prev_input) = (frame, input);
    }
}

#[test]
fn a_backward_raw_step_after_a_scaled_run_never_rewinds() {
    let mut probe = Probe::new();
    probe.paint(100 * MS, 1.0);
    probe.paint(200 * MS, 0.5);
    let (forward, _) = probe.paint(300 * MS, 1.0);
    let (after_rewind, input) = probe.paint(120 * MS, 1.0);
    assert_eq!(input, FrameTime::from_nanos(120 * MS));
    assert_eq!(after_rewind, forward, "the clock rewound to raw time");
    let (resumed, _) = probe.paint(135 * MS, 1.0);
    assert_eq!(
        resumed.saturating_sub(forward).as_nanos(),
        (15 * MS) as u128
    );
}

#[test]
fn out_of_range_scale_is_rejected() {
    for bad in [
        0.0,
        -1.0,
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        1000.0,
    ] {
        assert!(!set_animation_time_scale(bad), "{bad} was accepted");
        assert_eq!(animation_time_scale().to_bits(), 1.0f64.to_bits());
    }
    for edge in [MIN_ANIMATION_TIME_SCALE, MAX_ANIMATION_TIME_SCALE] {
        assert!(crate::anim::is_valid_time_scale(edge));
    }
    assert!(!crate::anim::is_valid_time_scale(
        MIN_ANIMATION_TIME_SCALE / 2.0
    ));
    assert!(!crate::anim::is_valid_time_scale(
        MAX_ANIMATION_TIME_SCALE * 1.01
    ));
    assert!(set_animation_time_scale(1.0));
}

#[test]
fn a_thread_scale_stays_on_its_thread_and_unwinds() {
    with_thread_animation_time_scale(0.25, || {
        assert_eq!(animation_time_scale(), 0.25);
        let other = std::thread::spawn(animation_time_scale).join().unwrap();
        assert_eq!(other, 1.0);
    });
    assert_eq!(animation_time_scale(), 1.0);
}
