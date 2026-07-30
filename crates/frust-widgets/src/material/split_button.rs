//! The Material 3 Expressive **split button**: a leading action button
//! joined to a trailing menu button by a hairline divider.
//!
//! [`split_button`] takes the leading label, an `on_press` (leading action)
//! callback, an `on_open` (trailing menu) callback, and the controlled `open`
//! flag. The two halves fire **distinct** callbacks: a tap on the leading half
//! runs `on_press`, a tap on the trailing half runs `on_open`. The menu popup
//! itself is out of scope — `on_open` is where an app hosts one (the dialog /
//! bottom-sheet machinery can present it; the catalog wires a demo).
//!
//! # Trailing shape morph (chevron rotation)
//!
//! The trailing button carries a chevron that **rotates** to reflect the
//! `open` state: down (▾) when closed, springing to up (▴) when open, animated
//! by [`CHEVRON_SPRING`] — the exact `MotionScheme::m3_expressive().default_spatial`
//! preset (stiffness 700, ζ 0.9). `split_button` is a *controlled* component:
//! the chevron follows the `open` prop the app feeds back (typically toggled
//! inside `on_open`), not a self-owned toggle, matching the framework's
//! controlled-component convention (see `docs/CODE_STANDARDS.md`). The spring
//! is a named constant, not a paint-time theme read, because the animation is
//! (re)triggered from the rebuild pass, which threads no theme;
//! `chevron_spring_matches_motion_scheme` keeps it equal to the token.
//!
//! # Connected geometry
//!
//! The two halves abut and read as one shape: the leading half's left corners
//! and the trailing half's right corners use the group shape token
//! ([`ShapeScale::large`](frust_theme::ShapeScale::large), 16dp fallback); the two adjacent inner corners use a
//! small radius ([`ShapeScale::extra_small`](frust_theme::ShapeScale::extra_small), 4dp fallback). A vertical hairline
//! (`outline_variant`) marks the split. Per-corner rounding is built with
//! `kurbo` [`RoundedRect`]/[`RoundedRectRadii`] and filled via
//! [`PaintScene::fill_path`], the same technique [`super::button_group`] and
//! [`super::sheet`] use.

use std::rc::Rc;
use std::time::Duration;

use frust_core::accesskit::{Action, Role};
use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, PointerPhase, SemanticsCtx, SpringDesc, View, Widget,
};
use frust_theme::Theme;
use kurbo::{BezPath, Point, Rect, RoundedRect, RoundedRectRadii, Shape, Size};
use peniko::{Brush, Color};

use crate::authoring::ThemeTextColor;
use crate::text;

/// Horizontal padding around the leading label, in logical px.
const PAD_X: f64 = 20.0;
/// Vertical padding around the leading label, in logical px.
const PAD_Y: f64 = 10.0;
/// Minimum overall height, in logical px (M3 button height floor / touch
/// target).
const MIN_HEIGHT: f64 = 40.0;
/// Fixed width of the trailing (chevron) half, in logical px.
const TRAILING_WIDTH: f64 = 44.0;
/// Tolerance for flattening `kurbo` rounded-rect paths (matches
/// [`super::sheet`]'s `PATH_TOLERANCE`).
const PATH_TOLERANCE: f64 = 0.1;

/// Unthemed-fallback outer corner radius (a theme resolves [`ShapeScale::large`](frust_theme::ShapeScale::large)).
const OUTER_RADIUS: f64 = 16.0;
/// Unthemed-fallback inner (adjacent) corner radius (a theme resolves
/// [`ShapeScale::extra_small`](frust_theme::ShapeScale::extra_small)).
const INNER_RADIUS: f64 = 4.0;

/// Unthemed-fallback container fill (a theme resolves `colors.secondary_container`).
const CONTAINER_FILL: Color = Color::from_rgb8(0xE8, 0xDE, 0xF8);
/// Unthemed-fallback content color — the label and chevron (a theme resolves
/// `colors.on_secondary_container`... exposed here via `on_surface`, see the
/// module docs' label note).
const CONTENT: Color = Color::from_rgb8(0x1D, 0x1B, 0x20);
/// Unthemed-fallback hairline divider color (a theme resolves
/// `colors.outline_variant`).
const DIVIDER: Color = Color::from_rgb8(0xCA, 0xC4, 0xD0);

