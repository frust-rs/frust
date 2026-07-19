//! The `Button` interactive widget (spec §6.4): a labelled, rounded pressable
//! that fires an app-state callback on release *inside* its bounds.
//!
//! [`button`] is the declarative view-fn; it produces a [`ButtonView`] carrying
//! the label and a typed `on_press` closure, which materialises into a retained
//! [`ButtonWidget`]. The masonry-verified interaction is **fire-on-up-inside**:
//! a `Down` inside captures the pointer and paints the pressed state; `Move`
//! only updates the pressed visual (cursor-inside); the callback fires on `Up`
//! *only if the release lands inside*. A `Cancel` (platform gesture steal) just
//! clears the pressed state. See `research/RESEARCH.md`.

use std::rc::Rc;

use frust_core::accesskit::{Action, Role};
use frust_core::{
    BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent, LayoutCtx,
    PaintCtx, PaintScene, PointerPhase, SemanticsCtx, View, Widget, any,
};
use frust_theme::Theme;
use kurbo::{Point, Size};
use peniko::Color;

use crate::text;
use crate::text::ThemeTextColor;

/// Corner radius of the button's rounded-rect background, in logical px (the
/// unthemed fallback; a theme resolves this from `shape.small`).
const RADIUS: f64 = 6.0;
/// Horizontal padding around the label, in logical px. No `Theme` spacing
/// token exists to resolve this from (`frust-theme` publishes a shape
/// scale and a type scale, not a padding/spacing scale) — hoisted here as a
/// named constant per task 6f-10's metric-hardcode-migration pass rather than
/// left as a bare literal, pending a future spacing-token addition.
const PAD_X: f64 = 12.0;
/// Vertical padding around the label, in logical px. See [`PAD_X`]'s doc
/// comment — no suitable `Theme` token exists for this metric either.
const PAD_Y: f64 = 8.0;
/// Resting background fill (unthemed fallback; a theme resolves this from
/// `colors.primary`).
const FILL: Color = Color::from_rgb8(0x3B, 0x82, 0xF6);
/// Pressed (darker) background fill (unthemed fallback; a theme resolves this
/// from a darkened `colors.primary` — see [`pressed_overlay`]).
const FILL_PRESSED: Color = Color::from_rgb8(0x1D, 0x4E, 0xD8);

/// The fixed multiplier applied to `colors.primary`'s RGB to synthesize the
/// pressed fill under a theme (a v1 stand-in reproducing today's press
/// contrast; M3 tonal state-layers land in phase 6c). `0.82` darkens primary by
/// roughly the same amount today's `FILL`→`FILL_PRESSED` step does.
const PRESSED_DARKEN: f32 = 0.82;

/// Darken a color by scaling its RGB components toward black by `factor`,
/// leaving alpha untouched. Used to synthesize the button's pressed fill from a
/// themed `primary` (see [`ButtonWidget::resolve_fills`]).
fn pressed_overlay(color: Color, factor: f32) -> Color {
    let c = color.components;
    Color::new([c[0] * factor, c[1] * factor, c[2] * factor, c[3]])
}

/// A view-held, typed press callback (erased to [`crate::ErasedCallback`] on build).
type OnPress<State> = Rc<dyn Fn(&mut State)>;

/// Build the type-erased label view, tagged with the `OnPrimary` themed color
/// role so the label reads correctly against the `primary`-filled button (an
/// unthemed button keeps its black label). Shared by build/rebuild/teardown so
/// the role stays consistent across the child's whole lifecycle.
fn label_view<State: 'static>(label: String) -> frust_core::AnyView<State> {
    any::<State, _>(text(label).themed_role(ThemeTextColor::OnPrimary))
}

/// A declarative pressable button. See the [module docs](self).
pub struct ButtonView<State: 'static> {
    label: String,
    on_press: OnPress<State>,
}

