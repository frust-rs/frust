//! The `Checkbox` interactive widget: a labelled box that reports a
//! requested toggle but is **not** its own source of truth.
//!
//! [`checkbox`] produces a [`CheckboxView`] carrying the current `checked` value,
//! a label, and an `on_toggle` closure. On release inside its bounds it fires
//! `on_toggle(state, !checked)` — it does **not** flip its own `checked` field.
//! The app mutates its state in the callback and the next rebuild feeds the new
//! value back in (the masonry rule: application state is the source of truth).

use std::rc::Rc;

use frust_core::accesskit::{Action, Role, Toggled};
use frust_core::{
    BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent, LayoutCtx,
    PaintCtx, PaintScene, PointerPhase, SemanticsCtx, View, Widget, any,
};
use frust_theme::Theme;
use kurbo::{Point, Size};
use peniko::Color;

use crate::text;

/// Side length of the check box, in logical px.
const BOX: f64 = 20.0;
/// Gap between the box and its label, in logical px.
const GAP: f64 = 8.0;
/// Corner radius of the box.
const RADIUS: f64 = 4.0;
/// Stroke width of the check mark.
const CHECK_WIDTH: f64 = 2.5;
/// Unchecked box fill (unthemed fallback; a theme resolves this from
/// `colors.outline`).
const FILL_OFF: Color = Color::from_rgb8(0xE5, 0xE7, 0xEB);
/// Checked box fill (unthemed fallback; a theme resolves this from
/// `colors.primary`).
const FILL_ON: Color = Color::from_rgb8(0x3B, 0x82, 0xF6);
/// Check-mark stroke color (unthemed fallback; a theme resolves this from
/// `colors.on_primary`).
const CHECK: Color = Color::from_rgb8(0xFF, 0xFF, 0xFF);

/// The resolved checkbox paint colors: `(off_fill, on_fill, check)`. Themed:
/// `outline`/`primary`/`on_primary`. Unthemed: the [`FILL_OFF`]/[`FILL_ON`]/
/// [`CHECK`] constants exactly, so a pre-theme app renders unchanged.
fn resolve_colors(theme: Option<&Theme>) -> (Color, Color, Color) {
    match theme {
        Some(theme) => {
            let scheme = theme.scheme();
            (scheme.outline, scheme.primary, scheme.on_primary)
        }
        None => (FILL_OFF, FILL_ON, CHECK),
    }
}

/// A view-held, typed toggle callback (erased on build).
type OnToggle<State> = Rc<dyn Fn(&mut State, bool)>;

/// A declarative checkbox. See the [module docs](self).
pub struct CheckboxView<State: 'static> {
    checked: bool,
    label: String,
    on_toggle: OnToggle<State>,
}

/// Create a checkbox reflecting `checked`, labelled `label`, that fires
/// `on_toggle(state, !checked)` on release inside its bounds.
pub fn checkbox<State: 'static, F: Fn(&mut State, bool) + 'static>(
    checked: bool,
    label: impl Into<String>,
    on_toggle: F,
) -> CheckboxView<State> {
    CheckboxView {
        checked,
        label: label.into(),
        on_toggle: Rc::new(on_toggle),
    }
}

/// PascalCase alias for [`checkbox`].
#[allow(non_snake_case)]
pub fn Checkbox<State: 'static, F: Fn(&mut State, bool) + 'static>(
    checked: bool,
    label: impl Into<String>,
    on_toggle: F,
) -> CheckboxView<State> {
    checkbox(checked, label, on_toggle)
}

/// The retained widget for a [`CheckboxView`].
pub struct CheckboxWidget {
    checked: bool,
    label: ChildPod,
    /// The label text, retained for the semantics node's accessible name (a
    /// checkbox is a single a11y node reading its name from here rather than
    /// recursing into the inner `TextWidget`).
    label_text: String,
    /// The pressed *visual* state; follows the cursor in/out while captured.
    pressed: bool,
    /// Armed by a `Down` (alongside `capture_pointer`), cleared on `Up`/`Cancel`.
    /// Gates all `Move`/`Up` handling so a hover `Move` never latches `pressed`
    /// or fires `on_toggle` without a preceding press.
    captured: bool,
    on_toggle: crate::ErasedArgCallback<bool>,
}

fn inside(pos: Point, size: Size) -> bool {
    pos.x >= 0.0 && pos.y >= 0.0 && pos.x < size.width && pos.y < size.height
}

