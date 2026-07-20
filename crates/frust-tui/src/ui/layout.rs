//! Workbench layout shell splits: titlebar / body / status bar, and the
//! sidebar / main split inside the body.

use ratatui::layout::{Constraint, Direction, Layout, Rect};

/// Height of the titlebar chrome (rows). A 2-row title + a 1-row underline
/// motif (matches workbook B2).
pub const TITLEBAR_HEIGHT: u16 = 3;

/// Height of the status bar (rows).
pub const STATUS_HEIGHT: u16 = 1;

/// Default sidebar width (columns) before drag-resize — the engine's
/// [`crate::engine::SIDEBAR_DEFAULT_WIDTH`] by re-export (one number, shared
/// across the render/engine layer boundary).
pub const SIDEBAR_WIDTH: u16 = crate::engine::SIDEBAR_DEFAULT_WIDTH;

/// The narrow-terminal responsive breakpoint (T05 / D5): below this width the
/// workbench collapses its inline sidebar out of the layout (see
/// `views::workbench::render`), reachable instead as a toggleable floating
/// overlay (`s` / `AppState::sidebar_overlay_open`). Comfortably above
/// [`crate::ui::views::too_small::MIN_WIDTH`], so there is a real narrow
/// range between "sidebar collapses" and "unusable".
pub const NARROW_WIDTH: u16 = 80;

/// Whether `area` is narrow enough to collapse the sidebar out of the inline
/// layout (see [`NARROW_WIDTH`]).
pub fn is_narrow(area: Rect) -> bool {
    area.width < NARROW_WIDTH
}

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

/// Split a body area into the default-width sidebar and the remaining main area.
pub fn sidebar_main(body: Rect) -> (Rect, Rect) {
    sidebar_main_at(body, SIDEBAR_WIDTH)
}

/// Split a body area into a `width`-column sidebar and the remaining main area
/// (T04 drag-to-resize). `width` is clamped to the sidebar min/max and to
/// `body.width - 1` so the main area never vanishes.
pub fn sidebar_main_at(body: Rect, width: u16) -> (Rect, Rect) {
    let w = crate::engine::clamp_sidebar_width(width).min(body.width.saturating_sub(1));
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(w), Constraint::Min(1)])
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
