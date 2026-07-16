//! The winit 0.30 [`ApplicationHandler`] that drives a ForgeKit app in a desktop
//! preview window (spec §12.9).
//!
//! Ownership mirrors the shared platform bootstrap (spec §10.3): the shell owns
//! the [`RenderContext`], per-window [`SurfaceRenderer`], the shared
//! [`TextContext`], the [`RenderRoot`], and the application `State`/`app_logic`.
//! Each on-demand frame runs the framework passes in Masonry order (the v0
//! subset): rebuild → layout → paint → render.

use std::any::Any;
use std::sync::Arc;

use anyhow::{Context, Result};
use forgekit_core::RenderRoot;
use forgekit_core::event::{InputEvent, PointerButton, PointerEvent, PointerPhase, ScrollDelta};
use forgekit_core::view::View;
use forgekit_render::{FrameOutcome, RenderContext, SurfacePhase, SurfaceRenderer};
use forgekit_scene::{Scene, SceneBuilder};
use forgekit_text::TextContext;
use kurbo::{Affine, Point, Size};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowAttributes, WindowId};

/// Initial preview-window size, in logical pixels.
const INITIAL_SIZE: LogicalSize<u32> = LogicalSize::new(800, 600);
const WINDOW_TITLE: &str = "ForgeKit";

/// Run `app_logic` over `state` in a desktop preview window until it is closed.
///
/// Blocks the calling thread on the winit event loop. The event loop is
/// on-demand ([`ControlFlow::Wait`]): frames are produced only in response to a
/// redraw request (state change on rebuild, or a resize), never free-running
/// (spec §8).
pub fn run_desktop<State, Logic, V>(state: State, app_logic: Logic) -> Result<()>
where
    State: 'static,
    V: View<State>,
    Logic: FnMut(&mut State) -> V + 'static,
{
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Wait);

    let mut handler = ShellHandler {
        state,
        app_logic,
        root: RenderRoot::new(),
        text_ctx: TextContext::new(),
        render_cx: RenderContext::new(),
        scene: Scene::new(),
        cursor: Point::ZERO,
        window: None,
        // Starts in `NoSurface`; the surface is created in `resumed` once the
        // window exists, and driven through the §8.1 lifecycle from there.
        renderer: SurfaceRenderer::new(),
        fatal: None,
    };
    event_loop.run_app(&mut handler)?;
    finish(handler.fatal)
}