impl<State: 'static> View<State> for CheckboxView<State> {
    type Element = CheckboxWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> CheckboxWidget {
        let label_view = any::<State, _>(text(self.label.clone()));
        CheckboxWidget {
            checked: self.checked,
            label: crate::build_child(&label_view, ctx),
            label_text: self.label.clone(),
            pressed: false,
            captured: false,
            on_toggle: crate::erase_callback_arg(&self.on_toggle),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut CheckboxWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_toggle = crate::erase_callback_arg(&self.on_toggle);
        let mut flags = ChangeFlags::NONE;
        if prev.checked != self.checked {
            // The app is the source of truth: adopt the new value on rebuild.
            element.checked = self.checked;
            flags |= ChangeFlags::PAINT;
        }
        if prev.label != self.label {
            element.label_text = self.label.clone();
            let prev_view = any::<State, _>(text(prev.label.clone()));
            let next_view = any::<State, _>(text(self.label.clone()));
            flags |= crate::rebuild_child(&prev_view, &next_view, &mut element.label, ctx);
        }
        flags
    }

    fn teardown(&self, element: &mut CheckboxWidget, ctx: &mut BuildCtx<'_>) {
        let label_view = any::<State, _>(text(self.label.clone()));
        crate::teardown_child(&label_view, &mut element.label, ctx);
    }
}

