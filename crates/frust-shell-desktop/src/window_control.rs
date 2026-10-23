//! Executes the app's queued [`WindowCommand`]s against the window and snapshots the
//! [`WindowState`] the shell publishes. The window sits behind [`WindowOps`] so this is
//! testable without a live event loop; `app_handler` owns when it runs (after each input
//! dispatch and once per frame) and the close path a `RequestClose` takes.

use std::sync::atomic::{AtomicBool, Ordering};

use frust_core::{ResizeEdge, WindowCommand, WindowState};
use winit::window::{Fullscreen, ResizeDirection, Window};

/// Latched after the first failed move drag, so a refusing platform logs one line.
static MOVE_FAILURE_WARNED: AtomicBool = AtomicBool::new(false);
/// Latched after the first failed resize drag (every one, on macOS).
static RESIZE_FAILURE_WARNED: AtomicBool = AtomicBool::new(false);

/// The window operations a [`WindowCommand`] needs; errors are strings so a fake needs no
/// winit error type.
pub(crate) trait WindowOps {
    fn drag_window(&self) -> Result<(), String>;
    fn drag_resize_window(&self, direction: ResizeDirection) -> Result<(), String>;
    fn set_maximized(&self, maximized: bool);
    fn is_maximized(&self) -> bool;
    fn set_minimized(&self, minimized: bool);
    fn is_minimized(&self) -> bool;
    fn set_fullscreen(&self, fullscreen: bool);
    fn is_fullscreen(&self) -> bool;
    fn has_focus(&self) -> bool;
}

impl WindowOps for Window {
    fn drag_window(&self) -> Result<(), String> {
        Window::drag_window(self).map_err(|e| e.to_string())
    }

    fn drag_resize_window(&self, direction: ResizeDirection) -> Result<(), String> {
        Window::drag_resize_window(self, direction).map_err(|e| e.to_string())
    }

    fn set_maximized(&self, maximized: bool) {
        Window::set_maximized(self, maximized);
    }

    fn is_maximized(&self) -> bool {
        Window::is_maximized(self)
    }

    fn set_minimized(&self, minimized: bool) {
        Window::set_minimized(self, minimized);
    }

    /// Wayland cannot report minimization (`None`), read as not minimized.
    fn is_minimized(&self) -> bool {
        Window::is_minimized(self).unwrap_or(false)
    }

    fn set_fullscreen(&self, fullscreen: bool) {
        Window::set_fullscreen(self, fullscreen.then_some(Fullscreen::Borderless(None)));
    }

    fn is_fullscreen(&self) -> bool {
        Window::fullscreen(self).is_some()
    }

    fn has_focus(&self) -> bool {
        Window::has_focus(self)
    }
}

/// The winit direction for a frust resize edge.
pub(crate) fn map_edge(edge: ResizeEdge) -> ResizeDirection {
    match edge {
        ResizeEdge::North => ResizeDirection::North,
        ResizeEdge::NorthEast => ResizeDirection::NorthEast,
        ResizeEdge::East => ResizeDirection::East,
        ResizeEdge::SouthEast => ResizeDirection::SouthEast,
        ResizeEdge::South => ResizeDirection::South,
        ResizeEdge::SouthWest => ResizeDirection::SouthWest,
        ResizeEdge::West => ResizeDirection::West,
        ResizeEdge::NorthWest => ResizeDirection::NorthWest,
    }
}

/// Whether this is the first failure `guard` has seen (and so the one to log).
fn first_failure(guard: &AtomicBool) -> bool {
    !guard.swap(true, Ordering::Relaxed)
}

/// Run `commands` in order against `window`. Returns whether any was `RequestClose`; the
/// caller then runs its close path once. A refused drag is logged once per kind, never fatal.
pub(crate) fn execute(commands: &[WindowCommand], window: &impl WindowOps) -> bool {
    let mut close = false;
    // The maximized state this pass has requested so far, seeded from the window on first
    // need. A platform may apply `set_maximized` asynchronously, so a toggle reads this and
    // not `is_maximized()`: two toggles in one batch cancel out.
    let mut requested_maximized: Option<bool> = None;
    for command in commands {
        match *command {
            WindowCommand::BeginMove => {
                if let Err(err) = window.drag_window()
                    && first_failure(&MOVE_FAILURE_WARNED)
                {
                    log::warn!(
                        "frust-shell-desktop: the window move drag was refused ({err}); macOS \
                         needs it pushed from a primary pointer-down"
                    );
                }
            }
            WindowCommand::BeginResize(edge) => {
                if let Err(err) = window.drag_resize_window(map_edge(edge))
                    && first_failure(&RESIZE_FAILURE_WARNED)
                {
                    log::warn!(
                        "frust-shell-desktop: the window resize drag was refused ({err}); \
                         expected on macOS, where AppKit keeps its own edge resizing"
                    );
                }
            }
            WindowCommand::SetMaximized(maximized) => {
                window.set_maximized(maximized);
                requested_maximized = Some(maximized);
            }
            WindowCommand::ToggleMaximized => {
                let next = !requested_maximized.unwrap_or_else(|| window.is_maximized());
                window.set_maximized(next);
                requested_maximized = Some(next);
            }
            WindowCommand::Minimize => window.set_minimized(true),
            WindowCommand::SetFullscreen(fullscreen) => window.set_fullscreen(fullscreen),
            WindowCommand::RequestClose => close = true,
        }
    }
    close
}