/// Turns a post-loop `ShellHandler::fatal` into the `run_desktop` result.
///
/// Pulled out of `run_desktop` so the "surface a stashed init error as `Err`"
/// behavior is unit-testable without driving a real winit event loop.
fn finish(fatal: Option<anyhow::Error>) -> Result<()> {
    match fatal {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

/// Convert a winit physical-pixel position into logical (density-independent)
/// pixels by the window's `scale_factor`.
///
/// winit reports `CursorMoved`/`PixelDelta` in **physical** pixels (verified
/// empirically on macOS — a HiDPI window reports positions at 2× the logical
/// point value); the widget tree lays out and hit-tests in the same logical
/// space the layout pass uses (spec §10.3), so every pointer coordinate is
/// divided by the scale factor at the shell boundary. Pulled out as a free
/// function so the conversion is unit-testable without a live window.
fn physical_to_logical(x: f64, y: f64, scale: f64) -> Point {
    Point::new(x / scale, y / scale)
}

/// Owns everything a running desktop app needs across frames.
struct ShellHandler<State: 'static, Logic, V: View<State>> {
    state: State,
    app_logic: Logic,
    root: RenderRoot<State, V>,
    text_ctx: TextContext,
    render_cx: RenderContext,
    /// Reused across frames; `reset()` each frame rather than reallocated.
    scene: Scene,
    /// Last known cursor position in logical pixels, updated on every
    /// `CursorMoved`. `MouseInput` (button press/release) carries no position of
    /// its own, so it reuses this — mirroring how winit models the two events.
    cursor: Point,
    /// Created lazily in `resumed()` (macOS requires window creation there).
    window: Option<Arc<Window>>,
    /// The §8.1 surface lifecycle machine; empty (`NoSurface`) until `resumed()`
    /// creates the surface, and torn down on `suspended()`.
    renderer: SurfaceRenderer,
    /// Set when `resumed` hits an unrecoverable init error; `event_loop.exit()`
    /// only stops the loop (it can't return an `Err`), so the error is stashed
    /// here and re-raised by `run_desktop` once `run_app` returns.
    fatal: Option<anyhow::Error>,
}

impl<State, Logic, V> ShellHandler<State, Logic, V>
where
    State: 'static,
    V: View<State>,
    Logic: FnMut(&mut State) -> V + 'static,
{
    /// Deliver one input event to the tree and schedule a frame if it dirtied
    /// state. This is the dirty-driven half of the desktop model (spec §8): the
    /// event pass never repaints, it only sets `needs_redraw`, which we turn into
    /// a single `request_redraw()` so the `Wait` loop wakes for exactly one frame.
    fn dispatch(&mut self, window: &Window, event: InputEvent) {
        let outcome = self.root.event(&mut self.state, &event);
        if outcome.needs_redraw {
            window.request_redraw();
        }
    }
}

impl<State, Logic, V> ApplicationHandler for ShellHandler<State, Logic, V>
where
    State: 'static,
    V: View<State>,
    Logic: FnMut(&mut State) -> V + 'static,
{
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        // The window is created once and reused; the surface, by contrast, is
        // (re)created here every time we resume — `suspended()` tears it down,
        // mirroring Android's surfaceDestroyed/surfaceCreated so the §8.1
        // machine stays exercised on desktop too.
        if self.window.is_none() {
            let attrs = WindowAttributes::default()
                .with_title(WINDOW_TITLE)
                .with_inner_size(INITIAL_SIZE);
            match event_loop.create_window(attrs) {
                Ok(window) => self.window = Some(Arc::new(window)),
                Err(err) => {
                    self.fatal =
                        Some(anyhow::Error::from(err).context("forgekit: failed to create window"));
                    event_loop.exit();
                    return;
                }
            }
        }

        let window = self
            .window
            .clone()
            .expect("window was just created or already present");

        if self.renderer.phase() != SurfacePhase::SurfaceReady {
            let size = window.inner_size();
            let (width, height) = (size.width.max(1), size.height.max(1));
            // spec §11: single-threaded here — the CPU/GPU render-thread split
            // lands in a later phase; for the preview shell one thread suffices.
            if let Err(err) = pollster::block_on(self.renderer.on_surface_created(
                &mut self.render_cx,
                window.clone(),
                width,
                height,
            ))
            .context("forgekit: failed to create render surface")
            {
                self.fatal = Some(err);
                event_loop.exit();
                return;
            }
        }

        window.request_redraw();
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        // Drop the surface on suspend (spec §8.1): rare on macOS, but keeps the
        // NoSurface path exercised on desktop and matches Android's lifecycle.
        self.renderer.on_surface_destroyed();
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(window) = self.window.clone() else {
            return;
        };

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),

            WindowEvent::Resized(size) => {
                // A resize with no live surface is dropped by the machine.
                self.renderer
                    .on_surface_changed(&self.render_cx, size.width, size.height);
                window.request_redraw();
            }

            // Track the cursor in logical space and dispatch a `Move`. We
            // dispatch on every move (not just while a button is down) so hover
            // handling can land later without a shell change; a widget that only
            // cares about drags simply ignores moves with no capture.
            WindowEvent::CursorMoved { position, .. } => {
                let scale = window.scale_factor();
                self.cursor = physical_to_logical(position.x, position.y, scale);
                self.dispatch(
                    &window,
                    InputEvent::Pointer(PointerEvent {
                        phase: PointerPhase::Move,
                        position: self.cursor,
                        button: PointerButton::Primary,
                    }),
                );
            }

            // Primary (left) button only in v1; other buttons are ignored until
            // secondary/middle gestures are specced. The press/release position
            // is the last `CursorMoved` position (winit carries none on the event).
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => {
                let phase = match state {
                    ElementState::Pressed => PointerPhase::Down,
                    ElementState::Released => PointerPhase::Up,
                };
                self.dispatch(
                    &window,
                    InputEvent::Pointer(PointerEvent {
                        phase,
                        position: self.cursor,
                        button: PointerButton::Primary,
                    }),
                );
            }

            // Wheel notches stay `Lines` (the ScrollView widget converts to px at
            // 40 px/line); precision-trackpad `PixelDelta` is physical, so divide
            // by the scale factor into logical pixels like every other coordinate.
            WindowEvent::MouseWheel { delta, .. } => {
                let scroll = match delta {
                    MouseScrollDelta::LineDelta(x, y) => ScrollDelta::Lines(x as f64, y as f64),
                    MouseScrollDelta::PixelDelta(px) => {
                        let scale = window.scale_factor();
                        ScrollDelta::Pixels(px.x / scale, px.y / scale)
                    }
                };
                self.dispatch(
                    &window,
                    InputEvent::Scroll {
                        position: self.cursor,
                        delta: scroll,
                    },
                );
            }

            WindowEvent::RedrawRequested => {
                // Rebuild the view tree every frame (app_logic is cheap by
                // construction, spec §5). A real dirty-tracking loop would skip
                // this when state is unchanged; the on-demand `Wait` control
                // flow already keeps us from free-running.
                let _flags = self.root.rebuild(&mut self.app_logic, &mut self.state);

                // HiDPI (spec task 08): lay out in logical pixels, then scale
                // the whole scene by the device pixel ratio so glyph outlines
                // are re-rasterised sharp at physical resolution.
                let physical = window.inner_size();
                let scale = window.scale_factor();
                let logical = Size::new(
                    physical.width as f64 / scale,
                    physical.height as f64 / scale,
                );
                let text_ctx: &mut dyn Any = &mut self.text_ctx;
                self.root.layout_with_text(logical, text_ctx);

                self.scene.reset();
                let paint_outcome = {
                    let mut builder = SceneBuilder::new(&mut self.scene);
                    builder.push_transform(Affine::scale(scale));
                    let outcome = self.root.paint(&mut builder);
                    builder.pop_transform();
                    outcome
                };

                // Animation driver (spec v1 seam): if paint advanced animation
                // state (e.g. a scroll fling) it asks for another frame here.
                // `ControlFlow::Wait` would otherwise idle with no pending input,
                // so we keep frames coming with an explicit redraw request until
                // the animation reaches rest and stops signalling.
                if paint_outcome.needs_frame {
                    window.request_redraw();
                }

                window.pre_present_notify();
                // Deliberately log-and-continue rather than fatal: a single
                // frame's render failure is most often a transient GPU/surface
                // hiccup, and killing the whole app on one bad frame would be
                // worse than skipping it. Turning *persistent* per-frame
                // failures into a fatal error is future work — see `fatal`.
                match self
                    .renderer
                    .render(&self.render_cx, &self.scene, peniko::Color::WHITE)
                {
                    // Stale swapchain (e.g. mid-resize): reconfigured internally,
                    // so ask for another frame against the fresh configuration.
                    Ok(FrameOutcome::Redraw) => window.request_redraw(),
                    // Surface lost (rare on desktop): recreate it from the same
                    // window and redraw. On failure, log and wait for the next
                    // event rather than killing the app.
                    Ok(FrameOutcome::SurfaceLost) => {
                        let size = window.inner_size();
                        let (width, height) = (size.width.max(1), size.height.max(1));
                        match pollster::block_on(self.renderer.on_surface_created(
                            &mut self.render_cx,
                            window.clone(),
                            width,
                            height,
                        )) {
                            Ok(()) => window.request_redraw(),
                            Err(err) => {
                                eprintln!("forgekit: failed to recreate surface: {err}");
                            }
                        }
                    }
                    Ok(FrameOutcome::Rendered | FrameOutcome::Skipped) => {}
                    Err(err) => eprintln!("forgekit: render error: {err}"),
                }
            }

            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{finish, physical_to_logical};
    use kurbo::Point;

    #[test]
    fn physical_to_logical_divides_by_scale() {
        // A 2× HiDPI display: a physical (200, 100) cursor is logical (100, 50).
        assert_eq!(
            physical_to_logical(200.0, 100.0, 2.0),
            Point::new(100.0, 50.0)
        );
    }

    #[test]
    fn physical_to_logical_is_identity_at_unit_scale() {
        // A non-HiDPI display: physical and logical coincide.
        assert_eq!(physical_to_logical(37.0, 12.0, 1.0), Point::new(37.0, 12.0));
    }

    #[test]
    fn finish_propagates_fatal_error_with_context() {
        let err = anyhow::anyhow!("boom").context("forgekit: failed to create window");
        let result = finish(Some(err));

        let err = result.expect_err("a stashed fatal error must surface as Err");
        assert_eq!(
            err.to_string(),
            "forgekit: failed to create window",
            "the outermost context string must be preserved"
        );
        assert_eq!(
            err.chain().last().unwrap().to_string(),
            "boom",
            "the underlying cause must still be reachable via the error chain"
        );
    }

    #[test]
    fn finish_is_ok_on_the_happy_path() {
        assert!(finish(None).is_ok());
    }
}