/// Hairline divider width, in logical px.
const DIVIDER_WIDTH: f64 = 1.0;
/// Chevron half-width (arm horizontal reach from center), in logical px.
const CHEVRON_HALF_WIDTH: f64 = 5.0;
/// Chevron half-height (vertical reach from center), in logical px.
const CHEVRON_HALF_HEIGHT: f64 = 3.0;
/// Chevron stroke width, in logical px.
const CHEVRON_STROKE: f64 = 2.0;

/// The chevron-rotation spring: the exact
/// `MotionScheme::m3_expressive().default_spatial` preset (stiffness 700, ζ 0.9,
/// mass 1). See the [module docs](self) for why it is a constant.
const CHEVRON_SPRING: SpringDesc = SpringDesc {
    mass: 1.0,
    stiffness: 700.0,
    damping_ratio: 0.9,
};

/// Nominal period seeding the chevron [`AnimationController`]'s clock; the
/// motion is spring-driven ([`CHEVRON_SPRING`]) via `fling`, so this backs the
/// controller's construction only.
const CHEVRON_ANIM_PERIOD: Duration = Duration::from_millis(300);

/// Launch velocity (value-units/sec) for the chevron open/close `fling`.
const FLING_VELOCITY: f64 = 3.0;

/// A view-held, typed callback (erased on build).
type Callback<State> = Rc<dyn Fn(&mut State)>;

/// Build the leading label view, themed [`ThemeTextColor::OnSurface`]. Shared by
/// build/rebuild/teardown.
fn label_view<State: 'static>(label: String) -> AnyView<State> {
    frust_core::any::<State, _>(text::text(label).themed_role(ThemeTextColor::OnSurface))
}

/// Which half of the split button an interaction targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Half {
    Leading,
    Trailing,
}

/// A declarative split button. See the [module docs](self).
pub struct SplitButtonView<State: 'static> {
    label: String,
    /// The controlled expanded state driving the chevron direction.
    open: bool,
    on_press: Callback<State>,
    on_open: Callback<State>,
}

/// Create a split button labelled `label`, with a leading `on_press` action and
/// a trailing `on_open` menu callback. `open` reflects whether the menu the app
/// hosts (in `on_open`) is currently showing — it drives the chevron direction;
/// pass `false` for a plain action+menu split button that never rotates.
pub fn split_button<State, P, O>(
    label: impl Into<String>,
    open: bool,
    on_press: P,
    on_open: O,
) -> SplitButtonView<State>
where
    State: 'static,
    P: Fn(&mut State) + 'static,
    O: Fn(&mut State) + 'static,
{
    SplitButtonView {
        label: label.into(),
        open,
        on_press: Rc::new(on_press),
        on_open: Rc::new(on_open),
    }
}

/// PascalCase alias for [`split_button`], matching the container view-fn
/// vocabulary.
#[allow(non_snake_case)]
pub fn SplitButton<State, P, O>(
    label: impl Into<String>,
    open: bool,
    on_press: P,
    on_open: O,
) -> SplitButtonView<State>
where
    State: 'static,
    P: Fn(&mut State) + 'static,
    O: Fn(&mut State) + 'static,
{
    split_button(label, open, on_press, on_open)
}

/// The retained widget for a [`SplitButtonView`].
pub struct SplitButtonWidget {
    /// The leading label [`TextWidget`](crate::TextWidget) pod.
    label: ChildPod,
    /// Retained leading label text, for the semantics node's accessible name.
    label_text: String,
    /// The controlled expanded state (chevron direction).
    open: bool,
    /// The leading/trailing half rects in local space, from [`Widget::layout`].
    leading_rect: Rect,
    trailing_rect: Rect,
    /// The armed half of an in-flight press (`None` when idle).
    armed: Option<Half>,
    /// Whether the armed pointer is currently inside the armed half.
    pressed_inside: bool,
    /// Chevron rotation spring (0 = closed/▾, 1 = open/▴).
    chevron_anim: frust_core::AnimationController,
    on_press: crate::authoring::ErasedCallback,
    on_open: crate::authoring::ErasedCallback,
}

