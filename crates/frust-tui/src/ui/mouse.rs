//! Per-frame mouse-region registry with hover, z-indexed hit-testing, and a
//! pointer-aware scroll-region kind — ported in shape from fdemon's proven
//! design (rewritten against ratatui 0.30 `Rect`; no verbatim copy).
//!
//! # Lifecycle
//!
//! 1. Before each draw the loop calls [`MouseRegions::begin_frame`] (clears the
//!    per-frame vecs, preserving capacity).
//! 2. During render, views receive a [`MouseCtx`] and register clickable /
//!    hoverable / scrollable surfaces (`button`/`click`/`hover`/`scroll`). A
//!    **suppressed** ctx ([`MouseCtx::suppressed`]) drops every registration —
//!    the modal base-layer suppression D4 calls for (pass `None`).
//! 3. On a pointer event the loop hit-tests the registry:
//!    [`MouseRegions::hover_at`] (topmost region id under the cursor, drives
//!    hover-change redraws), [`MouseRegions::click_at`] (topmost bound click
//!    message), [`MouseRegions::scroll_at`] (the pane under the cursor, not a
//!    global focus).
//!
//! Higher `z_index` wins on overlap; ties break last-pushed-wins (later render
//! order draws on top).

use ratatui::layout::{Position, Rect};

use crate::engine::{Message, RegionId};

/// A clickable/hoverable region.
#[derive(Debug, Clone)]
struct ClickRegion {
    rect: Rect,
    /// Semantic id used for hover tracking and press/release routing.
    id: Option<RegionId>,
    /// Message emitted on a completed left click, if this region binds one.
    on_left: Option<Message>,
    z_index: u8,
}

/// A pointer-aware scroll region: wheel events over `rect` route to `up`/`down`
/// rather than a global focused surface.
#[derive(Debug, Clone)]
struct ScrollRegion {
    rect: Rect,
    up: Message,
    down: Message,
    z_index: u8,
}

/// A drag region (splitter / scrollbar thumb). Typed stub wired in Phase 3;
/// registered here so the region-kind vocabulary is complete from the start.
#[derive(Debug, Clone)]
struct DragRegion {
    #[allow(dead_code)]
    rect: Rect,
    #[allow(dead_code)]
    id: RegionId,
}

/// The per-frame registry.
#[derive(Debug, Default)]
pub struct MouseRegions {
    clicks: Vec<ClickRegion>,
    scrolls: Vec<ScrollRegion>,
    drags: Vec<DragRegion>,
}

impl MouseRegions {
    /// A pre-sized registry.
    pub fn new() -> Self {
        Self {
            clicks: Vec::with_capacity(32),
            scrolls: Vec::with_capacity(8),
            drags: Vec::with_capacity(4),
        }
    }

    /// Drop the previous frame's regions, preserving capacity.
    pub fn begin_frame(&mut self) {
        self.clicks.clear();
        self.scrolls.clear();
        self.drags.clear();
    }

    /// The id of the topmost region under `(x, y)`, if any — the hover source.
    pub fn hover_at(&self, x: u16, y: u16) -> Option<RegionId> {
        let pos = Position::new(x, y);
        self.clicks
            .iter()
            .enumerate()
            .filter(|(_, r)| r.rect.contains(pos))
            .max_by_key(|(i, r)| (r.z_index, *i))
            .and_then(|(_, r)| r.id)
    }

    /// The click message bound to the topmost region under `(x, y)`, if any.
    pub fn click_at(&self, x: u16, y: u16) -> Option<Message> {
        let pos = Position::new(x, y);
        self.clicks
            .iter()
            .enumerate()
            .filter(|(_, r)| r.rect.contains(pos) && r.on_left.is_some())
            .max_by_key(|(i, r)| (r.z_index, *i))
            .and_then(|(_, r)| r.on_left.clone())
    }

    /// The scroll message for the pane under `(x, y)` for the given direction.
    pub fn scroll_at(&self, x: u16, y: u16, down: bool) -> Option<Message> {
        let pos = Position::new(x, y);
        self.scrolls
            .iter()
            .enumerate()
            .filter(|(_, r)| r.rect.contains(pos))
            .max_by_key(|(i, r)| (r.z_index, *i))
            .map(|(_, r)| if down { r.down.clone() } else { r.up.clone() })
    }

    /// Number of registered click regions (tests).
    pub fn click_len(&self) -> usize {
        self.clicks.len()
    }
}

/// The render-time handle views push regions through.
///
/// Wraps `Option<&mut MouseRegions>`: a **suppressed** ctx (`None`) makes every
/// registration a no-op, the modal base-layer suppression D4 specifies.
pub struct MouseCtx<'a> {
    regions: Option<&'a mut MouseRegions>,
}

