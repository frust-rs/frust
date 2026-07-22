//! `ShaderView`: the escape-hatch `View`/`Widget` pair painting a fullscreen,
//! continuously-animated fragment-shader quad ([`frust_scene::ShaderProgram`])
//! — the same hand-rolled pattern as `examples/bubblebench/src/chart.rs`'s
//! `BubbleChart`.

use frust::{FrameTime, RwSignal, Set};
use frust_core::{
    BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, View, Widget,
};
use frust_scene::ShaderProgram;
use kurbo::{Rect, Size};

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
    /// FPS measurement: window start on the shell clock + frames painted since.
    fps_window: Option<(FrameTime, u32)>,
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
    /// (identical cadence to `examples/bubblebench/src/chart.rs`'s
    /// `track_fps`).
    fn track_fps(&mut self, now: FrameTime) {
        match self.fps_window {
            None => self.fps_window = Some((now, 0)),
            Some((start, frames)) => {
                let frames = frames + 1;
                let elapsed = now.saturating_sub(start);
                if elapsed.as_secs_f64() >= 1.0 {
                    self.fps_out.set(frames as f64 / elapsed.as_secs_f64());
                    self.fps_window = Some((now, 0));
                } else {
                    self.fps_window = Some((start, frames));
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

        self.track_fps(now);
        // Continuous animation: always ask for another frame while mounted,
        // matching `BubbleChartWidget`'s live-simulation contract.
        ctx.request_frame();
    }
}