/// The resolved `(container, content, divider)` colors. Themed:
/// `secondary_container`/`on_secondary_container`/`outline_variant`. Unthemed:
/// the module fallback constants.
fn resolve_colors(theme: Option<&Theme>) -> (Color, Color, Color) {
    match theme {
        Some(theme) => {
            let s = theme.scheme();
            (
                s.secondary_container,
                s.on_secondary_container,
                s.outline_variant,
            )
        }
        None => (CONTAINER_FILL, CONTENT, DIVIDER),
    }
}

/// The `(outer, inner)` corner radii. Themed: `shape.large`/`shape.extra_small`.
fn resolve_radii(theme: Option<&Theme>) -> (f64, f64) {
    match theme {
        Some(theme) => (theme.shape.large, theme.shape.extra_small),
        None => (OUTER_RADIUS, INNER_RADIUS),
    }
}

/// The chevron's two-segment path, centered at `center` (local), rotated by
/// `angle` radians (0 = down ▾). Built as a `kurbo` [`BezPath`] of the two arms
/// so it can be stroked in one call.
fn chevron_path(center: Point, angle: f64) -> BezPath {
    let (sin, cos) = angle.sin_cos();
    // Rotate a local point about the origin, then translate to `center`.
    let rot =
        |x: f64, y: f64| Point::new(center.x + x * cos - y * sin, center.y + x * sin + y * cos);
    // Down chevron: left-arm top, tip at bottom, right-arm top.
    let left = rot(-CHEVRON_HALF_WIDTH, -CHEVRON_HALF_HEIGHT);
    let tip = rot(0.0, CHEVRON_HALF_HEIGHT);
    let right = rot(CHEVRON_HALF_WIDTH, -CHEVRON_HALF_HEIGHT);
    let mut path = BezPath::new();
    path.move_to(left);
    path.line_to(tip);
    path.line_to(right);
    path
}

impl SplitButtonWidget {
    /// Which half (if any) contains local point `pos`.
    fn half_at(&self, pos: Point) -> Option<Half> {
        if self.leading_rect.contains(pos) {
            Some(Half::Leading)
        } else if self.trailing_rect.contains(pos) {
            Some(Half::Trailing)
        } else {
            None
        }
    }

    /// (Re)launch the chevron spring toward the current `open` target.
    fn drive_chevron(&mut self) {
        let velocity = if self.open {
            FLING_VELOCITY
        } else {
            -FLING_VELOCITY
        };
        self.chevron_anim.fling(velocity, CHEVRON_SPRING);
    }
}

impl<State: 'static> View<State> for SplitButtonView<State> {
    type Element = SplitButtonWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> SplitButtonWidget {
        let mut chevron_anim = frust_core::AnimationController::new(CHEVRON_ANIM_PERIOD);
        // Seed the resting value so a button built already-open shows ▴ without
        // needing a frame to animate into it.
        if self.open {
            chevron_anim.fling(FLING_VELOCITY, CHEVRON_SPRING);
        }
        SplitButtonWidget {
            label: crate::authoring::build_child(&label_view::<State>(self.label.clone()), ctx),
            label_text: self.label.clone(),
            open: self.open,
            leading_rect: Rect::ZERO,
            trailing_rect: Rect::ZERO,
            armed: None,
            pressed_inside: false,
            chevron_anim,
            on_press: crate::authoring::erase_callback(&self.on_press),
            on_open: crate::authoring::erase_callback(&self.on_open),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SplitButtonWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_press = crate::authoring::erase_callback(&self.on_press);
        element.on_open = crate::authoring::erase_callback(&self.on_open);
        let mut flags = ChangeFlags::NONE;

        if prev.open != self.open {
            element.open = self.open;
            element.drive_chevron();
            flags |= ChangeFlags::PAINT;
        }

        if prev.label != self.label {
            element.label_text = self.label.clone();
            let prev_view = label_view::<State>(prev.label.clone());
            let next_view = label_view::<State>(self.label.clone());
            flags |=
                crate::authoring::rebuild_child(&prev_view, &next_view, &mut element.label, ctx);
        }

        flags
    }

    fn teardown(&self, element: &mut SplitButtonWidget, ctx: &mut BuildCtx<'_>) {
        crate::authoring::teardown_child(
            &label_view::<State>(self.label.clone()),
            &mut element.label,
            ctx,
        );
    }
}

