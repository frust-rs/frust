//! The `Slider` interactive widget (spec §6.4): a horizontal track with a thumb
//! that reports a `0.0..=1.0` value as it is dragged.
//!
//! [`slider`] produces a [`SliderView`] carrying the current `value` and an
//! `on_change` closure. A `Down` on the track captures the pointer and reports
//! the value at that x-position; each captured `Move` reports the updated value
//! (continuous drag); `Up`/`Cancel` release. Like [`crate::Checkbox`] it is not
//! its own source of truth — it fires `on_change` and the next rebuild feeds the
//! new value back in. Horizontal only for v1.

use std::rc::Rc;

use forgekit_core::{
    BoxConstraints, BuildCtx, ChangeFlags, EventCtx, EventResult, InputEvent, LayoutCtx, PaintCtx,
    PaintScene, PointerPhase, View, Widget,
};
use forgekit_theme::Theme;
use kurbo::{Point, Size};
use peniko::Color;

/// Slider control height, in logical px.
const HEIGHT: f64 = 24.0;
/// Default track width when the incoming constraints are unbounded.
const DEFAULT_WIDTH: f64 = 200.0;
/// Track (groove) thickness, in logical px.
const TRACK_H: f64 = 4.0;
/// Thumb diameter, in logical px.
const THUMB: f64 = 18.0;
/// Unfilled (inactive) track color (unthemed fallback; a theme resolves this
/// from `colors.surface_container_highest`, per the M3 slider spec).
const TRACK: Color = Color::from_rgb8(0xD1, 0xD5, 0xDB);
/// Filled (active) track color (unthemed fallback; a theme resolves this from
/// `colors.primary`).
const FILL: Color = Color::from_rgb8(0x3B, 0x82, 0xF6);
/// Thumb color (unthemed fallback; a theme resolves this from `colors.primary`).
const THUMB_FILL: Color = Color::from_rgb8(0x1D, 0x4E, 0xD8);

/// The resolved slider paint colors: `(inactive_track, active_track, thumb)`.
/// Themed per the Material 3 slider spec: the inactive track is
/// `surface_container_highest`, the active track and thumb are `primary`.
/// Unthemed: the [`TRACK`]/[`FILL`]/[`THUMB_FILL`] constants exactly, so a
/// pre-theme app renders unchanged.
fn resolve_colors(theme: Option<&Theme>) -> (Color, Color, Color) {
    match theme {
        Some(theme) => {
            let scheme = theme.scheme();
            (
                scheme.surface_container_highest,
                scheme.primary,
                scheme.primary,
            )
        }
        None => (TRACK, FILL, THUMB_FILL),
    }
}

/// A view-held, typed change callback (erased on build).
type OnChange<State> = Rc<dyn Fn(&mut State, f64)>;

/// A declarative slider. See the [module docs](self).
pub struct SliderView<State: 'static> {
    value: f64,
    on_change: OnChange<State>,
}

/// Create a slider at `value` (clamped to `0.0..=1.0`) that fires
/// `on_change(state, new_value)` as it is dragged.
pub fn slider<State: 'static, F: Fn(&mut State, f64) + 'static>(
    value: f64,
    on_change: F,
) -> SliderView<State> {
    SliderView {
        value: value.clamp(0.0, 1.0),
        on_change: Rc::new(on_change),
    }
}

/// PascalCase alias for [`slider`].
#[allow(non_snake_case)]
pub fn Slider<State: 'static, F: Fn(&mut State, f64) + 'static>(
    value: f64,
    on_change: F,
) -> SliderView<State> {
    slider(value, on_change)
}

/// The retained widget for a [`SliderView`].
pub struct SliderWidget {
    value: f64,
    /// Armed by a `Down` (alongside `capture_pointer`), cleared on `Up`/`Cancel`.
    /// Gates `Move` so a hover `Move` (no prior press) never fires `on_change`;
    /// a `Down` still jumps to and reports the tapped value regardless.
    captured: bool,
    on_change: crate::ErasedArgCallback<f64>,
}

/// Map a widget-local x (in `0..=width`) to a `0.0..=1.0` value, clamped.
fn value_from_x(x: f64, width: f64) -> f64 {
    if width <= 0.0 {
        0.0
    } else {
        (x / width).clamp(0.0, 1.0)
    }
}

impl<State: 'static> View<State> for SliderView<State> {
    type Element = SliderWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> SliderWidget {
        SliderWidget {
            value: self.value,
            captured: false,
            on_change: crate::erase_callback_arg(&self.on_change),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SliderWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_change = crate::erase_callback_arg(&self.on_change);
        if prev.value != self.value {
            element.value = self.value;
            ChangeFlags::PAINT
        } else {
            ChangeFlags::NONE
        }
    }
}

