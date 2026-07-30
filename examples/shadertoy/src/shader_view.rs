//! `ShaderView`: the hand-rolled `View`/`Widget` pair painting a fullscreen,
//! continuously-animated fragment-shader quad ([`frust::authoring::scene::ShaderProgram`])
//! — the same hand-rolled pattern as
//! `benchmarks/frust_bench/src/scenarios/s1_animation/chart.rs`'s
//! `BubbleChart`.

use frust::authoring::scene::ShaderProgram;
use frust::authoring::{
    BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, Rect, Size, View,
    Widget,
};
use frust::{FrameTime, RwSignal, Set};

/// Wrap the shader's time origin at this many seconds to dodge `f32`
/// precision drift on a long-running session — periodic enough (an hour) to
/// be imperceptible in practice.
const TIME_WRAP_SECS: f64 = 3600.0;

/// The view descriptor: which compiled shader to paint, plus where to publish
/// the measured painted-frame rate.
pub struct ShaderView {
    pub program: ShaderProgram,
    pub fps_out: RwSignal<f64>,
}

/// Construct a [`ShaderView`].
pub fn shader_view(program: ShaderProgram, fps_out: RwSignal<f64>) -> ShaderView {
    ShaderView { program, fps_out }
}

/// One in-flight FPS measurement window: when it opened (on the shell clock),
/// how many times this widget has painted since, and — when the shell wired a
/// presented-frame counter (`PaintCtx::presented_frames`) — the counter value at
/// window open. `Δpresented / Δseconds` is the rate a user actually sees; the
/// paint count is the fallback when no counter is wired (bare-core tests,
/// pre-wiring shells).
struct FpsWindow {
    /// Window start on the shell clock.
    start: FrameTime,
    /// Frames this widget painted since `start` (the fallback measure).
    paints: u32,
    /// The shell's presented-frame count at `start`, if a counter was wired when
    /// the window opened. `None` selects the paint-count fallback on close.
    presented_at_start: Option<u64>,
}

/// The retained widget: the live program handle, the shader's own time
/// origin (first-seen frame time), and the FPS measurement window.
pub struct ShaderViewWidget {
    program: ShaderProgram,
    fps_out: RwSignal<f64>,
    size: Size,
    /// The frame time first observed by this widget instance — `time` fed to
    /// the shader is always relative to this, never an absolute shell clock
    /// value (see `frust_core::anim::FrameTime`'s doc).
    start: Option<FrameTime>,
    /// The live FPS measurement window (see [`FpsWindow`]).
    fps_window: Option<FpsWindow>,
}

impl<State: 'static> View<State> for ShaderView {
    type Element = ShaderViewWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> ShaderViewWidget {
        ShaderViewWidget {
            program: self.program.clone(),
            fps_out: self.fps_out,
            size: Size::ZERO,
            start: None,
            fps_window: None,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ShaderViewWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        element.fps_out = self.fps_out;
        if prev.program.id() != self.program.id() {
            element.program = self.program.clone();
            // A freshly switched-to shader restarts its own time origin and
            // FPS window rather than inheriting the previous shader's clock.
            element.start = None;
            element.fps_window = None;
            flags |= ChangeFlags::PAINT;
        }
        flags
    }
}

impl ShaderViewWidget {
    /// Count this painted frame and publish the measured rate ~once a second
    /// (identical cadence to
    /// `benchmarks/frust_bench/src/scenarios/s1_animation/chart.rs`'s
    /// `track_fps`).
    ///
    /// `presented` is the shell's running presented-frame count
    /// (`PaintCtx::presented_frames`), or `None` when no shell wired one. **When
    /// wired** the window publishes `Δpresented / Δseconds` — the rate the render
    /// thread actually *presents*, which under the render-thread split is far
    /// below this widget's own paint cadence (the OP9 HUD read ~121 while the
    /// render thread presented ~12; measuring presented deltas is the fix).
    /// **Otherwise** it falls back to the old paint-count-per-second measure.
    fn track_fps(&mut self, now: FrameTime, presented: Option<u64>) {
        match &mut self.fps_window {
            None => {
                self.fps_window = Some(FpsWindow {
                    start: now,
                    paints: 0,
                    presented_at_start: presented,
                });
            }
            Some(window) => {
                window.paints += 1;
                let elapsed = now.saturating_sub(window.start).as_secs_f64();
                if elapsed >= 1.0 {
                    // Presented-delta mode when a counter is wired at both ends of
                    // the window; else the paint-count fallback. `wrapping_sub`
                    // guards the (never-in-practice) u64 counter wrap.
                    let rate = match (window.presented_at_start, presented) {
                        (Some(at_start), Some(now_presented)) => {
                            now_presented.wrapping_sub(at_start) as f64 / elapsed
                        }
                        _ => window.paints as f64 / elapsed,
                    };
                    self.fps_out.set(rate);
                    self.fps_window = Some(FpsWindow {
                        start: now,
                        paints: 0,
                        presented_at_start: presented,
                    });
                }
            }
        }
    }
}

impl Widget for ShaderViewWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        self.size = bc.max();
        self.size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let origin = ctx.origin();
        let size = ctx.size();
        let dest = Rect::new(
            origin.x,
            origin.y,
            origin.x + size.width,
            origin.y + size.height,
        );

        let now = ctx.frame_time();
        let start = *self.start.get_or_insert(now);
        let elapsed_secs = now.saturating_sub(start).as_secs_f64() % TIME_WRAP_SECS;
        let time = elapsed_secs as f32;

        scene.draw_shader(&self.program, dest, time);

        self.track_fps(now, ctx.presented_frames());
        // Continuous animation: always ask for another frame while mounted,
        // matching `BubbleChartWidget`'s live-simulation contract.
        ctx.request_frame();
    }
}