/// Take every queued command and execute it. `None` when the queue was empty, so the caller
/// skips its state publish; otherwise whether a close was requested.
pub(crate) fn drain(window: &impl WindowOps) -> Option<bool> {
    let commands = frust_core::take_window_commands();
    if commands.is_empty() {
        return None;
    }
    Some(execute(&commands, window))
}

/// The window's current state, as the app-facing context carries it.
pub(crate) fn window_state_snapshot(window: &impl WindowOps) -> WindowState {
    WindowState::new(
        window.is_maximized(),
        window.is_fullscreen(),
        window.has_focus(),
        window.is_minimized(),
    )
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, MutexGuard};

    use frust_core::push_window_command;

    use super::*;

    /// The command queue and the once-guards are process-global, so these tests run one at a
    /// time and start from an empty queue.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn lock_and_drain() -> MutexGuard<'static, ()> {
        let guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        drain(&FakeWindow::default());
        guard
    }

    /// Records the calls `execute` makes, standing in for the `winit::Window` a host test
    /// cannot construct. Reads are not recorded; they answer from the flags.
    #[derive(Debug, Default)]
    struct FakeWindow {
        calls: Mutex<Vec<String>>,
        maximized: bool,
        fullscreen: bool,
        focused: bool,
        minimized: bool,
        refuse_drags: bool,
    }

    impl FakeWindow {
        fn calls(&self) -> Vec<String> {
            self.calls.lock().expect("test mutex").clone()
        }

        fn record(&self, call: String) {
            self.calls.lock().expect("test mutex").push(call);
        }

        fn drag_result(&self) -> Result<(), String> {
            if self.refuse_drags {
                Err("not supported".to_string())
            } else {
                Ok(())
            }
        }
    }

    impl WindowOps for FakeWindow {
        fn drag_window(&self) -> Result<(), String> {
            self.record("drag_window".to_string());
            self.drag_result()
        }

        fn drag_resize_window(&self, direction: ResizeDirection) -> Result<(), String> {
            self.record(format!("drag_resize_window({direction:?})"));
            self.drag_result()
        }

        fn set_maximized(&self, maximized: bool) {
            self.record(format!("set_maximized({maximized})"));
        }

        fn is_maximized(&self) -> bool {
            self.maximized
        }

        fn set_minimized(&self, minimized: bool) {
            self.record(format!("set_minimized({minimized})"));
        }

        fn is_minimized(&self) -> bool {
            self.minimized
        }

        fn set_fullscreen(&self, fullscreen: bool) {
            self.record(format!("set_fullscreen({fullscreen})"));
        }

        fn is_fullscreen(&self) -> bool {
            self.fullscreen
        }

        fn has_focus(&self) -> bool {
            self.focused
        }
    }

    fn run(commands: &[WindowCommand], window: &FakeWindow) -> (bool, Vec<String>) {
        let close = execute(commands, window);
        (close, window.calls())
    }

    #[test]
    fn drag_commands_start_the_matching_os_drag() {
        let _guard = lock_and_drain();
        assert_eq!(
            run(&[WindowCommand::BeginMove], &FakeWindow::default()),
            (false, vec!["drag_window".to_string()])
        );
        assert_eq!(
            run(
                &[WindowCommand::BeginResize(ResizeEdge::East)],
                &FakeWindow::default()
            ),
            (false, vec!["drag_resize_window(East)".to_string()])
        );
    }

    #[test]
    fn toggle_maximized_sets_the_opposite_of_the_current_state() {
        let maximized = FakeWindow {
            maximized: true,
            ..FakeWindow::default()
        };
        let (_, calls) = run(&[WindowCommand::ToggleMaximized], &maximized);
        assert_eq!(calls, ["set_maximized(false)"]);
        let (_, calls) = run(&[WindowCommand::ToggleMaximized], &FakeWindow::default());
        assert_eq!(calls, ["set_maximized(true)"]);
    }

    #[test]
    fn two_toggles_in_one_batch_cancel_out_before_the_platform_applies_them() {
        // `FakeWindow` never updates its maximized flag on `set_maximized`, standing in for a
        // platform that applies the request asynchronously.
        let (_, calls) = run(
            &[
                WindowCommand::ToggleMaximized,
                WindowCommand::ToggleMaximized,
            ],
            &FakeWindow::default(),
        );
        assert_eq!(calls, ["set_maximized(true)", "set_maximized(false)"]);
    }

    #[test]
    fn a_set_then_toggle_in_one_batch_toggles_from_the_requested_state() {
        let maximized = FakeWindow {
            maximized: true,
            ..FakeWindow::default()
        };
        let (_, calls) = run(
            &[
                WindowCommand::SetMaximized(false),
                WindowCommand::ToggleMaximized,
            ],
            &maximized,
        );
        assert_eq!(calls, ["set_maximized(false)", "set_maximized(true)"]);
    }

    #[test]
    fn state_commands_map_one_to_one_in_order() {
        let commands = [
            WindowCommand::SetFullscreen(true),
            WindowCommand::Minimize,
            WindowCommand::SetMaximized(true),
            WindowCommand::SetMaximized(false),
            WindowCommand::SetFullscreen(false),
        ];
        let (close, calls) = run(&commands, &FakeWindow::default());
        assert!(!close);
        assert_eq!(
            calls,
            [
                "set_fullscreen(true)",
                "set_minimized(true)",
                "set_maximized(true)",
                "set_maximized(false)",
                "set_fullscreen(false)",
            ]
        );
    }

    #[test]
    fn request_close_is_reported_and_touches_no_window_call() {
        let (close, calls) = run(&[WindowCommand::RequestClose], &FakeWindow::default());
        assert!(close);
        assert!(calls.is_empty());
        let (close, calls) = run(
            &[
                WindowCommand::RequestClose,
                WindowCommand::Minimize,
                WindowCommand::RequestClose,
            ],
            &FakeWindow::default(),
        );
        assert!(close, "several closes still report one flag");
        assert_eq!(calls, ["set_minimized(true)"]);
    }

    #[test]
    fn a_refused_drag_is_swallowed_and_warns_once_per_kind() {
        let _guard = lock_and_drain();
        MOVE_FAILURE_WARNED.store(false, Ordering::Relaxed);
        RESIZE_FAILURE_WARNED.store(false, Ordering::Relaxed);
        let refusing = FakeWindow {
            refuse_drags: true,
            ..FakeWindow::default()
        };
        let commands = [
            WindowCommand::BeginResize(ResizeEdge::North),
            WindowCommand::Minimize,
        ];
        assert!(!execute(&commands, &refusing));
        assert!(RESIZE_FAILURE_WARNED.load(Ordering::Relaxed));
        assert!(
            !MOVE_FAILURE_WARNED.load(Ordering::Relaxed),
            "each command kind has its own guard"
        );
        assert!(!first_failure(&RESIZE_FAILURE_WARNED), "already latched");
        assert!(!execute(&commands, &refusing));
        assert_eq!(
            refusing.calls(),
            [
                "drag_resize_window(North)",
                "set_minimized(true)",
                "drag_resize_window(North)",
                "set_minimized(true)",
            ],
            "the commands after a refused drag still run"
        );
        let guard = AtomicBool::new(false);
        assert!(first_failure(&guard));
        assert!(!first_failure(&guard));
    }

    #[test]
    fn map_edge_covers_all_eight_edges() {
        let pairs = [
            (ResizeEdge::North, ResizeDirection::North),
            (ResizeEdge::NorthEast, ResizeDirection::NorthEast),
            (ResizeEdge::East, ResizeDirection::East),
            (ResizeEdge::SouthEast, ResizeDirection::SouthEast),
            (ResizeEdge::South, ResizeDirection::South),
            (ResizeEdge::SouthWest, ResizeDirection::SouthWest),
            (ResizeEdge::West, ResizeDirection::West),
            (ResizeEdge::NorthWest, ResizeDirection::NorthWest),
        ];
        for (edge, direction) in pairs {
            assert_eq!(map_edge(edge), direction);
        }
    }

    #[test]
    fn window_state_snapshot_copies_the_four_flags() {
        let window = FakeWindow {
            maximized: true,
            fullscreen: false,
            focused: true,
            minimized: false,
            ..FakeWindow::default()
        };
        assert_eq!(
            window_state_snapshot(&window),
            WindowState::new(true, false, true, false)
        );
        let window = FakeWindow {
            fullscreen: true,
            minimized: true,
            ..FakeWindow::default()
        };
        assert_eq!(
            window_state_snapshot(&window),
            WindowState::new(false, true, false, true)
        );
        assert!(window.calls().is_empty(), "a snapshot only reads");
    }

    #[test]
    fn an_empty_queue_drains_to_none_without_a_window_call() {
        let _guard = lock_and_drain();
        let window = FakeWindow::default();
        assert_eq!(drain(&window), None);
        assert!(window.calls().is_empty());

        push_window_command(WindowCommand::BeginMove);
        push_window_command(WindowCommand::RequestClose);
        assert_eq!(drain(&window), Some(true));
        assert_eq!(window.calls(), ["drag_window"]);
        assert_eq!(drain(&window), None, "the queue is left empty");
    }
}