impl Widget for SliderWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            DEFAULT_WIDTH
        };
        bc.constrain(Size::new(width, HEIGHT))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let (track, fill, thumb_fill) = resolve_colors(Theme::from_paint_ctx(ctx));
        let o = ctx.origin();
        let w = ctx.size().width;
        let mid_y = o.y + ctx.size().height / 2.0;
        // Groove.
        scene.fill_rounded_rect(
            Point::new(o.x, mid_y - TRACK_H / 2.0),
            Size::new(w, TRACK_H),
            TRACK_H / 2.0,
            track,
        );
        // Filled portion up to the thumb.
        let thumb_x = o.x + self.value.clamp(0.0, 1.0) * w;
        scene.fill_rounded_rect(
            Point::new(o.x, mid_y - TRACK_H / 2.0),
            Size::new((thumb_x - o.x).max(0.0), TRACK_H),
            TRACK_H / 2.0,
            fill,
        );
        // Thumb (rounded-rect stand-in for a circle in v1).
        scene.fill_rounded_rect(
            Point::new(thumb_x - THUMB / 2.0, mid_y - THUMB / 2.0),
            Size::new(THUMB, THUMB),
            THUMB / 2.0,
            thumb_fill,
        );
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        match p.phase {
            PointerPhase::Down => {
                self.captured = true;
                ctx.capture_pointer();
                let v = value_from_x(p.position.x, ctx.size().width);
                (self.on_change)(ctx, v);
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                // A hover `Move` (no prior press) never reports a value.
                if !self.captured {
                    return EventResult::Ignored;
                }
                let v = value_from_x(p.position.x, ctx.size().width);
                (self.on_change)(ctx, v);
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Up | PointerPhase::Cancel => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                self.captured = false;
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::any::Any;

    #[derive(Default)]
    struct Val {
        value: f64,
        changes: u32,
    }

    fn widget(value: f64) -> SliderWidget {
        let view = slider::<Val, _>(value, |s: &mut Val, v: f64| {
            s.value = v;
            s.changes += 1;
        });
        let mut counter = 0u64;
        View::<Val>::build(&view, &mut BuildCtx::new(&mut counter))
    }

    fn ev(phase: PointerPhase, x: f64) -> InputEvent {
        InputEvent::Pointer(forgekit_core::PointerEvent {
            phase,
            position: Point::new(x, 12.0),
            button: forgekit_core::PointerButton::Primary,
        })
    }

    fn dispatch(w: &mut SliderWidget, state: &mut Val, event: &InputEvent) {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(200.0, HEIGHT));
        w.event(&mut ctx, event);
    }

    #[test]
    fn value_math_maps_position_to_fraction() {
        assert_eq!(value_from_x(0.0, 200.0), 0.0);
        assert_eq!(value_from_x(100.0, 200.0), 0.5);
        assert_eq!(value_from_x(200.0, 200.0), 1.0);
        // Clamps outside the track.
        assert_eq!(value_from_x(-40.0, 200.0), 0.0);
        assert_eq!(value_from_x(9000.0, 200.0), 1.0);
        // Degenerate zero-width track.
        assert_eq!(value_from_x(10.0, 0.0), 0.0);
    }

    #[test]
    fn down_reports_value_at_position() {
        let mut w = widget(0.0);
        let mut state = Val::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 50.0));
        assert_eq!(state.value, 0.25);
        assert_eq!(state.changes, 1);
    }

    #[test]
    fn drag_reports_continuously_and_clamps() {
        let mut w = widget(0.0);
        let mut state = Val::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 100.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Move, 300.0)); // past the end
        assert_eq!(state.value, 1.0);
        dispatch(&mut w, &mut state, &ev(PointerPhase::Move, -20.0)); // before the start
        assert_eq!(state.value, 0.0);
        assert_eq!(state.changes, 3);
        // Up does not report a value.
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 0.0));
        assert_eq!(state.changes, 3);
    }

    #[test]
    fn hover_move_without_down_is_ignored_noop() {
        let mut w = widget(0.3);
        let mut state = Val::default();
        let state_any: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(200.0, HEIGHT));
        let result = w.event(&mut ctx, &ev(PointerPhase::Move, 100.0));
        assert!(matches!(result, EventResult::Ignored));
        assert!(!ctx.needs_redraw(), "hover must not request a redraw");
        assert_eq!(state.changes, 0, "hover must not report a value");
    }

    #[test]
    fn cancel_clears_captured_state() {
        let mut w = widget(0.0);
        let mut state = Val::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 100.0));
        assert!(w.captured);
        dispatch(&mut w, &mut state, &ev(PointerPhase::Cancel, 100.0));
        assert!(!w.captured, "Cancel disarms the drag");
        // A follow-up hover Move must not report.
        let before = state.changes;
        dispatch(&mut w, &mut state, &ev(PointerPhase::Move, 40.0));
        assert_eq!(state.changes, before, "hover after Cancel does not report");
    }

    /// Records rounded-rect fill colors in paint order (track, fill, thumb).
    #[derive(Default)]
    struct TrackRecorder {
        rrects: Vec<Color>,
    }

    impl PaintScene for TrackRecorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, _o: Point, _s: Size, _r: f64, color: Color) {
            self.rrects.push(color);
        }
    }

    fn paint_colors(value: f64, theme: Option<&Theme>) -> Vec<Color> {
        let mut w = widget(value);
        let mut rec = TrackRecorder::default();
        let mut ctx = match theme {
            Some(t) => PaintCtx::new(Point::ZERO, Size::new(200.0, HEIGHT)).with_theme(t),
            None => PaintCtx::new(Point::ZERO, Size::new(200.0, HEIGHT)),
        };
        w.paint(&mut ctx, &mut rec);
        rec.rrects
    }

    #[test]
    fn unthemed_paint_uses_fallback_constants() {
        assert_eq!(paint_colors(0.5, None), vec![TRACK, FILL, THUMB_FILL]);
    }

    #[test]
    fn themed_paint_resolves_m3_slider_roles() {
        let theme = Theme::m3_baseline();
        let scheme = theme.scheme();
        assert_eq!(
            paint_colors(0.5, Some(&theme)),
            vec![
                scheme.surface_container_highest,
                scheme.primary,
                scheme.primary
            ],
        );
    }

    #[test]
    fn rebuild_adopts_new_value_without_self_mutation() {
        let mut counter = 0u64;
        let prev = slider::<Val, _>(0.2, |_s, _v| {});
        let mut w = View::<Val>::build(&prev, &mut BuildCtx::new(&mut counter));
        assert_eq!(w.value, 0.2);
        let next = slider::<Val, _>(0.8, |_s, _v| {});
        View::<Val>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));
        assert_eq!(w.value, 0.8);
    }
}
