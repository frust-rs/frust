//! The bubble-chart canvas — the benchmark's whole GPU workload, as one
//! hand-rolled `View`/`Widget` pair (the facade's documented low-level escape
//! hatch, the same pattern as huddle's `FillBox`/`Shimmer`).
//!
//! Per frame it paints exactly what the Flutter repro's `CustomPainter` does:
//! 60 radial-gradient circle fills + 60 stroked circle borders + two shaped
//! text runs per bubble. Physics advance during **paint** on the shell-fed
//! frame clock ([`PaintCtx::frame_time`]'s pass), one step per painted frame —
//! the same cadence as the repro's per-vsync `Ticker` — and the widget calls
//! [`PaintCtx::request_frame`] while the simulation is live, so frames keep
//! coming with no external input and stop entirely (frame-gate `Skip` on
//! mobile, `ControlFlow::Wait` idle on desktop) once every bubble settles.
//!
//! Text is shaped ONCE per bubble at init time (symbols, percentages, and
//! font sizes are all fixed per bubble) during the layout pass, where the
//! shell-threaded [`TextContext`] lives; paint just replays the cached
//! [`TextLayout`]s as glyph runs. The repro hand-rolls the same caching with
//! `TextPainter`s.

use frust::{FrameTime, RwSignal, Set};
use frust_core::{
    BoxConstraints, BuildCtx, ChangeFlags, EventCtx, EventResult, InputEvent, LayoutCtx, PaintCtx,
    PaintScene, PointerPhase, View, Widget,
};
use frust_text::{FontWeight, TextContext, TextLayout, TextStyle};
use kurbo::{BezPath, Circle, Point, Shape, Size};
use peniko::color::DynamicColor;
use peniko::{Brush, Color, ColorStop, Gradient};

/// How many bubbles the benchmark runs — the repro's count.
pub const BUBBLE_COUNT: usize = 60;
/// The fixed seed, matching the repro's `Random(42)`.
pub const SEED: u64 = 42;

/// The repro's Scaffold background (`0xFF0D1421`).
const BACKGROUND: Color = Color::from_rgb8(0x0D, 0x14, 0x21);
/// `Colors.green.shade400` / `Colors.red.shade400` / `Colors.grey`.
const GREEN: Color = Color::from_rgb8(0x66, 0xBB, 0x6A);
const RED: Color = Color::from_rgb8(0xEF, 0x53, 0x50);
const GREY: Color = Color::from_rgb8(0x9E, 0x9E, 0x9E);

// Paint constants — verbatim from the repro's `_drawBubble`.
const FILL_CENTER_OPACITY: f32 = 0.7;
const FILL_EDGE_OPACITY: f32 = 0.3;
const BORDER_OPACITY: f32 = 0.6;
const BORDER_WIDTH: f64 = 2.0;

use crate::physics::BubblePhysics;

/// The view descriptor: benchmark controls in, measured FPS out.
pub struct BubbleChart {
    /// Whether the simulation steps (the Pause/Play toggle).
    pub running: bool,
    /// Bumping this reseeds the field (the Reset button).
    pub epoch: u64,
    /// Written roughly once per second with the measured painted-frame rate.
    pub fps_out: RwSignal<f64>,
}

/// Construct a [`BubbleChart`].
pub fn bubble_chart(running: bool, epoch: u64, fps_out: RwSignal<f64>) -> BubbleChart {
    BubbleChart {
        running,
        epoch,
        fps_out,
    }
}

/// Per-bubble cached render data, built once per init/reset: the circle path
/// (centered on the origin; paint translates it to the live position) and the
/// two shaped text layouts with their measured sizes.
struct BubbleVisual {
    circle: BezPath,
    base_color: Color,
    symbol: TextLayout,
    symbol_size: Size,
    /// The repro skips a bubble's text when the symbol outgrows the bubble
    /// (`width > radius * 1.6`).
    symbol_too_wide: bool,
    percent: TextLayout,
    percent_size: Size,
}

/// The retained widget: the simulation, the per-bubble visual cache, and the
/// FPS window.
pub struct BubbleChartWidget {
    running: bool,
    epoch: u64,
    fps_out: RwSignal<f64>,
    physics: BubblePhysics,
    visuals: Vec<BubbleVisual>,
    size: Size,
    needs_init: bool,
    /// FPS measurement: window start on the shell clock + frames painted since.
    fps_window: Option<(FrameTime, u32)>,
}

