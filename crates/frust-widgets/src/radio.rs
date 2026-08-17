//! The `Radio` interactive widget: a labelled circular indicator
//! that reports a requested selection but is **not** its own source of truth.
//!
//! [`radio`] produces a [`RadioView`] carrying the current `selected` value
//! and a label; [`RadioView::on_select`] attaches an `on_select(state)`
//! closure fired on release inside its bounds. Unlike [`crate::checkbox`]'s
//! `on_toggle`, `on_select` carries no value argument — a radio's release
//! always means "select me", and the app-level callback is the one that knows
//! *which* radio it is (e.g. `radio(freq == Daily, "Daily")
//! .on_select(|s| s.freq = Freq::Daily)`). The widget never flips its own
//! `selected` field; the app mutates its state in the callback and the next
//! rebuild feeds the confirmed selection back in (the masonry rule:
//! application state is the source of truth).
//!
//! **No group container in v1.** A radio group is app-level: a column of
//! [`radio`] views sharing one piece of app state that records which is
//! selected (see the module doc example above).
//! There is no `RadioGroup` widget here — compare
//! [`crate::material::button_group`], whose `Role::RadioGroup` semantics
//! container is a *different*, already-shipped single-select control.

use std::rc::Rc;

use frust_core::accesskit::{Action, Role};
use frust_core::{
    BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent, LayoutCtx,
    PaintCtx, PaintScene, PointerPhase, SemanticsCtx, View, Widget, any,
};
use frust_scene::arc_path;
use frust_theme::Theme;
use kurbo::{Point, Size};
use peniko::{Brush, Color};

use crate::text;

/// Outer diameter of the radio ring, in logical px (matches
/// [`crate::checkbox`]'s box side so a mixed column of checkboxes/radios
/// lines up).
const DIAMETER: f64 = 20.0;
/// Gap between the ring and its label, in logical px.
const GAP: f64 = 8.0;
/// Stroke width of the outer ring.
const RING_STROKE: f64 = 2.0;
/// Radius of the filled inner dot, painted only when selected.
const DOT_RADIUS: f64 = 5.0;
/// Unselected ring stroke color (unthemed fallback; a theme resolves this
/// from `colors.outline`).
const RING_OFF: Color = Color::from_rgb8(0xE5, 0xE7, 0xEB);
/// Selected ring stroke + inner dot fill color (unthemed fallback; a theme
/// resolves this from `colors.primary`). A filled dot inside a same-colored
/// ring is the correct M3 shape — the ring is stroked over a transparent
/// interior, not filled, so there is no primary-colored surface underneath
/// the dot that would call for an `on_primary`-contrast fill the way
/// [`crate::checkbox`]'s check mark needs one.
const RING_ON: Color = Color::from_rgb8(0x3B, 0x82, 0xF6);

/// The resolved radio ring/dot color for the given `selected` state. Themed:
/// `outline`/`primary`. Unthemed: [`RING_OFF`]/[`RING_ON`] exactly, so a
/// pre-theme app renders unchanged.
fn resolve_color(theme: Option<&Theme>, selected: bool) -> Color {
    match theme {
        Some(theme) => {
            let scheme = theme.scheme();
            if selected {
                scheme.primary
            } else {
                scheme.outline
            }
        }
        None => {
            if selected {
                RING_ON
            } else {
                RING_OFF
            }
        }
    }
}

/// A view-held, typed select callback (erased on build).
type OnSelect<State> = Rc<dyn Fn(&mut State)>;

/// A declarative radio button. See the [module docs](self).
pub struct RadioView<State: 'static> {
    selected: bool,
    label: String,
    on_select: Option<OnSelect<State>>,
}

/// Create a radio reflecting `selected`, labelled `label`. Attach a callback
/// with [`RadioView::on_select`]; a radio with none is inert (still paints
/// and captures the press, but never fires) — the same optional-callback
/// shape a design system's own list-item row uses.
pub fn radio<State: 'static>(selected: bool, label: impl Into<String>) -> RadioView<State> {
    RadioView {
        selected,
        label: label.into(),
        on_select: None,
    }
}

/// PascalCase alias for [`radio`].
#[allow(non_snake_case)]
pub fn Radio<State: 'static>(selected: bool, label: impl Into<String>) -> RadioView<State> {
    radio(selected, label)
}

impl<State: 'static> RadioView<State> {
    /// Fire `on_select(state)` on release inside this radio's bounds. Never
    /// self-mutates `selected` — see the [module docs](self).
    pub fn on_select<F: Fn(&mut State) + 'static>(mut self, on_select: F) -> Self {
        self.on_select = Some(Rc::new(on_select));
        self
    }
}

