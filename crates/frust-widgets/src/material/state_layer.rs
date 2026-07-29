//! The shared M3 interaction-state overlay: a fill painted over a widget's
//! shape, in the widget's own content color, at an opacity selected by its
//! current hover/focus/pressed/dragged interaction state.
//!
//! **Source:** androidx Compose Material3 `StateTokens` (`v0_210`, retrieval
//! 2026-07-17) — hover 8%, **focus 10%, pressed 10%**, dragged 16%. This
//! supersedes the older material-web `v0.192` table (hover 8%, focus/pressed
//! 12%, dragged 16%); `v0_210` is the table this helper ships.
//!
//! [`StateLayer`] carries no theme dependency and no interaction-detection
//! logic of its own — a widget's own `event` handler calls the `set_*`
//! setters to record state transitions (each returns whether the flag
//! actually changed, so a caller can gate a redraw request on it), and its
//! `paint` calls [`StateLayer::paint`] once per frame with the already
//! theme-resolved content color (the 3-tier explicit/theme/fallback
//! precedence stays the caller's responsibility — see
//! `docs/CODE_STANDARDS.md`'s Theming conventions).
//!
//! When more than one state is active at once (e.g. a focused *and* pressed
//! widget), the overlay opacity is the **maximum** of the active states'
//! opacities — M3 does not stack multiple overlays additively, it shows the
//! strongest one.
//!
//! # Live vs. aspirational states
//!
//! Of the four interaction states this helper models, **only `pressed` is
//! wired by any shipping widget today** — the max-of-active-states rule above
//! describes the full M3 design, but three of its four inputs have no live
//! signal source yet:
//!
//! - **`hovered` — unwireable today.** `frust_core::event`'s `PointerPhase` (re-exported as `frust_core::PointerPhase`)
//!   has only Down/Move/Up/Cancel — no
//!   Enter/Leave/hover phase — so no widget can detect hover without future
//!   input-pipeline work adding one.
//! - **`focused` — the focus-routing prerequisite landed** (`material::dialog`/
//!   `sheet`, `cupertino::alert_dialog`/`action_sheet` all now call
//!   `EventCtx::request_focus` on a `Down` and dismiss on a focus-routed
//!   `Key(Escape)`), but **still unwired here**: none of those
//!   four modals paint their own `StateLayer`-shaped actionable surface — each
//!   is a plain scrim + panel modal barrier (fills/hairlines), not an M3
//!   interactive surface with a content color a state layer would tint. There
//!   was deliberately nothing to wire `set_focused` into without inventing
//!   chrome no design calls for. A future widget that both participates in
//!   focus routing *and* paints its own `StateLayer` (e.g. a focusable list
//!   item, chip, or button) is what will make this state live.
//! - **`dragged` — unwired.** No catalog widget reports a drag into
//!   [`StateLayer::set_dragged`] yet.
//!
//! The `set_hovered`/`set_focused`/`set_dragged` setters remain the stable API
//! for when those upstream signals exist; they are deliberately kept, not
//! dead-stripped, so a widget can adopt each state the moment its source lands
//! without re-plumbing this helper.

use frust_core::{PaintCtx, PaintScene};
use kurbo::Rect;
use peniko::Color;

/// Hover-state overlay opacity (source: androidx Compose Material3
/// `StateTokens` v0_210, retrieved 2026-07-17).
pub const HOVER_OPACITY: f32 = 0.08;
/// Focus-state overlay opacity (source: androidx Compose Material3
/// `StateTokens` v0_210, retrieved 2026-07-17 — supersedes material-web
/// v0.192's 12%, see R18).
pub const FOCUS_OPACITY: f32 = 0.10;
/// Pressed-state overlay opacity (source: androidx Compose Material3
/// `StateTokens` v0_210, retrieved 2026-07-17 — supersedes material-web
/// v0.192's 12%, see R18).
pub const PRESSED_OPACITY: f32 = 0.10;
/// Dragged-state overlay opacity (source: androidx Compose Material3
/// `StateTokens` v0_210, retrieved 2026-07-17).
pub const DRAGGED_OPACITY: f32 = 0.16;

/// Tracks a widget's hover/focus/pressed/dragged interaction state and paints
/// the M3 state-layer overlay for it. See the [module docs](self).
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct StateLayer {
    hovered: bool,
    focused: bool,
    pressed: bool,
    dragged: bool,
}

impl StateLayer {
    /// A fresh state layer with every interaction flag clear.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record the widget's hover state. Returns whether the flag changed.
    pub fn set_hovered(&mut self, hovered: bool) -> bool {
        let changed = self.hovered != hovered;
        self.hovered = hovered;
        changed
    }

    /// Record the widget's focus state. Returns whether the flag changed.
    pub fn set_focused(&mut self, focused: bool) -> bool {
        let changed = self.focused != focused;
        self.focused = focused;
        changed
    }

    /// Record the widget's pressed state. Returns whether the flag changed.
    pub fn set_pressed(&mut self, pressed: bool) -> bool {
        let changed = self.pressed != pressed;
        self.pressed = pressed;
        changed
    }