impl<State: 'static> View<State> for BubbleChart {
    type Element = BubbleChartWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> BubbleChartWidget {
        BubbleChartWidget {
            running: self.running,
            epoch: self.epoch,
            fps_out: self.fps_out,
            physics: BubblePhysics::default(),
            visuals: Vec::new(),
            size: Size::ZERO,
            needs_init: true,
            fps_window: None,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut BubbleChartWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        element.fps_out = self.fps_out;
        if prev.epoch != self.epoch {
            element.epoch = self.epoch;
            element.needs_init = true;
            // Re-init happens in layout, where the shaping context lives.
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.running != self.running {
            element.running = self.running;
            if self.running {
                // Play-after-pause restarts settle tracking, like the repro's
                // `wakeFromSettled` on toggle.
                element.physics.wake_from_settled();
            }
            flags |= ChangeFlags::PAINT;
        }
        flags
    }
}

impl BubbleChartWidget {
    /// Whether the simulation has fully settled (test/diagnostic seam).
    pub fn is_settled(&self) -> bool {
        self.physics.is_fully_settled
    }

    /// The repro's `_getScaledFontSize`: sqrt-eased between 8px and 24px over
    /// the 18..60 radius range.
    fn scaled_font_size(radius: f64) -> f32 {
        const MIN_R: f64 = 18.0;
        const MAX_R: f64 = 60.0;
        const MIN_FS: f64 = 8.0;
        const MAX_FS: f64 = 24.0;
        if radius <= MIN_R {
            return MIN_FS as f32;
        }
        if radius >= MAX_R {
            return MAX_FS as f32;
        }
        let normalized = (radius - MIN_R) / (MAX_R - MIN_R);
        (MIN_FS + normalized.sqrt() * (MAX_FS - MIN_FS)) as f32
    }

    fn bubble_color(performance: f64) -> Color {
        if performance > 0.0 {
            GREEN
        } else if performance < 0.0 {
            RED
        } else {
            GREY
        }
    }

    /// Reseed the field and rebuild the per-bubble visual cache (circle paths
    /// + shaped text). Runs inside layout, the pass that owns `TextContext`.
    fn init(&mut self, text_ctx: &mut TextContext) {
        self.physics.initialize_bubbles(BUBBLE_COUNT, SEED);
        self.visuals = self
            .physics
            .bubbles
            .iter()
            .map(|b| {
                let base_color = Self::bubble_color(b.performance);
                let font_size = Self::scaled_font_size(b.radius);

                let symbol_style = TextStyle {
                    weight: FontWeight::BOLD,
                    ..TextStyle::new(font_size, Color::WHITE)
                };
                let symbol = text_ctx.layout(b.symbol, &symbol_style, None);
                let symbol_size = symbol.size();

                let percent_style = TextStyle {
                    weight: FontWeight::SEMI_BOLD,
                    ..TextStyle::new(font_size * 0.7, base_color)
                };
                let percent_text = format!("{:+.1}%", b.performance);
                let percent = text_ctx.layout(&percent_text, &percent_style, None);
                let percent_size = percent.size();

                BubbleVisual {
                    circle: Circle::new(Point::ZERO, b.radius).to_path(0.1),
                    base_color,
                    symbol,
                    symbol_size,
                    symbol_too_wide: symbol_size.width > b.radius * 1.6,
                    percent,
                    percent_size,
                }
            })
            .collect();
        self.needs_init = false;
        self.fps_window = None;
    }

    /// Count this painted frame and publish the measured rate ~once a second.
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

impl Widget for BubbleChartWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let size = bc.max();
        if size != self.size {
            self.size = size;
            self.physics.set_size(size.width, size.height);
        }
        if self.needs_init && size.width > 0.0 && size.height > 0.0 {
            let text_ctx = ctx.text_context::<TextContext>();
            self.init(text_ctx);
        }
        size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let origin = ctx.origin();
        scene.fill_rect(origin, ctx.size(), BACKGROUND);

        // Advance the simulation one step per painted frame while live, and
        // keep frames coming; a settled (or paused) field paints statically
        // and requests nothing, so the app goes fully idle.
        if self.running && self.physics.needs_repaint() {
            self.physics.update();
            self.track_fps(ctx.frame_time());
            ctx.request_frame();
        } else {
            self.fps_window = None;
        }

        for (b, vis) in self.physics.bubbles.iter().zip(&self.visuals) {
            let center = Point::new(origin.x + b.x, origin.y + b.y);
            let r = b.radius;

            // Radial-gradient fill: center offset to (-0.2, -0.2) of the
            // radius, gradient radius 0.95r, 0.7→0.3 alpha — the repro's
            // `RadialGradient` verbatim. Brush geometry is in absolute scene
            // coordinates (the recorded command transform is identity here).
            let fill =
                Gradient::new_radial((center.x - 0.2 * r, center.y - 0.2 * r), (0.95 * r) as f32)
                    .with_stops([
                        ColorStop {
                            offset: 0.0,
                            color: DynamicColor::from_alpha_color(
                                vis.base_color.with_alpha(FILL_CENTER_OPACITY),
                            ),
                        },
                        ColorStop {
                            offset: 1.0,
                            color: DynamicColor::from_alpha_color(
                                vis.base_color.with_alpha(FILL_EDGE_OPACITY),
                            ),
                        },
                    ]);
            scene.fill_path(center, &vis.circle, &Brush::Gradient(fill));

            scene.stroke_path(
                center,
                &vis.circle,
                BORDER_WIDTH,
                &Brush::Solid(vis.base_color.with_alpha(BORDER_OPACITY)),
            );

            if vis.symbol_too_wide {
                continue;
            }
            // Symbol above center, percentage below — the repro's offsets.
            let symbol_origin = Point::new(
                center.x - vis.symbol_size.width / 2.0,
                center.y - vis.symbol_size.height / 2.0 - r * 0.12,
            );
            for run in vis.symbol.to_scene_runs(symbol_origin) {
                scene.draw_glyph_run(run);
            }
            let percent_origin =
                Point::new(center.x - vis.percent_size.width / 2.0, center.y + r * 0.1);
            for run in vis.percent.to_scene_runs(percent_origin) {
                scene.draw_glyph_run(run);
            }
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let InputEvent::Pointer(pointer) = event else {
            return EventResult::Ignored;
        };
        match pointer.phase {
            PointerPhase::Down => {
                ctx.capture_pointer();
                self.physics.set_touch(Some(pointer.position));
                self.physics.wake_from_settled();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                if self.physics.touch().is_some() {
                    self.physics.set_touch(Some(pointer.position));
                    ctx.request_redraw();
                    EventResult::Handled
                } else {
                    EventResult::Ignored
                }
            }
            PointerPhase::Up | PointerPhase::Cancel => {
                if self.physics.touch().is_some() {
                    self.physics.set_touch(None);
                    ctx.request_redraw();
                    EventResult::Handled
                } else {
                    EventResult::Ignored
                }
            }
        }
    }
}