impl Widget for CheckboxWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let label_max = Size::new((bc.max().width - BOX - GAP).max(0.0), bc.max().height);
        let label_size = self
            .label
            .layout_child(ctx, &BoxConstraints::loose(label_max));
        let height = label_size.height.max(BOX);
        // Vertically centre the label against the box.
        self.label
            .set_origin(Point::new(BOX + GAP, (height - label_size.height) / 2.0));
        bc.constrain(Size::new(BOX + GAP + label_size.width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let (off_fill, on_fill, check) = resolve_colors(Theme::from_paint_ctx(ctx));
        let box_y = ctx.origin().y + (ctx.size().height - BOX) / 2.0;
        let box_origin = Point::new(ctx.origin().x, box_y);
        let fill = if self.checked { on_fill } else { off_fill };
        scene.fill_rounded_rect(box_origin, Size::new(BOX, BOX), RADIUS, fill);
        if self.checked {
            // A two-segment check mark within the box.
            let p0 = Point::new(box_origin.x + BOX * 0.24, box_origin.y + BOX * 0.52);
            let p1 = Point::new(box_origin.x + BOX * 0.42, box_origin.y + BOX * 0.70);
            let p2 = Point::new(box_origin.x + BOX * 0.76, box_origin.y + BOX * 0.30);
            scene.stroke_line(p0, p1, CHECK_WIDTH, check);
            scene.stroke_line(p1, p2, CHECK_WIDTH, check);
        }
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
                self.pressed = inside(p.position, ctx.size());
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Up => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                if inside(p.position, ctx.size()) {
                    // Report the *requested* value; never self-toggle.
                    let requested = !self.checked;
                    (self.on_toggle)(ctx, requested);
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
        // A single CheckBox node carrying its toggle state and label; it fires a
        // Click to toggle.
        ctx.push_node(Role::CheckBox, |node| {
            node.set_label(self.label_text.as_str());
            node.set_toggled(Toggled::from(self.checked));
            node.add_action(Action::Click);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::any::Any;

    #[derive(Default)]
    struct ToggleState {
        last: Option<bool>,
        toggles: u32,
    }

    fn widget(checked: bool) -> CheckboxWidget {
        let view = checkbox::<ToggleState, _>(checked, "on", |s: &mut ToggleState, v: bool| {
            s.last = Some(v);
            s.toggles += 1;
        });
        let mut counter = 0u64;
        View::<ToggleState>::build(&view, &mut BuildCtx::new(&mut counter))
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(frust_core::PointerEvent {
            phase,
            position: Point::new(x, y),
            button: frust_core::PointerButton::Primary,
        })
    }

    fn dispatch(w: &mut CheckboxWidget, state: &mut ToggleState, event: &InputEvent) {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(120.0, 24.0));
        w.event(&mut ctx, event);
    }

    #[test]
    fn unchecked_fires_true_and_does_not_self_toggle() {
        let mut w = widget(false);
        let mut state = ToggleState::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 12.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 5.0, 12.0));
        assert_eq!(state.last, Some(true));
        assert_eq!(state.toggles, 1);
        assert!(!w.checked, "checkbox must not mutate its own checked flag");
    }

    #[test]
    fn checked_fires_false() {
        let mut w = widget(true);
        let mut state = ToggleState::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 12.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 5.0, 12.0));
        assert_eq!(state.last, Some(false));
        assert!(w.checked, "still checked until the app rebuilds it");
    }

    #[test]
    fn up_outside_does_not_fire() {
        let mut w = widget(false);
        let mut state = ToggleState::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 12.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Move, 500.0, 12.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 500.0, 12.0));
        assert_eq!(state.toggles, 0);
    }

    #[test]
    fn hover_move_without_down_is_ignored_noop() {
        let mut w = widget(false);
        let mut state = ToggleState::default();
        let state_any: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(120.0, 24.0));
        let result = w.event(&mut ctx, &ev(PointerPhase::Move, 5.0, 12.0));
        assert!(matches!(result, EventResult::Ignored));
        assert!(!w.pressed, "hover must not press");
        assert!(!ctx.needs_redraw(), "hover must not request a redraw");
        assert_eq!(state.toggles, 0);
    }

    #[test]
    fn up_without_down_does_not_fire() {
        let mut w = widget(false);
        let mut state = ToggleState::default();
        let state_any: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(120.0, 24.0));
        let result = w.event(&mut ctx, &ev(PointerPhase::Up, 5.0, 12.0));
        assert!(matches!(result, EventResult::Ignored));
        assert_eq!(state.toggles, 0);
    }

    #[test]
    fn cancel_clears_armed_state() {
        let mut w = widget(false);
        let mut state = ToggleState::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 12.0));
        assert!(w.captured);
        dispatch(&mut w, &mut state, &ev(PointerPhase::Cancel, 5.0, 12.0));
        assert!(!w.captured, "Cancel disarms the press");
        // A follow-up hover Up must not fire.
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 5.0, 12.0));
        assert_eq!(state.toggles, 0);
    }

    /// Records the box fill (rounded rect) and check-stroke colors.
    #[derive(Default)]
    struct BoxRecorder {
        rrects: Vec<Color>,
        strokes: Vec<Color>,
    }

    impl PaintScene for BoxRecorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, _o: Point, _s: Size, _r: f64, color: Color) {
            self.rrects.push(color);
        }
        fn stroke_line(&mut self, _a: Point, _b: Point, _w: f64, color: Color) {
            self.strokes.push(color);
        }
    }

    fn paint_rec(w: &mut CheckboxWidget, theme: Option<&Theme>) -> BoxRecorder {
        let mut rec = BoxRecorder::default();
        let mut ctx = match theme {
            Some(t) => PaintCtx::new(Point::ZERO, Size::new(120.0, 24.0)).with_theme(t),
            None => PaintCtx::new(Point::ZERO, Size::new(120.0, 24.0)),
        };
        w.paint(&mut ctx, &mut rec);
        rec
    }

    #[test]
    fn unthemed_paint_uses_fallback_constants() {
        let mut off = widget(false);
        assert_eq!(paint_rec(&mut off, None).rrects, vec![FILL_OFF]);
        let mut on = widget(true);
        let rec = paint_rec(&mut on, None);
        assert_eq!(rec.rrects, vec![FILL_ON]);
        assert_eq!(rec.strokes, vec![CHECK, CHECK]);
    }

    #[test]
    fn themed_paint_resolves_roles() {
        let theme = Theme::m3_baseline();
        let scheme = theme.scheme();
        let mut off = widget(false);
        assert_eq!(
            paint_rec(&mut off, Some(&theme)).rrects,
            vec![scheme.outline],
            "unchecked box uses the outline role"
        );
        let mut on = widget(true);
        let rec = paint_rec(&mut on, Some(&theme));
        assert_eq!(rec.rrects, vec![scheme.primary], "checked box uses primary");
        assert_eq!(
            rec.strokes,
            vec![scheme.on_primary, scheme.on_primary],
            "check mark uses on_primary"
        );
    }

    #[test]
    fn rebuild_adopts_new_checked_value() {
        let mut counter = 0u64;
        let prev = checkbox::<ToggleState, _>(false, "on", |_s, _v| {});
        let mut w = View::<ToggleState>::build(&prev, &mut BuildCtx::new(&mut counter));
        assert!(!w.checked);
        let next = checkbox::<ToggleState, _>(true, "on", |_s, _v| {});
        let flags =
            View::<ToggleState>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));
        assert!(w.checked);
        assert!(flags.needs_paint());
    }
}
