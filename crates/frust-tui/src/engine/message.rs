//! TEA messages and the semantic region ids they are routed through.
//!
//! Every message is a *pure* input to [`super::update`]: the terminal event
//! loop (`crate::runner`) translates raw crossterm events + mouse-region
//! hit-tests into `Message`s, and the engine's single mutation point applies
//! them. Background tasks (Phase 2 session supervisors) will feed the same
//! channel — see [`super::Engine`].

/// A semantic id for a per-frame mouse region.
///
/// Ids are stable identities the render pass tags its clickable/hoverable
/// surfaces with (see `crate::ui::mouse`); the event loop compares the
/// hovered id against `AppState::hover` to decide whether a hover change is
/// worth a redraw, and drives press/release off the id under the cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RegionId {
    /// The single large "Create a new Frust project" button on the welcome
    /// screen (D6b). The wizard it opens is Phase 2 — the skeleton emits a
    /// toast.
    CreateButton,
}

/// A TEA message: the only way `AppState` ever changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    /// Quit requested (`q` / `Ctrl+Q`). Sets `should_quit`; the loop exits.
    Quit,
    /// A tick from the frame interval. The skeleton has no animation, so this
    /// is a no-op (dirty-frame skip keeps it from forcing a draw).
    Tick,
    /// The terminal was resized; forces a redraw at the new size.
    Resize(u16, u16),
    /// The hovered region changed (or cleared). Deduped by `update` so an
    /// unchanged hover costs no redraw.
    HoverChanged(Option<RegionId>),
    /// A primary-button press landed on the Create button (shows pressed
    /// chrome).
    CreatePressed,
    /// A primary-button release landed on the Create button, or `Enter`/`c`
    /// activated it — opens the create flow (Phase 2; toast for now).
    CreateActivate,
    /// A press that started on the Create button was released elsewhere —
    /// clears the pressed chrome without activating.
    CreateCancel,
}