    /// Record the widget's dragged state. Returns whether the flag changed.
    pub fn set_dragged(&mut self, dragged: bool) -> bool {
        let changed = self.dragged != dragged;
        self.dragged = dragged;
        changed
    }

    /// Whether any interaction state is currently active (i.e. whether
    /// [`StateLayer::paint`] will paint anything).
    pub fn is_active(&self) -> bool {
        self.hovered || self.focused || self.pressed || self.dragged
    }

    /// The overlay opacity for the current state: the maximum of every active
    /// state's opacity (see the [module docs](self)), or `0.0` if none are
    /// active.
    pub fn opacity(&self) -> f32 {
        let mut opacity: f32 = 0.0;
        if self.hovered {
            opacity = opacity.max(HOVER_OPACITY);
        }
        if self.focused {
            opacity = opacity.max(FOCUS_OPACITY);
        }
        if self.pressed {
            opacity = opacity.max(PRESSED_OPACITY);
        }
        if self.dragged {
            opacity = opacity.max(DRAGGED_OPACITY);
        }
        opacity
    }

    /// Paint the overlay: a filled rounded rect covering `shape_rect` with
    /// `radius`, filled with `content_color` at the current interaction
    /// opacity (replacing, not multiplying, `content_color`'s own alpha — the
    /// M3 spec expresses state layers as "content color at N% opacity"). A
    /// no-op when no interaction state is active.
    pub fn paint(
        &self,
        _ctx: &mut PaintCtx<'_>,
        scene: &mut dyn PaintScene,
        shape_rect: Rect,
        radius: f64,
        content_color: Color,
    ) {
        let opacity = self.opacity();
        if opacity <= 0.0 {
            return;
        }
        scene.fill_rounded_rect(
            shape_rect.origin(),
            shape_rect.size(),
            radius,
            with_alpha(content_color, opacity),
        );
    }
}

/// Return `color` with its alpha channel replaced by `alpha`.
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_core::PaintCtx;
    use kurbo::{Point, Size};

    #[test]
    fn no_active_state_has_zero_opacity() {
        let layer = StateLayer::new();
        assert_eq!(layer.opacity(), 0.0);
        assert!(!layer.is_active());
    }

    #[test]
    fn each_state_selects_its_own_opacity() {
        let mut layer = StateLayer::new();
        layer.set_hovered(true);
        assert_eq!(layer.opacity(), HOVER_OPACITY);

        let mut layer = StateLayer::new();
        layer.set_focused(true);
        assert_eq!(layer.opacity(), FOCUS_OPACITY);

        let mut layer = StateLayer::new();
        layer.set_pressed(true);
        assert_eq!(layer.opacity(), PRESSED_OPACITY);

        let mut layer = StateLayer::new();
        layer.set_dragged(true);
        assert_eq!(layer.opacity(), DRAGGED_OPACITY);
    }

    #[test]
    fn combined_states_use_the_maximum_opacity() {
        let mut layer = StateLayer::new();
        layer.set_hovered(true);
        layer.set_pressed(true);
        // pressed (0.10) > hover (0.08)
        assert_eq!(layer.opacity(), PRESSED_OPACITY);

        layer.set_dragged(true);
        // dragged (0.16) is the strongest of the three
        assert_eq!(layer.opacity(), DRAGGED_OPACITY);
    }

    #[test]
    fn setters_report_whether_the_flag_changed() {
        let mut layer = StateLayer::new();
        assert!(layer.set_hovered(true), "false -> true is a change");
        assert!(!layer.set_hovered(true), "true -> true is not a change");
        assert!(layer.set_hovered(false), "true -> false is a change");
    }

    /// A recording scene that captures each rounded rect's `(origin, size,
    /// radius, color)`, mirroring `button.rs`'s `RRectRecorder` test pattern.
    #[derive(Default)]
    struct RRectRecorder {
        rrects: Vec<(Point, Size, f64, Color)>,
    }

    impl PaintScene for RRectRecorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
    }

    #[test]
    fn inactive_layer_paints_nothing() {
        let layer = StateLayer::new();
        let mut rec = RRectRecorder::default();
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(40.0, 40.0));
        let shape = Rect::from_origin_size(Point::new(2.0, 2.0), Size::new(36.0, 36.0));
        layer.paint(&mut ctx, &mut rec, shape, 8.0, Color::from_rgb8(0, 0, 0));
        assert!(rec.rrects.is_empty());
    }

    #[test]
    fn active_layer_paints_content_color_at_state_opacity() {
        let mut layer = StateLayer::new();
        layer.set_hovered(true);
        let mut rec = RRectRecorder::default();
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(40.0, 40.0));
        let shape = Rect::from_origin_size(Point::new(2.0, 2.0), Size::new(36.0, 36.0));
        let content_color = Color::from_rgb8(0x10, 0x20, 0x30);
        layer.paint(&mut ctx, &mut rec, shape, 8.0, content_color);

        let (origin, size, radius, color) = rec.rrects.first().copied().expect("paints one rect");
        assert_eq!(origin, shape.origin());
        assert_eq!(size, shape.size());
        assert_eq!(radius, 8.0);
        assert_eq!(color, with_alpha(content_color, HOVER_OPACITY));
    }
}