impl Widget for SplitButtonWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let inner_max = Size::new((bc.max().width - TRAILING_WIDTH).max(0.0), bc.max().height);
        let label_size = self
            .label
            .layout_child(ctx, &BoxConstraints::loose(inner_max));
        let height = (label_size.height + PAD_Y * 2.0).max(MIN_HEIGHT);
        let leading_w = label_size.width + PAD_X * 2.0;

        self.label
            .set_origin(Point::new(PAD_X, (height - label_size.height) / 2.0));
        self.leading_rect = Rect::new(0.0, 0.0, leading_w, height);
        self.trailing_rect = Rect::new(leading_w, 0.0, leading_w + TRAILING_WIDTH, height);

        bc.constrain(Size::new(leading_w + TRAILING_WIDTH, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let (container, content, divider) = resolve_colors(theme);
        let (outer, inner) = resolve_radii(theme);
        let origin = ctx.origin();

        // Leading half: outer on the left, inner on the right.
        let leading_radii = RoundedRectRadii::new(outer, inner, inner, outer);
        let leading_path =
            RoundedRect::from_rect(self.leading_rect, leading_radii).to_path(PATH_TOLERANCE);
        scene.fill_path(origin, &leading_path, &Brush::Solid(container));

        // Trailing half: inner on the left, outer on the right.
        let trailing_radii = RoundedRectRadii::new(inner, outer, outer, inner);
        let trailing_path =
            RoundedRect::from_rect(self.trailing_rect, trailing_radii).to_path(PATH_TOLERANCE);
        scene.fill_path(origin, &trailing_path, &Brush::Solid(container));

        // Hairline divider at the split.
        let split_x = self.leading_rect.x1;
        scene.stroke_line(
            Point::new(origin.x + split_x, origin.y + PAD_Y),
            Point::new(origin.x + split_x, origin.y + self.leading_rect.y1 - PAD_Y),
            DIVIDER_WIDTH,
            divider,
        );

        // Chevron: advance the rotation spring and draw the two arms.
        let animating = self.chevron_anim.advance(ctx.frame_time());
        let angle = self.chevron_anim.value() * std::f64::consts::PI;
        let center = Point::new(self.trailing_rect.center().x, self.trailing_rect.center().y);
        let chevron = chevron_path(center, angle);
        scene.stroke_path(origin, &chevron, CHEVRON_STROKE, &Brush::Solid(content));

        self.label.paint_child(ctx, scene);

        if animating {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        match p.phase {
            PointerPhase::Down => {
                let Some(half) = self.half_at(p.position) else {
                    return EventResult::Ignored;
                };
                self.armed = Some(half);
                self.pressed_inside = true;
                ctx.capture_pointer();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                let Some(half) = self.armed else {
                    return EventResult::Ignored;
                };
                let rect = match half {
                    Half::Leading => self.leading_rect,
                    Half::Trailing => self.trailing_rect,
                };
                self.pressed_inside = rect.contains(p.position);
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Up => {
                let Some(half) = self.armed else {
                    return EventResult::Ignored;
                };
                if self.pressed_inside {
                    match half {
                        Half::Leading => (self.on_press)(ctx),
                        Half::Trailing => (self.on_open)(ctx),
                    }
                }
                self.armed = None;
                self.pressed_inside = false;
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if self.armed.is_none() {
                    return EventResult::Ignored;
                }
                self.armed = None;
                self.pressed_inside = false;
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // Two buttons under a group: the leading action and the trailing menu
        // (which advertises its expanded state). Bounds are the whole widget's
        // (v1 — see `super::button_group`'s semantics note).
        ctx.push_container(
            Role::Group,
            |_| {},
            |ctx| {
                ctx.push_node(Role::Button, |node| {
                    node.set_label(self.label_text.as_str());
                    node.add_action(Action::Click);
                });
                ctx.push_node(Role::Button, |node| {
                    node.set_label("Show menu");
                    node.set_expanded(self.open);
                    node.add_action(Action::Click);
                    node.add_action(Action::Expand);
                });
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_core::{PointerButton, PointerEvent};
    use frust_theme::MotionScheme;
    use std::any::Any;

    #[derive(Default)]
    struct Log {
        presses: u32,
        opens: u32,
    }

    fn build_split(open: bool) -> SplitButtonWidget {
        let view = split_button::<Log, _, _>(
            "Save",
            open,
            |s: &mut Log| s.presses += 1,
            |s: &mut Log| s.opens += 1,
        );
        let mut counter = 0u64;
        View::<Log>::build(&view, &mut BuildCtx::new(&mut counter))
    }

    /// Give the widget known half rects without a text-context layout: leading
    /// [0,100)x[0,40), trailing [100,144)x[0,40).
    fn with_rects(w: &mut SplitButtonWidget) {
        w.leading_rect = Rect::new(0.0, 0.0, 100.0, 40.0);
        w.trailing_rect = Rect::new(100.0, 0.0, 144.0, 40.0);
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    fn dispatch(w: &mut SplitButtonWidget, state: &mut Log, event: &InputEvent) {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(144.0, 40.0));
        w.event(&mut ctx, event);
    }

    #[test]
    fn chevron_spring_matches_motion_scheme() {
        let default_spatial = MotionScheme::m3_expressive().default_spatial;
        assert_eq!(CHEVRON_SPRING.stiffness, default_spatial.stiffness);
        assert_eq!(CHEVRON_SPRING.damping_ratio, default_spatial.damping_ratio);
        assert_eq!(CHEVRON_SPRING.mass, 1.0);
    }

    #[test]
    fn leading_tap_fires_on_press_only() {
        let mut w = build_split(false);
        with_rects(&mut w);
        let mut state = Log::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 50.0, 20.0));
        assert_eq!(w.armed, Some(Half::Leading));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 50.0, 20.0));
        assert_eq!(state.presses, 1);
        assert_eq!(state.opens, 0, "leading tap must not open the menu");
    }

    #[test]
    fn trailing_tap_fires_on_open_only() {
        let mut w = build_split(false);
        with_rects(&mut w);
        let mut state = Log::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 120.0, 20.0));
        assert_eq!(w.armed, Some(Half::Trailing));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 120.0, 20.0));
        assert_eq!(state.opens, 1);
        assert_eq!(state.presses, 0, "trailing tap must not fire the action");
    }

    #[test]
    fn release_outside_the_armed_half_fires_nothing() {
        let mut w = build_split(false);
        with_rects(&mut w);
        let mut state = Log::default();
        // Press leading, drag into trailing, release there.
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 50.0, 20.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Move, 120.0, 20.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 120.0, 20.0));
        assert_eq!(state.presses, 0);
        assert_eq!(state.opens, 0);
    }

    #[test]
    fn down_outside_both_halves_is_ignored() {
        let mut w = build_split(false);
        with_rects(&mut w);
        let mut state = Log::default();
        let state_any: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(144.0, 40.0));
        let result = w.event(&mut ctx, &ev(PointerPhase::Down, 300.0, 20.0));
        assert!(matches!(result, EventResult::Ignored));
        assert!(w.armed.is_none());
    }

    #[test]
    fn cancel_disarms_without_firing() {
        let mut w = build_split(false);
        with_rects(&mut w);
        let mut state = Log::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 120.0, 20.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Cancel, 120.0, 20.0));
        assert!(w.armed.is_none());
        assert_eq!(state.opens, 0);
    }

    #[test]
    fn open_prop_change_relaunches_the_chevron_spring() {
        let mut w = build_split(false);
        assert!(!w.open);
        let prev = split_button::<Log, _, _>("Save", false, |_s: &mut Log| {}, |_s: &mut Log| {});
        let next = split_button::<Log, _, _>("Save", true, |_s: &mut Log| {}, |_s: &mut Log| {});
        let mut counter = 0u64;
        View::<Log>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));
        assert!(w.open, "open state moved on rebuild (controlled)");
        assert!(
            w.chevron_anim.is_animating(),
            "the chevron spring relaunched toward the open target"
        );
    }

    #[test]
    fn chevron_path_rotates_with_the_angle() {
        // At angle 0 the tip is below center; at angle π it is above center.
        let center = Point::new(10.0, 10.0);
        let down = chevron_path(center, 0.0);
        let up = chevron_path(center, std::f64::consts::PI);
        // The middle element (LineTo to the tip) carries the tip point.
        let tip_of = |p: &BezPath| match p.elements()[1] {
            kurbo::PathEl::LineTo(pt) => pt,
            other => panic!("expected LineTo tip, got {other:?}"),
        };
        assert!(tip_of(&down).y > center.y, "closed chevron points down");
        assert!(tip_of(&up).y < center.y, "open chevron points up");
    }

    #[derive(Default)]
    struct PathRecorder {
        fills: usize,
        strokes: usize,
        lines: usize,
    }

    impl PaintScene for PathRecorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_path(&mut self, _o: Point, _p: &BezPath, _b: &Brush) {
            self.fills += 1;
        }
        fn stroke_path(&mut self, _o: Point, _p: &BezPath, _w: f64, _b: &Brush) {
            self.strokes += 1;
        }
        fn stroke_line(&mut self, _p0: Point, _p1: Point, _w: f64, _c: Color) {
            self.lines += 1;
        }
    }

    #[test]
    fn paints_two_halves_a_divider_and_a_chevron() {
        let mut w = build_split(false);
        with_rects(&mut w);
        let mut rec = PathRecorder::default();
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(144.0, 40.0));
        w.paint(&mut ctx, &mut rec);
        assert_eq!(rec.fills, 2, "leading + trailing half backgrounds");
        assert_eq!(rec.strokes, 1, "the chevron path");
        assert_eq!(rec.lines, 1, "the hairline divider");
    }

    #[test]
    fn themed_paint_resolves_secondary_container() {
        let theme = Theme::m3_baseline();
        let mut w = build_split(false);
        with_rects(&mut w);

        #[derive(Default)]
        struct ColorRec {
            fills: Vec<Color>,
        }
        impl PaintScene for ColorRec {
            fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
            fn draw_text(&mut self, _o: Point, _t: &str) {}
            fn fill_path(&mut self, _o: Point, _p: &BezPath, b: &Brush) {
                if let Brush::Solid(c) = b {
                    self.fills.push(*c);
                }
            }
        }

        let mut rec = ColorRec::default();
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(144.0, 40.0)).with_theme(&theme);
        w.paint(&mut ctx, &mut rec);
        assert_eq!(rec.fills[0], theme.scheme().secondary_container);
    }

    #[test]
    fn semantics_yields_a_group_with_leading_and_expandable_trailing() {
        fn logic(_s: &mut ()) -> SplitButtonView<()> {
            split_button::<(), _, _>("Save", true, |_s: &mut ()| {}, |_s: &mut ()| {})
        }
        let mut root: frust_core::RenderRoot<(), SplitButtonView<()>> =
            frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = frust_text::TextContext::new();
        root.layout_with_text(Size::new(300.0, 60.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();

        let group = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Group)
            .expect("a Group container node is contributed");
        assert_eq!(group.1.children().len(), 2);

        let leading = update
            .nodes
            .iter()
            .find(|(_, n)| n.label() == Some("Save"))
            .expect("the leading action node is present");
        assert!(leading.1.supports_action(Action::Click));

        let trailing = update
            .nodes
            .iter()
            .find(|(_, n)| n.label() == Some("Show menu"))
            .expect("the trailing menu node is present");
        assert_eq!(trailing.1.is_expanded(), Some(true));
        assert!(trailing.1.supports_action(Action::Expand));
    }
}
