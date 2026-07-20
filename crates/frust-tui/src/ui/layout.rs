//! Workbench layout shell splits: titlebar / body / status bar, and the
//! sidebar / main split inside the body.

use ratatui::layout::{Constraint, Direction, Layout, Rect};

/// Height of the titlebar chrome (rows). A 2-row title + a 1-row underline
/// motif (matches workbook B2).
pub const TITLEBAR_HEIGHT: u16 = 3;

/// Height of the status bar (rows).
pub const STATUS_HEIGHT: u16 = 1;

/// Default sidebar width (columns) before drag-resize (Phase 3).
pub const SIDEBAR_WIDTH: u16 = 26;

/// The three horizontal bands of the shell.
#[derive(Debug, Clone, Copy)]
pub struct Shell {
    /// Top chrome (project switcher, version, toolchain chip, action buttons).
    pub titlebar: Rect,
    /// The middle working area (sidebar + main).
    pub body: Rect,
    /// Bottom status bar (keyhints, toasts, mouse indicator).
    pub status: Rect,
}

impl Shell {
    /// Split `area` into titlebar / body / status.
    pub fn split(area: Rect) -> Self {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(TITLEBAR_HEIGHT),
                Constraint::Min(1),
                Constraint::Length(STATUS_HEIGHT),
            ])
            .split(area);
        Self {
            titlebar: rows[0],
            body: rows[1],
            status: rows[2],
        }
    }
}

/// Split a body area into a fixed-width sidebar and the remaining main area.
pub fn sidebar_main(body: Rect) -> (Rect, Rect) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(SIDEBAR_WIDTH), Constraint::Min(1)])
        .split(body);
    (cols[0], cols[1])
}

/// Center a `width` x `height` box inside `area` (clamped to `area`).
pub fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    Rect::new(x, y, w, h)
}
