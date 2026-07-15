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
use forgekit_core::view::View;
use forgekit_render::{RenderContext, SurfaceRenderer};
use forgekit_scene::{Scene, SceneBuilder};
use forgekit_text::TextContext;
use kurbo::{Affine, Size};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
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
        window: None,
        renderer: None,
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

/// Owns everything a running desktop app needs across frames.
struct ShellHandler<State: 'static, Logic, V: View<State>> {
    state: State,
    app_logic: Logic,
    root: RenderRoot<State, V>,
    text_ctx: TextContext,
    render_cx: RenderContext,
    /// Reused across frames; `reset()` each frame rather than reallocated.
    scene: Scene,
    /// Created lazily in `resumed()` (macOS requires window creation there).
    window: Option<Arc<Window>>,
    /// Created alongside the window once a surface is available.
    renderer: Option<SurfaceRenderer>,
    /// Set when `resumed` hits an unrecoverable init error; `event_loop.exit()`
    /// only stops the loop (it can't return an `Err`), so the error is stashed
    /// here and re-raised by `run_desktop` once `run_app` returns.
    fatal: Option<anyhow::Error>,
}

impl<State, Logic, V> ApplicationHandler for ShellHandler<State, Logic, V>
where
    State: 'static,
    V: View<State>,
    Logic: FnMut(&mut State) -> V + 'static,
{
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        // `resumed` can fire more than once; only bootstrap the surface once.
        if self.window.is_some() {
            return;
        }

        let attrs = WindowAttributes::default()
            .with_title(WINDOW_TITLE)
            .with_inner_size(INITIAL_SIZE);
        let window = match event_loop.create_window(attrs) {
            Ok(window) => Arc::new(window),
            Err(err) => {
                self.fatal =
                    Some(anyhow::Error::from(err).context("forgekit: failed to create window"));
                event_loop.exit();
                return;
            }
        };

        let size = window.inner_size();
        let (width, height) = (size.width.max(1), size.height.max(1));
        // spec §11: single-threaded here — the CPU/GPU render-thread split lands
        // in a later phase; for the preview shell one thread is sufficient.
        let renderer =
            match pollster::block_on(self.render_cx.create_surface(window.clone(), width, height))
                .context("forgekit: failed to create render surface")
            {
                Ok(renderer) => renderer,
                Err(err) => {
                    self.fatal = Some(err);
                    event_loop.exit();
                    return;
                }
            };

        self.window = Some(window.clone());
        self.renderer = Some(renderer);
        window.request_redraw();
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        let (Some(window), Some(renderer)) = (self.window.as_ref(), self.renderer.as_mut()) else {
            return;
        };

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),

            WindowEvent::Resized(size) => {
                renderer.resize(&mut self.render_cx, size.width, size.height);
                window.request_redraw();
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
                {
                    let mut builder = SceneBuilder::new(&mut self.scene);
                    builder.push_transform(Affine::scale(scale));
                    self.root.paint(&mut builder);
                    builder.pop_transform();
                }

                window.pre_present_notify();
                // Deliberately log-and-continue rather than fatal: a single
                // frame's render failure is most often a transient GPU/surface
                // hiccup (e.g. an out-of-date swapchain image), and killing the
                // whole app on one bad frame would be worse than skipping it.
                // Turning *persistent* per-frame failures into a fatal error is
                // future work — see `fatal` field, used for shell init errors.
                if let Err(err) =
                    renderer.render(&self.render_cx, &self.scene, peniko::Color::WHITE)
                {
                    eprintln!("forgekit: render error: {err}");
                }
            }

            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::finish;

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