/// Create a button labelled `label` that runs `on_press` against the app state
/// when released inside its bounds.
pub fn button<State: 'static, F: Fn(&mut State) + 'static>(
    label: impl Into<String>,
    on_press: F,
) -> ButtonView<State> {
    ButtonView {
        label: label.into(),
        on_press: Rc::new(on_press),
    }
}

/// PascalCase alias for [`button`], matching the container view-fn vocabulary.
#[allow(non_snake_case)]
pub fn Button<State: 'static, F: Fn(&mut State) + 'static>(
    label: impl Into<String>,
    on_press: F,
) -> ButtonView<State> {
    button(label, on_press)
}

/// The retained widget for a [`ButtonView`]. The label is a nested
/// [`crate::TextWidget`] owned as a [`ChildPod`].
pub struct ButtonWidget {
    label: ChildPod,
    /// The label text, retained for the semantics node's accessible name (the
    /// label lives inside the `label` pod as a `TextWidget`; a button is a single
    /// a11y node, so it reads its name from here rather than recursing).
    label_text: String,
    /// The pressed *visual* state (background darkens). Follows the cursor
    /// in/out while captured, and is purely cosmetic.
    pressed: bool,
    /// Armed by a `Down` (alongside `capture_pointer`), cleared on `Up`/`Cancel`.
    /// Gates all `Move`/`Up` handling so a hover `Move` (dispatched by the
    /// desktop shell on every cursor motion) never latches `pressed` or fires
    /// the callback without a preceding press.
    captured: bool,
    on_press: crate::ErasedCallback,
}

/// Whether a widget-local `pos` lies within a `size`-sized box anchored at the
/// origin (the button's own bounds).
fn inside(pos: Point, size: Size) -> bool {
    pos.x >= 0.0 && pos.y >= 0.0 && pos.x < size.width && pos.y < size.height
}

impl ButtonWidget {
    /// The `(resting, pressed)` background fills. Themed: `colors.primary` and a
    /// darkened primary ([`pressed_overlay`]). Unthemed: the [`FILL`]/
    /// [`FILL_PRESSED`] constants exactly, so a pre-theme app renders unchanged.
    fn resolve_fills(theme: Option<&Theme>) -> (Color, Color) {
        match theme {
            Some(theme) => {
                let primary = theme.scheme().primary;
                (primary, pressed_overlay(primary, PRESSED_DARKEN))
            }
            None => (FILL, FILL_PRESSED),
        }
    }

    /// The corner radius: themed `shape.small` (8dp — one step up from today's 6px
    /// fallback, the closest M3 token; a visually negligible change), resolved
    /// against the box so it never exceeds a pill. Unthemed: the [`RADIUS`]
    /// constant exactly.
    fn resolve_radius(theme: Option<&Theme>, size: Size) -> f64 {
        match theme {
            Some(theme) => {
                frust_theme::ShapeScale::resolve(theme.shape.small, size.width, size.height)
            }
            None => RADIUS,
        }
    }
}

impl<State: 'static> View<State> for ButtonView<State> {
    type Element = ButtonWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> ButtonWidget {
        let label_view = label_view::<State>(self.label.clone());
        ButtonWidget {
            label: crate::build_child(&label_view, ctx),
            label_text: self.label.clone(),
            pressed: false,
            captured: false,
            on_press: crate::erase_callback(&self.on_press),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ButtonWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        // Closures are not comparable — always reinstall the adapter.
        element.on_press = crate::erase_callback(&self.on_press);
        let mut flags = ChangeFlags::NONE;
        if prev.label != self.label {
            element.label_text = self.label.clone();
            let prev_view = label_view::<State>(prev.label.clone());
            let next_view = label_view::<State>(self.label.clone());
            flags |= crate::rebuild_child(&prev_view, &next_view, &mut element.label, ctx);
        }
        flags
    }

    fn teardown(&self, element: &mut ButtonWidget, ctx: &mut BuildCtx<'_>) {
        let label_view = label_view::<State>(self.label.clone());
        crate::teardown_child(&label_view, &mut element.label, ctx);
    }
}

