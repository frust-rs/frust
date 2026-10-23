//! The window-control vocabulary an app-drawn title bar uses: [`WindowCommand`], [`ResizeEdge`],
//! the process-global queue [`push_window_command`] fills, and the [`WindowState`] context.
//! The desktop shell drains the queue after each input dispatch and once per frame; other shells
//! never drain it.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// The most commands the queue holds; a push beyond it drops the oldest.
const WINDOW_COMMAND_CAP: usize = 32;

/// The window edge or corner a resize drag grabs; mirrors winit's `ResizeDirection`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ResizeEdge {
    North,
    NorthEast,
    East,
    SouthEast,
    South,
    SouthWest,
    West,
    NorthWest,
}

/// A request to the desktop shell about its window; not `#[non_exhaustive]` by design.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowCommand {
    /// Start the OS move drag; push it from a primary pointer-down (macOS needs that event).
    BeginMove,
    /// Start the OS resize drag. A no-op on macOS, where AppKit keeps its own edge resizing.
    BeginResize(ResizeEdge),
    /// Maximize (`true`) or restore (`false`).
    SetMaximized(bool),
    /// Maximize a restored window, or restore a maximized one.
    ToggleMaximized,
    /// Minimize; there is no un-minimize, which Wayland cannot provide.
    Minimize,
    /// Enter (`true`) or leave (`false`) fullscreen.
    SetFullscreen(bool),
    /// Request a close, through the same path as the OS close button; no bypass variant exists.
    RequestClose,
}

/// The desktop window's live state, provided beside [`crate::WindowMetrics`] and re-provided on
/// change. Absent (`None`) on mobile and web.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct WindowState {
    pub maximized: bool,
    pub fullscreen: bool,
    pub focused: bool,
    pub minimized: bool,
}

impl WindowState {
    /// Construct a [`WindowState`]; needed outside this crate because of `#[non_exhaustive]`.
    pub fn new(maximized: bool, fullscreen: bool, focused: bool, minimized: bool) -> Self {
        Self {
            maximized,
            fullscreen,
            focused,
            minimized,
        }
    }
}

static QUEUE: Mutex<Vec<WindowCommand>> = Mutex::new(Vec::new());

/// Latched after the first overflow warning, so a flood logs one line.
static OVERFLOW_WARNED: AtomicBool = AtomicBool::new(false);

/// Queue a command for the next drain. Past the cap, the oldest non-`RequestClose` command is
/// dropped. The queue serves the one desktop window (no window id; multi-window is out of scope),
/// so push from UI-thread contexts. A push does not wake the event loop: it runs at the end of
/// the current input dispatch or the next frame, so a cross-thread push waits for either.
pub fn push_window_command(command: WindowCommand) {
    let mut queue = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
    if queue.len() >= WINDOW_COMMAND_CAP {
        let victim = queue
            .iter()
            .position(|queued| *queued != WindowCommand::RequestClose)
            .unwrap_or(0);
        queue.remove(victim);
        if !OVERFLOW_WARNED.swap(true, Ordering::Relaxed) {
            log::warn!(
                "frust-core: more than {WINDOW_COMMAND_CAP} window commands are queued and \
                 dropping the oldest; no desktop shell is draining them"
            );
        }
    }
    queue.push(command);
}