impl<'a> MouseCtx<'a> {
    /// A live ctx that records into `regions`.
    pub fn new(regions: &'a mut MouseRegions) -> Self {
        Self {
            regions: Some(regions),
        }
    }

    /// A suppressed ctx: every registration is dropped (base layer under a
    /// modal).
    pub fn suppressed() -> Self {
        Self { regions: None }
    }

    /// Register a hover + press/release target with a semantic id and no bound
    /// click message (the loop drives press/release off the id).
    pub fn button(&mut self, rect: Rect, id: RegionId) {
        if rect.is_empty() {
            return;
        }
        if let Some(r) = self.regions.as_deref_mut() {
            r.clicks.push(ClickRegion {
                rect,
                id: Some(id),
                on_left: None,
                z_index: 0,
            });
        }
    }

    /// Register a click region that emits `msg` on a completed left click.
    pub fn click(&mut self, rect: Rect, id: RegionId, msg: Message) {
        if rect.is_empty() {
            return;
        }
        if let Some(r) = self.regions.as_deref_mut() {
            r.clicks.push(ClickRegion {
                rect,
                id: Some(id),
                on_left: Some(msg),
                z_index: 0,
            });
        }
    }

    /// Register a hover-only region (no click binding).
    pub fn hover(&mut self, rect: Rect, id: RegionId) {
        if rect.is_empty() {
            return;
        }
        if let Some(r) = self.regions.as_deref_mut() {
            r.clicks.push(ClickRegion {
                rect,
                id: Some(id),
                on_left: None,
                z_index: 0,
            });
        }
    }

    /// Register a pointer-aware scroll region.
    pub fn scroll(&mut self, rect: Rect, up: Message, down: Message) {
        if rect.is_empty() {
            return;
        }
        if let Some(r) = self.regions.as_deref_mut() {
            r.scrolls.push(ScrollRegion {
                rect,
                up,
                down,
                z_index: 0,
            });
        }
    }

    /// Register a drag region (Phase 3 stub — splitter / scrollbar thumb).
    pub fn drag(&mut self, rect: Rect, id: RegionId) {
        if rect.is_empty() {
            return;
        }
        if let Some(r) = self.regions.as_deref_mut() {
            r.drags.push(DragRegion { rect, id });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hover_and_click_hit_test() {
        let mut regions = MouseRegions::new();
        {
            let mut ctx = MouseCtx::new(&mut regions);
            ctx.button(Rect::new(2, 3, 10, 3), RegionId::CreateButton);
        }
        assert_eq!(regions.hover_at(4, 4), Some(RegionId::CreateButton));
        assert_eq!(regions.hover_at(0, 0), None);
        // Button regions bind no click message (press/release driven by id).
        assert_eq!(regions.click_at(4, 4), None);
    }

    #[test]
    fn click_region_binds_message() {
        let mut regions = MouseRegions::new();
        {
            let mut ctx = MouseCtx::new(&mut regions);
            ctx.click(
                Rect::new(0, 0, 5, 1),
                RegionId::CreateButton,
                Message::CreateActivate,
            );
        }
        assert_eq!(regions.click_at(1, 0), Some(Message::CreateActivate));
    }

    #[test]
    fn suppressed_ctx_records_nothing() {
        let regions = MouseRegions::new();
        {
            let mut ctx = MouseCtx::suppressed();
            ctx.button(Rect::new(0, 0, 10, 3), RegionId::CreateButton);
        }
        assert_eq!(regions.click_len(), 0);
    }

    #[test]
    fn empty_rect_is_skipped() {
        let mut regions = MouseRegions::new();
        {
            let mut ctx = MouseCtx::new(&mut regions);
            ctx.button(Rect::new(0, 0, 0, 3), RegionId::CreateButton);
        }
        assert_eq!(regions.click_len(), 0);
    }

    #[test]
    fn begin_frame_clears_preserving_capacity() {
        let mut regions = MouseRegions::new();
        {
            let mut ctx = MouseCtx::new(&mut regions);
            ctx.button(Rect::new(0, 0, 4, 1), RegionId::CreateButton);
        }
        assert_eq!(regions.click_len(), 1);
        regions.begin_frame();
        assert_eq!(regions.click_len(), 0);
    }

    #[test]
    fn scroll_region_routes_by_direction() {
        let mut regions = MouseRegions::new();
        {
            let mut ctx = MouseCtx::new(&mut regions);
            ctx.scroll(
                Rect::new(0, 0, 10, 10),
                Message::Tick,
                Message::Resize(1, 1),
            );
        }
        assert_eq!(regions.scroll_at(5, 5, false), Some(Message::Tick));
        assert_eq!(regions.scroll_at(5, 5, true), Some(Message::Resize(1, 1)));
        assert_eq!(regions.scroll_at(50, 50, true), None);
    }
}