impl Widget for ButtonWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // Lay the label out inside the padded content box, then grow to wrap it.
        let inset = Size::new(PAD_X * 2.0, PAD_Y * 2.0);
        let inner_max = Size::new(
            (bc.max().width - inset.width).max(0.0),
            (bc.max().height - inset.height).max(0.0),
        );
        let label_size = self
            .label
            .layout_child(ctx, &BoxConstraints::loose(inner_max));
        self.label.set_origin(Point::new(PAD_X, PAD_Y));
        bc.constrain(Size::new(
            label_size.width + inset.width,
            label_size.height + inset.height,
        ))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let (resting, pressed) = Self::resolve_fills(theme);
        let radius = Self::resolve_radius(theme, ctx.size());
        let fill = if self.pressed { pressed } else { resting };
        scene.fill_rounded_rect(ctx.origin(), ctx.size(), radius, fill);
        self.label.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        match p.phase {
            PointerPhase::Down => {
                self.pressed = true;
                self.captured = true;
                ctx.capture_pointer();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                // Only a press we armed on `Down` tracks the cursor; a hover
                // `Move` (no prior press) is not ours.
                if !self.captured {
                    return EventResult::Ignored;
                }
                // Visual only: track whether the cursor is still over the button.
                self.pressed = inside(p.position, ctx.size());
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Up => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                // Fire on up-inside only (masonry semantics).
                if inside(p.position, ctx.size()) {
                    (self.on_press)(ctx);
                }
                self.pressed = false;
                self.captured = false;
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                self.pressed = false;
                self.captured = false;
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // A button is a single a11y node (Role::Button) labelled by its text; it
        // does not expose its inner label as a separate child node. It advertises
        // the Click action it fires on release.
        ctx.push_node(Role::Button, |node| {
            node.set_label(self.label_text.as_str());
            node.add_action(Action::Click);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::any::Any;

    #[derive(Default)]
    struct Counter {
        presses: u32,
    }

    /// Build a button widget over `Counter` state, with a known 100x40 size for
    /// the inside/outside geometry (set directly, avoiding a text-context layout).
    fn widget() -> ButtonWidget {
        let view = button::<Counter, _>("go", |s: &mut Counter| s.presses += 1);
        let mut counter = 0u64;
        View::<Counter>::build(&view, &mut BuildCtx::new(&mut counter))
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(frust_core::PointerEvent {
            phase,
            position: Point::new(x, y),
            button: frust_core::PointerButton::Primary,
        })
    }

    fn dispatch(w: &mut ButtonWidget, state: &mut Counter, event: &InputEvent) {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(100.0, 40.0));
        w.event(&mut ctx, event);
    }

    #[test]
    fn down_then_up_inside_fires_once() {
        let mut w = widget();
        let mut state = Counter::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 10.0, 10.0));
        assert!(w.pressed);
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 12.0, 12.0));
        assert_eq!(state.presses, 1);
        assert!(!w.pressed);
    }

    #[test]
    fn down_inside_move_out_up_outside_does_not_fire() {
        let mut w = widget();
        let mut state = Counter::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 10.0, 10.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Move, 200.0, 10.0));
        assert!(!w.pressed, "moving out clears the pressed visual");
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 200.0, 10.0));
        assert_eq!(state.presses, 0, "up outside must not fire");
    }

    #[test]
    fn cancel_clears_pressed_without_firing() {
        let mut w = widget();
        let mut state = Counter::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 10.0, 10.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Cancel, 10.0, 10.0));
        assert!(!w.pressed);
        assert_eq!(state.presses, 0);
    }

    #[test]
    fn hover_move_without_down_is_ignored_noop() {
        let mut w = widget();
        let mut state = Counter::default();
        // A cursor drifting over the button with no prior press must not latch
        // pressed, fire, or request a redraw.
        let state_any: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(100.0, 40.0));
        let result = w.event(&mut ctx, &ev(PointerPhase::Move, 20.0, 20.0));
        assert!(matches!(result, EventResult::Ignored));
        assert!(!w.pressed, "hover must not press");
        assert!(!ctx.needs_redraw(), "hover must not request a redraw");
        assert_eq!(state.presses, 0);
    }

    #[test]
    fn up_without_down_does_not_fire() {
        let mut w = widget();
        let mut state = Counter::default();
        let state_any: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(100.0, 40.0));
        let result = w.event(&mut ctx, &ev(PointerPhase::Up, 20.0, 20.0));
        assert!(matches!(result, EventResult::Ignored));
        assert_eq!(state.presses, 0, "an unarmed Up must never fire");
    }

    #[test]
    fn cancel_clears_armed_state() {
        let mut w = widget();
        let mut state = Counter::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 10.0, 10.0));
        assert!(w.captured);
        dispatch(&mut w, &mut state, &ev(PointerPhase::Cancel, 10.0, 10.0));
        assert!(!w.captured, "Cancel disarms the press");
        // A subsequent hover Move must not re-press or fire.
        let state_any: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(100.0, 40.0));
        let result = w.event(&mut ctx, &ev(PointerPhase::Move, 12.0, 12.0));
        assert!(matches!(result, EventResult::Ignored));
        assert!(!w.pressed);
    }

    /// A recording scene that captures each rounded rect's `(radius, color)`.
    #[derive(Default)]
    struct RRectRecorder {
        rrects: Vec<(f64, Color)>,
    }

    impl PaintScene for RRectRecorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, _o: Point, _s: Size, radius: f64, color: Color) {
            self.rrects.push((radius, color));
        }
    }

    fn paint_bg(w: &mut ButtonWidget, theme: Option<&frust_theme::Theme>) -> (f64, Color) {
        let mut rec = RRectRecorder::default();
        let mut ctx = match theme {
            Some(t) => PaintCtx::new(Point::ZERO, Size::new(100.0, 40.0)).with_theme(t),
            None => PaintCtx::new(Point::ZERO, Size::new(100.0, 40.0)),
        };
        w.paint(&mut ctx, &mut rec);
        *rec.rrects.first().expect("button paints its background")
    }

    #[test]
    fn unthemed_paint_uses_fallback_constants() {
        // Parity: no theme → exactly today's fill and radius.
        let mut w = widget();
        assert_eq!(paint_bg(&mut w, None), (RADIUS, FILL));
        // Pressed uses the darker constant, unchanged.
        w.pressed = true;
        assert_eq!(paint_bg(&mut w, None), (RADIUS, FILL_PRESSED));
    }

    #[test]
    fn themed_paint_resolves_primary_and_shape_small() {
        let theme = frust_theme::Theme::m3_baseline();
        let mut w = widget();
        let (radius, color) = paint_bg(&mut w, Some(&theme));
        assert_eq!(color, theme.scheme().primary, "resting fill is primary");
        assert_eq!(radius, theme.shape.small, "radius is shape.small (8dp)");
        // Pressed fill is a darkened primary (not the unthemed constant).
        w.pressed = true;
        let (_, pressed) = paint_bg(&mut w, Some(&theme));
        assert_eq!(
            pressed,
            pressed_overlay(theme.scheme().primary, PRESSED_DARKEN)
        );
    }

    #[test]
    fn move_back_inside_then_up_fires() {
        // out then back in: up-inside fires (masonry re-hover behaviour).
        let mut w = widget();
        let mut state = Counter::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 10.0, 10.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Move, 200.0, 10.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Move, 20.0, 10.0));
        assert!(w.pressed);
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 20.0, 10.0));
        assert_eq!(state.presses, 1);
    }
}