/// The retained widget for a [`RadioView`].
pub struct RadioWidget {
    selected: bool,
    label: ChildPod,
    /// The label text, retained for the semantics node's accessible name (a
    /// radio is a single a11y node reading its name from here rather than
    /// recursing into the inner `TextWidget`).
    label_text: String,
    /// The pressed *visual* state; follows the cursor in/out while captured.
    pressed: bool,
    /// Armed by a `Down` (alongside `capture_pointer`), cleared on `Up`/`Cancel`.
    /// Gates all `Move`/`Up` handling so a hover `Move` never latches `pressed`
    /// or fires `on_select` without a preceding press.
    captured: bool,
    on_select: Option<crate::authoring::ErasedCallback>,
}

fn inside(pos: Point, size: Size) -> bool {
    pos.x >= 0.0 && pos.y >= 0.0 && pos.x < size.width && pos.y < size.height
}

impl<State: 'static> View<State> for RadioView<State> {
    type Element = RadioWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> RadioWidget {
        let label_view = any::<State, _>(text(self.label.clone()));
        RadioWidget {
            selected: self.selected,
            label: crate::authoring::build_child(&label_view, ctx),
            label_text: self.label.clone(),
            pressed: false,
            captured: false,
            on_select: self
                .on_select
                .as_ref()
                .map(crate::authoring::erase_callback),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut RadioWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        // Closures are not comparable — always reinstall the adapter.
        element.on_select = self
            .on_select
            .as_ref()
            .map(crate::authoring::erase_callback);
        let mut flags = ChangeFlags::NONE;
        if prev.selected != self.selected {
            // The app is the source of truth: adopt the new value on rebuild.
            element.selected = self.selected;
            flags |= ChangeFlags::PAINT;
        }
        if prev.label != self.label {
            element.label_text = self.label.clone();
            let prev_view = any::<State, _>(text(prev.label.clone()));
            let next_view = any::<State, _>(text(self.label.clone()));
            flags |=
                crate::authoring::rebuild_child(&prev_view, &next_view, &mut element.label, ctx);
        }
        flags
    }

    fn teardown(&self, element: &mut RadioWidget, ctx: &mut BuildCtx<'_>) {
        let label_view = any::<State, _>(text(self.label.clone()));
        crate::authoring::teardown_child(&label_view, &mut element.label, ctx);
    }
}