/// Drain every pending command, oldest first, leaving the queue empty.
pub fn take_window_commands() -> Vec<WindowCommand> {
    std::mem::take(&mut *QUEUE.lock().unwrap_or_else(|e| e.into_inner()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The queue is process-global, so tests that touch it run one at a time.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn lock_and_drain() -> std::sync::MutexGuard<'static, ()> {
        let guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        take_window_commands();
        guard
    }

    fn label(command: WindowCommand) -> &'static str {
        match command {
            WindowCommand::BeginMove => "move",
            WindowCommand::BeginResize(edge) => match edge {
                ResizeEdge::North => "n",
                ResizeEdge::NorthEast => "ne",
                ResizeEdge::East => "e",
                ResizeEdge::SouthEast => "se",
                ResizeEdge::South => "s",
                ResizeEdge::SouthWest => "sw",
                ResizeEdge::West => "w",
                ResizeEdge::NorthWest => "nw",
            },
            WindowCommand::SetMaximized(_) => "set-max",
            WindowCommand::ToggleMaximized => "toggle-max",
            WindowCommand::Minimize => "minimize",
            WindowCommand::SetFullscreen(_) => "fullscreen",
            WindowCommand::RequestClose => "close",
        }
    }

    #[test]
    fn pushed_commands_come_back_in_fifo_order_once() {
        let _guard = lock_and_drain();
        let pushed = [
            WindowCommand::BeginMove,
            WindowCommand::SetMaximized(true),
            WindowCommand::RequestClose,
        ];
        for command in pushed {
            push_window_command(command);
        }
        assert_eq!(take_window_commands(), pushed.to_vec());
        assert!(take_window_commands().is_empty());
    }

    #[test]
    fn overflow_keeps_the_newest_commands_and_warns_once() {
        let _guard = lock_and_drain();
        OVERFLOW_WARNED.store(false, Ordering::Relaxed);

        push_window_command(WindowCommand::SetMaximized(false));
        for _ in 0..WINDOW_COMMAND_CAP {
            push_window_command(WindowCommand::ToggleMaximized);
        }

        let drained = take_window_commands();
        assert_eq!(drained.len(), WINDOW_COMMAND_CAP);
        assert!(
            drained.iter().all(|c| *c == WindowCommand::ToggleMaximized),
            "the first-pushed command is the one dropped"
        );
        assert!(
            OVERFLOW_WARNED.load(Ordering::Relaxed),
            "the overflow warning latches once the cap is exceeded"
        );
    }

    #[test]
    fn a_request_close_pushed_first_survives_later_overflow() {
        let _guard = lock_and_drain();
        push_window_command(WindowCommand::RequestClose);
        for _ in 0..40 {
            push_window_command(WindowCommand::ToggleMaximized);
        }

        let drained = take_window_commands();
        assert_eq!(drained.len(), WINDOW_COMMAND_CAP);
        assert_eq!(
            drained.first(),
            Some(&WindowCommand::RequestClose),
            "the close is kept while the oldest non-close commands are evicted"
        );
        assert_eq!(
            drained
                .iter()
                .filter(|c| **c == WindowCommand::RequestClose)
                .count(),
            1
        );
    }

    #[test]
    fn window_state_round_trips_its_fields_and_defaults_to_all_false() {
        let state = WindowState::new(true, false, true, false);
        assert!(state.maximized);
        assert!(!state.fullscreen);
        assert!(state.focused);
        assert!(!state.minimized);
        assert_eq!(
            WindowState::default(),
            WindowState::new(false, false, false, false)
        );
    }

    #[test]
    fn every_command_and_edge_is_copy_and_matched_exhaustively() {
        let edges = [
            ResizeEdge::North,
            ResizeEdge::NorthEast,
            ResizeEdge::East,
            ResizeEdge::SouthEast,
            ResizeEdge::South,
            ResizeEdge::SouthWest,
            ResizeEdge::West,
            ResizeEdge::NorthWest,
        ];
        let mut labels = Vec::new();
        for edge in edges {
            let copied = edge;
            assert_eq!(copied, edge);
            labels.push(label(WindowCommand::BeginResize(edge)));
        }
        let commands = [
            WindowCommand::BeginMove,
            WindowCommand::SetMaximized(true),
            WindowCommand::ToggleMaximized,
            WindowCommand::Minimize,
            WindowCommand::SetFullscreen(false),
            WindowCommand::RequestClose,
        ];
        for command in commands {
            let copied = command;
            assert_eq!(copied, command);
            labels.push(label(command));
        }
        assert_eq!(labels.len(), edges.len() + commands.len());
    }
}