impl Widget for RadioWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let label_max = Size::new((bc.max().width - DIAMETER - GAP).max(0.0), bc.max().height);
        let label_size = self
            .label
            .layout_child(ctx, &BoxConstraints::loose(label_max));
        let height = label_size.height.max(DIAMETER);
        // Vertically centre the label against the ring.
        self.label.set_origin(Point::new(
            DIAMETER + GAP,
            (height - label_size.height) / 2.0,
        ));
        bc.constrain(Size::new(DIAMETER + GAP + label_size.width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let color = resolve_color(Theme::from_paint_ctx(ctx), self.selected);
        let brush = Brush::Solid(color);
        let center = Point::new(
            ctx.origin().x + DIAMETER / 2.0,
            ctx.origin().y + ctx.size().height / 2.0,
        );
        let ring_radius = (DIAMETER - RING_STROKE) / 2.0;
        let ring_path = arc_path(center, ring_radius, 0.0, std::f64::consts::TAU);
        scene.stroke_path(Point::ZERO, &ring_path, RING_STROKE, &brush);
        if self.selected {
            let dot_path = arc_path(center, DOT_RADIUS, 0.0, std::f64::consts::TAU);
            scene.fill_path(Point::ZERO, &dot_path, &brush);
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
                if inside(p.position, ctx.size())
                    && let Some(on_select) = self.on_select.as_mut()
                {
                    // Fire unconditionally: a release always requests
                    // "select me", never self-mutating `selected`.
                    (on_select)(ctx);
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
        // A single RadioButton node carrying its selected state and label; it
        // fires a Click to select.
        ctx.push_node(Role::RadioButton, |node| {
            node.set_label(self.label_text.as_str());
            node.set_selected(self.selected);
            node.add_action(Action::Click);
        });
    }

    crate::authoring::visit_children!(label);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::any::Any;

    #[derive(Default)]
    struct SelectState {
        selects: u32,
    }

    fn widget(selected: bool) -> RadioWidget {
        let view = radio::<SelectState>(selected, "daily").on_select(|s: &mut SelectState| {
            s.selects += 1;
        });
        let mut counter = 0u64;
        View::<SelectState>::build(&view, &mut BuildCtx::new(&mut counter))
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(frust_core::PointerEvent {
            phase,
            position: Point::new(x, y),
            button: frust_core::PointerButton::Primary,
        })
    }

    fn dispatch(w: &mut RadioWidget, state: &mut SelectState, event: &InputEvent) {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(120.0, 24.0));
        w.event(&mut ctx, event);
    }

    #[test]
    fn unselected_fires_on_select_and_does_not_self_mutate() {
        let mut w = widget(false);
        let mut state = SelectState::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 12.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 5.0, 12.0));
        assert_eq!(state.selects, 1);
        assert!(!w.selected, "radio must not mutate its own selected flag");
    }

    #[test]
    fn already_selected_still_fires() {
        let mut w = widget(true);
        let mut state = SelectState::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 12.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 5.0, 12.0));
        assert_eq!(state.selects, 1);
        assert!(w.selected, "still selected until the app rebuilds it");
    }

    #[test]
    fn up_outside_does_not_fire() {
        let mut w = widget(false);
        let mut state = SelectState::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 12.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Move, 500.0, 12.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 500.0, 12.0));
        assert_eq!(state.selects, 0);
    }

    #[test]
    fn hover_move_without_down_is_ignored_noop() {
        let mut w = widget(false);
        let mut state = SelectState::default();
        let state_any: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(120.0, 24.0));
        let result = w.event(&mut ctx, &ev(PointerPhase::Move, 5.0, 12.0));
        assert!(matches!(result, EventResult::Ignored));
        assert!(!w.pressed, "hover must not press");
        assert!(!ctx.needs_redraw(), "hover must not request a redraw");
        assert_eq!(state.selects, 0);
    }

    #[test]
    fn up_without_down_does_not_fire() {
        let mut w = widget(false);
        let mut state = SelectState::default();
        let state_any: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(120.0, 24.0));
        let result = w.event(&mut ctx, &ev(PointerPhase::Up, 5.0, 12.0));
        assert!(matches!(result, EventResult::Ignored));
        assert_eq!(state.selects, 0);
    }

    #[test]
    fn cancel_clears_armed_state() {
        let mut w = widget(false);
        let mut state = SelectState::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 12.0));
        assert!(w.captured);
        dispatch(&mut w, &mut state, &ev(PointerPhase::Cancel, 5.0, 12.0));
        assert!(!w.captured, "Cancel disarms the press");
        // A follow-up hover Up must not fire.
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 5.0, 12.0));
        assert_eq!(state.selects, 0);
    }

    /// Records the ring stroke and dot fill colors.
    #[derive(Default)]
    struct RingRecorder {
        strokes: Vec<Color>,
        fills: Vec<Color>,
    }

    impl PaintScene for RingRecorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn stroke_path(&mut self, _o: Point, _path: &kurbo::BezPath, _width: f64, brush: &Brush) {
            if let Brush::Solid(c) = brush {
                self.strokes.push(*c);
            }
        }
        fn fill_path(&mut self, _o: Point, _path: &kurbo::BezPath, brush: &Brush) {
            if let Brush::Solid(c) = brush {
                self.fills.push(*c);
            }
        }
    }

    fn paint_rec(w: &mut RadioWidget, theme: Option<&Theme>) -> RingRecorder {
        let mut rec = RingRecorder::default();
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
        let rec = paint_rec(&mut off, None);
        assert_eq!(rec.strokes, vec![RING_OFF]);
        assert!(rec.fills.is_empty(), "unselected radio paints no dot");
        let mut on = widget(true);
        let rec = paint_rec(&mut on, None);
        assert_eq!(rec.strokes, vec![RING_ON]);
        assert_eq!(rec.fills, vec![RING_ON]);
    }

    #[test]
    fn themed_paint_resolves_roles() {
        let theme = Theme::neutral();
        let scheme = theme.scheme();
        let mut off = widget(false);
        assert_eq!(
            paint_rec(&mut off, Some(&theme)).strokes,
            vec![scheme.outline],
            "unselected ring uses the outline role"
        );
        let mut on = widget(true);
        let rec = paint_rec(&mut on, Some(&theme));
        assert_eq!(
            rec.strokes,
            vec![scheme.primary],
            "selected ring uses primary"
        );
        assert_eq!(rec.fills, vec![scheme.primary], "dot uses primary");
    }

    #[test]
    fn rebuild_adopts_new_selected_value() {
        let mut counter = 0u64;
        let prev = radio::<SelectState>(false, "daily");
        let mut w = View::<SelectState>::build(&prev, &mut BuildCtx::new(&mut counter));
        assert!(!w.selected);
        let next = radio::<SelectState>(true, "daily");
        let flags =
            View::<SelectState>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));
        assert!(w.selected);
        assert!(flags.needs_paint());
    }

    #[test]
    fn semantics_reports_role_label_selected_and_bounds() {
        fn logic(_s: &mut ()) -> RadioView<()> {
            radio::<()>(true, "daily")
        }
        let mut root: frust_core::RenderRoot<(), RadioView<()>> = frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = frust_text::TextContext::new();
        root.layout_with_text(Size::new(200.0, 200.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::RadioButton)
            .expect("radio contributes a Role::RadioButton node");
        assert_eq!(node.label(), Some("daily"));
        assert_eq!(node.is_selected(), Some(true));
        let bounds = node.bounds().expect("radio node has bounds");
        assert_eq!((bounds.x0, bounds.y0), (0.0, 0.0));
    }
}
